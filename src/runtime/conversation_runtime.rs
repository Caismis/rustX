//! The conversation runtime coordinator (Issue #61): the semantic owner of
//! conversation coordination for one conversation.
//!
//! [`ConversationRuntime`] owns the semantic conversation/runtime state that
//! used to live inside the old Runtime Client host:
//!
//! ```text
//! conversation identity / agent identity
//! authoritative mutable session model state
//! between-attempt bounded ConversationState hot read model
//! ConversationStore and its RequestHistory durable read handle
//! attempt-id allocation
//! the current-attempt slot and its cancellation handle
//! attempt admission (the ONE admission owner)
//! current model request timeout execution policy and monotonic clock
//! ordinary inbound acceptance/admission coordination
//! the ConversationInboundMailbox active-process relationship
//! ConversationToolRuntime / ConversationBackgroundRegistry
//! CapabilityCoordinator
//! context/request assembly dependencies (policy, estimator, Agent Status engine)
//! the lifecycle/drain authority
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
//! # Lifecycle: activation, drain, and quiescence
//!
//! The runtime has one authoritative lifecycle, shared with every
//! conversation-owned semantic boundary:
//!
//! ```text
//! ConversationRuntime::new(..)          -> Inactive
//!     [optional] pre-activation MCP settlement failure -> Draining
//!     [optional] RuntimeClientHost::new(..)   binds the client adapter
//! ConversationRuntime::activate()       -> Running
//! ConversationRuntime::shutdown()       -> Draining, then awaits Quiescent
//! ```
//!
//! `Running -> Draining` is the exact drain linearization point. It is
//! performed under the coordinator lock that also serializes inbound
//! acceptance, model updates, and attempt admission. Background ownership and
//! capability revision commits use the same lifecycle at their native
//! synchronization boundaries. After the transition, no new semantic work
//! may begin; already-owned work retains only its typed settlement path.
//!
//! Cancellation requested, operation settled, and runtime quiescent are
//! different facts. A cancellation signal, a dropped waiter, an OS signal,
//! or an empty registry is not settlement. Quiescence is published only
//! after the current Agent Execution **and its attempt task**, foreground
//! tools, conversation-owned background terminal publication, counted
//! capability/environment preparation (including in-flight MCP connection
//! owners), retained MCP process closure, owned process terminality, and
//! the admission worker's exit boundary have all settled.
//!
//! Drain is a supervisor, not a short-circuiting pipeline: it closes
//! admission, requests cancellation/closure of every concrete owner,
//! supervises each owner to its own native terminal boundary, and only then
//! decides between `Quiescent` and one aggregated settlement failure. A
//! failure in one participant is evidence, never permission to abandon a
//! sibling that can still act.
//!
//! An authoritative MCP `PhysicalSettlement` failure is persistent runtime
//! fencing authority even during the inactive phase. The retirement callback
//! enters the failure-drain operation, whose one coordinator critical section
//! publishes the latch and closes lifecycle admission together; if it
//! predates activation, the runtime enters the explicit failure-drain
//! lifecycle and activation cannot reopen healthy admission. If it races
//! activation, the same lock orders the failure before or after the
//! `Inactive -> Running` transition; a failure that follows a successful
//! activation immediately starts the existing drain.
//!
//! Construction performs one **tool-runtime ownership transfer** over the
//! `ConversationToolRuntime` it claims (Issue #61): under the background
//! registry synchronization boundary it requires a pristine background
//! plane (no prepared dispatch, no committed record), claims the one-time
//! coordinator binding, and binds the canonical mailbox runtime-owned with
//! a fresh `Inactive` shared lifecycle — all at one linearization point.
//! Either a standalone background commit wins first (construction fails
//! typed with [`ConversationRuntimeError::ToolRuntimeNotQuiescent`] and
//! nothing is consumed) or the transfer wins first (a later background
//! commit is refused with
//! [`BackgroundDispatchError::ConversationInactive`](crate::tools::background::BackgroundDispatchError::ConversationInactive)).
//! A runtime is therefore constructed only over a pristine background
//! plane, and the inactive phase can never inherit a detached semantic
//! transition.
//!
//! # One lifecycle authority
//!
//! The conversation has exactly **one** authoritative lifecycle state:
//! the [`ConversationLifecycle`](crate::runtime::types::ConversationLifecycle)
//! composed by the runtime and shared with every runtime-owned semantic
//! boundary — the inbound mailbox (runtime ownership is the lifecycle
//! handle; the mailbox keeps no activation flag), the background registry
//! (reads the same gate through its mailbox), the capability coordinator
//! (reads the same handle attached at its claim), and the coordinator
//! itself. `activate` performs the single `Inactive -> Running` transition
//! and `shutdown` performs the single `Running -> Draining` transition of
//! that one lifecycle. Every runtime-owned semantic commit observes it:
//!
//! ```text
//! operation observes Inactive
//!     -> it linearizes before activation
//!     -> runtime-semantic commit is refused (typed, consumes nothing)
//!
//! operation observes Running
//!     -> it linearizes before drain
//!     -> normal subsystem rules apply
//!
//! operation observes Draining or Quiescent
//!     -> new semantic admission is refused
//!     -> required settlement remains allowed only while Draining
//! ```
//!
//! There is no subsystem-specific lifecycle state, so two runtime-owned
//! subsystems cannot disagree about whether the conversation admits new
//! work. The ownership transfer (`standalone -> runtime-owned/inactive`),
//! activation (`Inactive -> Running`), drain (`Running -> Draining`), and
//! quiescence publication (`Draining -> Quiescent`) are distinct commit
//! points with distinct contracts.
//!
//! An **inactive** runtime is inert for semantic mutation, and this is
//! enforced, not merely documented. The one composition/readiness exception
//! is counted capability/background preparation; it cannot publish a live
//! revision or owned execution until a later `Running` commit:
//!
//! ```text
//! ConversationRuntime constructed
//!     |
//!     |  inactive composition phase
//!     |    no inbound admission        (mailbox refuses enqueue)
//!     |    no model mutation           (model_set: ModelUpdateError::Inactive)
//!     |    no ordinary shutdown transition (shutdown: ShutdownError::Inactive)
//!     |    no background dispatch commit (registry: BackgroundDispatchError::ConversationInactive)
//!     |    no ordinary capability commit (coordinator: CapabilityCommitError::RuntimePublicationRequired)
//!     |    counted candidate preparation (composition only; no live commit)
//!     |
//! [optional RuntimeClientHost bootstrap]
//!     |
//! ConversationRuntime::activate()      <- the one Inactive -> Running transition
//!     |
//! all runtime semantic mutations may begin
//! ```
//!
//! An authoritative MCP physical-settlement failure is the one terminal
//! exception to that inactive diagram: it explicitly transitions the runtime
//! into `Draining` for final settlement reporting. It is not an activation and
//! cannot be followed by healthy admission.
//!
//! The lifecycle state is an `AcqRel/Acquire` atomic, and its short native
//! commit boundary also serializes activation, drain, background ownership,
//! and capability revision commits. The coordinator lock remains the
//! conversation authority for inbound acceptance, attempt admission, model
//! changes, and the host-binding handshake; the shared lifecycle boundary
//! covers the runtime-owned commits that do not take that lock. A bootstrap
//! that acquires the coordinator lock first sees `Inactive` and completes
//! before activation, one that acquires it after sees `Running` and is
//! refused ([`RuntimeBootstrapError::RuntimeAlreadyActivated`]).
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
//! into the runtime-owned observation fan-out. The Runtime Client projection
//! owns the primary [`PendingObservations`](crate::runtime::observation::PendingObservations)
//! queue and folds it under its own synchronization boundary, translating the
//! semantic observations into its snapshot/cursor read model (see
//! `RuntimeClientProjection`), so snapshot/cursor reads remain linearizable.
//! Existing bounded local observation surfaces may receive a separate
//! disposable fan-out queue; that is not a second Runtime Client or history
//! authority. A conversation with zero Runtime Client attachments runs the
//! exact same admission/execution path; only the installed host's primary
//! queue is absent when no host exists.
//!
//! # The bootstrap cut
//!
//! [`ConversationRuntime::install_observation_bridge`] is the one runtime
//! handshake a Runtime Client adapter uses at construction. It runs
//! entirely under the one coordinator lock, refuses an already-activated
//! runtime under that same lock, installs the observation fan-out and every
//! subsystem observation seam, and captures the bootstrap seed:
//!
//! ```text
//! T0  coordinator lock; reject if activated; install the queue;
//!     capture shutting_down / current Surface messages / session model
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
//! - the background registry is pristine by construction — the ownership
//!   transfer requires no committed record and no prepared dispatch, and
//!   `commit_dispatch` refuses its mailbox while it is bound inactive — so
//!   no background record exists across `[T0, R]` and none can be created;
//! - the mailbox refuses `enqueue` while its bound runtime is inactive, so
//!   the pending queue is frozen across `[T0, R]`;
//! - the capability coordinator refuses a runtime-owned ordinary `commit`
//!   before activation, and the capability snapshot is captured *at* `R`;
//!   live publication is available only through the runtime resource owner.
//!
//! And because each authority's observer installation shares one lock
//! section with its own seed capture, no transition can be both seeded and
//! queued, and none can be neither.
//!
//! The bootstrap cut `R` **precedes** the activation transition: the
//! handshake completes over the inert runtime, and activation (the shared
//! `ConversationLifecycle` `Inactive -> Running` CAS) happens afterwards.
//! Because the runtime remains semantically inert from `R` until that
//! transition — the mailbox refuses `enqueue`, the background registry
//! refuses `commit_dispatch`, the capability coordinator refuses a
//! runtime-owned `commit`, and the coordinator mutators are inactive-gated —
//! no projected semantic fact can appear in the interval `[R, activation)`.
//! The live stream therefore carries *every* observation the runtime ever
//! emits, and the first allocated `RuntimeClientCursor` always belongs to a
//! real post-activation semantic transition — bootstrap state never
//! fabricates a live event.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use crate::agent::cancellation::AgentCancellation;
use crate::agent::observer::{AgentExecutionObserver, AgentStatusObservation};
use crate::agent::{AgentExecution, AgentExecutionRequest};
use crate::capabilities::{CapabilityCoordinator, CapabilityObserver, CapabilitySnapshot};
use crate::context::compaction::{
    CompactionAttribution, CompactionExecutionError, ExecutedCompaction, execute_compaction,
};
use crate::context::tokens::TokenEstimator;
use crate::context::{
    AgentStatusEngine, CompactionConstraints, ContextError, ContextErrorKind, ContextRuntime,
    NativeContextInput, SessionContextPolicy, render_effective_system_prompt,
};
use crate::conversation::ConversationState;
use crate::conversation::SurfaceRevision;
use crate::durable::{
    ConversationStore, ConversationStoreError, InboundDraft, SurfaceUserMessageBoundary,
    SurfaceUserMessageBoundaryPage, TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT, TranscriptPage,
};
use crate::events::types::RuntimeEvent;
use crate::message::types::{InboundKind, MessageBlock, UserContentBlock, UserSource};
use crate::model::catalog::ModelCatalogView;
use crate::model::deadline::ModelTimeoutPolicy;
use crate::model::session::{
    AttemptModelSnapshot, SessionModelConfig, SessionModelState, SessionModelView,
};
use crate::model::{ModelRequest, RequestIdentity, invocation::ModelInvocationError};
use crate::publication::{PublicationAudit, PublicationFrame, PublicationStreamStart};
use crate::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolExecutionId};
use crate::runtime::inbound::{
    ConversationInboundMailbox, FreshInboundTurn, InboundBatch, InboundItem, InboundObserver,
    InboundSequence, InitialTurnTrigger, MailboxError,
};
use crate::runtime::interaction::{
    InteractionCoordinator, InteractionObserver, InteractionOutcome, InteractionRef,
    InteractionRequest, InteractionRoute, RoutedInteraction, RoutedInteractionError, route_error,
};
use crate::runtime::monotonic::{MonotonicClock, SystemMonotonicClock};
use crate::runtime::observation::{
    ConversationObservation, ObservationFanout, PendingObservations,
};
use crate::runtime::request_history::RequestHistory;
use crate::runtime::resources::{RuntimeResourceLoader, RuntimeResourceSnapshot};
use crate::runtime::transcript_history::TranscriptHistory;
use crate::runtime::types::{
    ApprovalMode, CancellationReason, ConversationLifecycle, ConversationLifecycleState,
    DurabilityFailureCommit, DurabilityGate, DurableOperation, RuntimeClock, SystemClock,
};
use crate::tools::background::{BackgroundExecutionSnapshot, BackgroundObserver};
use crate::tools::runtime::ConversationToolRuntime;
use crate::tools::types::ModelToolDefinition;

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
    /// The materialized native Agent Extension facets of this composition do
    /// not all follow from **one** frozen composition (Issue #259).
    ///
    /// A conversation's Todo state owner, its extension Tool plane, its
    /// Agent Status engine, and the composition the Runtime Client projects
    /// are four facets of a single decision, and this runtime is where they
    /// are transferred into one owner. Construction therefore proves they
    /// agree instead of trusting the caller to have passed matching values:
    /// a runtime that offered the model `todo` while owning no
    /// `ConversationTodoList` would fail every call for a purely
    /// architectural reason, and one owning a list it never offers a Tool for
    /// would publish a task list nothing can change.
    ///
    /// It fails closed rather than running a composition that cannot be
    /// described coherently, and the two Tool facets are reported separately
    /// because they fail for different reasons: a wrong *configured* plane
    /// means the coordinator was built for another conversation's
    /// materialization, while a missing *active* Tool means the coordinator
    /// has not published an executable generation carrying it yet.
    ExtensionCompositionMismatch {
        /// The conversation whose facets disagree.
        conversation_id: ConversationId,
        /// Whether the frozen composition of the attached conversation
        /// composes the Todo extension.
        composed_todo: bool,
        /// Whether the conversation tool runtime materialized a Todo state
        /// owner.
        materialized_todo_state: bool,
        /// Whether the capability coordinator's **configured** extension Tool
        /// plane names the `todo` Tool.
        ///
        /// This is an input to future capability preparation, never proof
        /// that anything executable carries the Tool.
        configured_todo_tool: bool,
        /// Whether the coordinator's **currently active** `CapabilitySnapshot`
        /// publishes the canonical Todo Tool authority.
        ///
        /// This is the executable fact: `false` with `composed_todo` true
        /// means the model would be told it has `todo` while the active
        /// generation cannot dispatch it — which is exactly what an
        /// unprepared, uncommitted coordinator carries at revision zero.
        active_todo_tool: bool,
        /// Whether the composition's Agent Status engine materialization
        /// matches the frozen composition.
        agent_status_agrees: bool,
        /// Whether Goal state and the complete exact command surface agree.
        goal_agrees: bool,
    },
    /// The context engine configuration is impossible.
    Context(String),
    /// The runtime-owned model request deadline policy is invalid.
    InvalidModelTimeoutPolicy,
    /// The runtime-owned tool execution-liveness policy is invalid.
    InvalidToolDeadlinePolicy,
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
    /// The conversation tool runtime is not pristine: its background plane
    /// already holds a prepared dispatch or a committed execution record,
    /// so it cannot become the inactive semantic base of a new
    /// `ConversationRuntime`.
    ///
    /// The ownership transfer linearizes against the background dispatch
    /// ownership commit at the registry synchronization boundary: either a
    /// standalone background commit wins first (this failure) or the
    /// transfer wins first and a later commit is refused with
    /// [`BackgroundDispatchError::ConversationInactive`](crate::tools::background::BackgroundDispatchError::ConversationInactive).
    /// The claim is never consumed by this failure: once the background
    /// plane is pristine again, a fresh construction of the same identity
    /// may succeed.
    ToolRuntimeNotQuiescent {
        /// The conversation whose tool runtime is not pristine.
        conversation_id: ConversationId,
    },
    /// The supplied `SubagentRegistry` does not belong to this
    /// conversation's ownership domain: another `ConversationId`, another
    /// parent `AgentId`, or a different canonical inbound mailbox.
    ///
    /// The runtime may only coordinate the conversation's own subagent
    /// plane; a registry for another conversation/agent/mailbox domain is
    /// rejected before anything is claimed.
    SubagentOwnershipMismatch {
        /// The registry's conversation owner.
        registry_conversation: ConversationId,
        /// The runtime's conversation owner.
        runtime_conversation: ConversationId,
    },
    /// The supplied `SubagentRegistry` already owns a committed child
    /// record.
    ///
    /// Construction requires a pristine logical subagent plane: a registry
    /// with live children can never be silently adopted by a runtime that
    /// did not own their start.
    SubagentRegistryNotPristine {
        /// The conversation whose subagent plane is not pristine.
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
    /// Startup recovery could not establish a coherent conversation from the
    /// durable authority (Issue #12, M9a).
    ///
    /// This supersedes the coarse M8 restart gate: the runtime no longer
    /// refuses every non-trivial durable tail, it classifies and reconciles
    /// it. This variant is what remains — the reconciled state is *still*
    /// incoherent, so no runtime is produced and nothing is admitted as
    /// though recovery had completed.
    RecoveryRequired {
        /// Why the durable head cannot be resumed safely.
        reason: String,
    },
    /// The durable Pending Inbound Inbox failed a storage operation.
    Storage(String),
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
            Self::ExtensionCompositionMismatch {
                conversation_id,
                composed_todo,
                materialized_todo_state,
                configured_todo_tool,
                active_todo_tool,
                agent_status_agrees,
                goal_agrees,
            } => write!(
                f,
                "the native Agent Extension facets of conversation {conversation_id} do not follow from one frozen composition: \
                 composed todo={composed_todo}, materialized todo state={materialized_todo_state}, \
                 configured todo Tool={configured_todo_tool}, active todo Tool={active_todo_tool}, \
                 agent status agrees={agent_status_agrees}, Goal facets agree={goal_agrees}"
            ),
            Self::Context(message) => write!(f, "context configuration failed: {message}"),
            Self::InvalidModelTimeoutPolicy => write!(
                f,
                "model request timeout policy is invalid: response-start and stream-idle timeouts must be positive"
            ),
            Self::InvalidToolDeadlinePolicy => write!(
                f,
                "tool execution deadline policy is invalid: the hard deadline and any idle-liveness window must be positive"
            ),
            Self::InvalidInitialConversation(message) => write!(
                f,
                "the initial canonical conversation is invalid: {message}"
            ),
            Self::RuntimeAlreadyBound { conversation_id } => write!(
                f,
                "the conversation runtime identity of {conversation_id} is already bound to a conversation runtime"
            ),
            Self::ToolRuntimeNotQuiescent { conversation_id } => write!(
                f,
                "the conversation tool runtime of {conversation_id} is not pristine: it already contains prepared or committed background work and cannot become the inactive semantic base of a new conversation runtime"
            ),
            Self::SubagentOwnershipMismatch {
                registry_conversation,
                runtime_conversation,
            } => write!(
                f,
                "the subagent registry belongs to conversation {registry_conversation}, not to this runtime's conversation {runtime_conversation}"
            ),
            Self::SubagentRegistryNotPristine { conversation_id } => write!(
                f,
                "the subagent registry of {conversation_id} is not pristine: it already owns committed child records and cannot become the logical base of a new conversation runtime"
            ),
            Self::NoExecutionRuntime => write!(
                f,
                "the conversation runtime requires a Tokio execution runtime at construction"
            ),
            Self::RecoveryRequired { reason } => write!(
                f,
                "the durable conversation head is not at a recovery-safe boundary: {reason}"
            ),
            Self::Storage(message) => {
                write!(f, "the durable ConversationStore failed: {message}")
            }
        }
    }
}

impl std::error::Error for ConversationRuntimeError {}

/// The shared context-plane pieces of one conversation runtime.
///
/// These are the **composition-owned static** context pieces: the token
/// estimator, the Agent Status engine template, and the current runtime
/// context policy (reserve tokens, keep-recent target, summary output cap).
/// They persist across attempts, and the model path and the Runtime Client
/// projection share one accepted Agent Status generation.
///
/// There is deliberately no separate summary store: compaction lineage is
/// derived from the immutable Conversation Surface history owned by the
/// durable `ConversationStore`, while the one `ConversationState` is only the
/// current hot read model.
///
/// The context *window* is deliberately absent: it belongs to the model, so
/// each attempt derives its [`ContextRuntime`] from this policy plus that
/// attempt's immutable model snapshot. No window captured at process start
/// can survive a session model change.
pub struct ConversationContextConfig {
    /// The current runtime context policy captured by this composition.
    pub policy: SessionContextPolicy,
    /// The deterministic token estimator.
    pub estimator: Arc<dyn TokenEstimator>,
    /// The launch-scoped Agent Status engine template, present exactly when
    /// this composition's frozen native Agent Extension set contains the
    /// Agent Status extension (Issue #256). Each admitted attempt constructs
    /// a fresh engine from it, retaining the configured clock/module
    /// semantics while keeping quarantine state attempt-local.
    ///
    /// `None` is the whole representation of "this runtime composes no Agent
    /// Status": nothing else in the Agent Loop, Tool Plane, cancellation, or
    /// durability path consults the extension set.
    pub status_engine: Option<AgentStatusEngine>,
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
    /// Deliberate Session intent, distinct from the authored Root default.
    pub explicit_model: bool,
    /// The agent executed by attempts of this runtime.
    pub agent_id: AgentId,
    /// The session's authoritative model state: the binding registry plus
    /// the initial desired configuration, already resolved and validated.
    ///
    /// This is the one model authority of the conversation. Attempts freeze
    /// snapshots of it; a client updates it through `model_set`; nothing
    /// else in the process resolves a provider binding.
    pub model: SessionModelState,
    /// The launch-scoped runtime approval mode.
    pub approval_mode: ApprovalMode,
    /// The current runtime-owned model request deadline policy. It is copied
    /// at attempt/manual-operation admission and never enters model input or
    /// durable historical state.
    pub model_timeout_policy: ModelTimeoutPolicy,
    /// The current runtime-owned tool execution-liveness policy (Issue #204).
    /// It is copied at attempt admission into the attempt's frozen execution
    /// policy, so a running `ToolCall` never observes a later value; it never
    /// enters model input, executor context, or durable historical state.
    pub tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy,
    /// The shared context-plane pieces.
    pub context: ConversationContextConfig,
    /// The conversation tool runtime (owns the canonical mailbox and the
    /// authoritative background registry).
    pub tool_runtime: ConversationToolRuntime,
    /// The capability coordinator (owns the active capability snapshot).
    pub capability: CapabilityCoordinator,
    /// The complete immutable process-local resource generation initially
    /// active for this runtime.
    pub resources: Arc<RuntimeResourceSnapshot>,
    /// The resource preparer invoked off-side by native configuration coordination.
    pub resource_loader: Arc<dyn RuntimeResourceLoader>,
    /// The runtime clock stamping submitted inbound messages; the system
    /// clock is used when omitted.
    pub clock: Option<Arc<dyn RuntimeClock>>,
    /// The canonical conversation history the runtime starts from.
    pub initial_messages: Vec<MessageBlock>,
    /// The conversation-owned subagent registry (Issue #60), when this
    /// runtime may delegate to child runtimes. A subagent child itself is
    /// composed without one, so recursive delegation is absent by
    /// construction.
    pub subagents: Option<crate::runtime::subagent::SubagentRegistry>,
    /// The reserved Workflow Agent terminal protocol for a headless child,
    /// when this runtime executes one Workflow-owned `AgentRun`.
    pub workflow_output: Option<Arc<dyn crate::runtime::workflow::WorkflowOutputTerminal>>,
}

/// The runtime-owned current attempt handle.
///
/// This is a control handle only: the coordinator keeps exactly the
/// cancellation trigger the attempt task runs against so that
/// `cancel_current_attempt` can request cancellation. It does **not** own a
/// second cancellation state machine and never decides a terminal outcome;
/// [`AgentExecution`] remains the attempt execution/terminal authority.
struct CurrentAttempt {
    configuration: crate::local_runtime::configuration::settings::AdmittedConfiguration,
    /// The attempt identity.
    attempt_id: AttemptId,
    /// The attempt cancellation trigger observed by the loop.
    cancellation: AgentCancellation,
    /// What this runtime admitted (Issue #351). Runtime-owned admission
    /// provenance, never Goal lifecycle state.
    provenance: AttemptProvenance,
}

/// What the coordinator admitted an attempt *for*.
///
/// This is runtime-owned admission provenance: it is decided once, at the one
/// admission publication point, from the durable inbound the coordinator
/// actually adopted. It is deliberately **not** Goal state — it is never
/// persisted, never reaches `GoalView`, never reaches a Runtime Client
/// snapshot or event, and disappears with the attempt it describes. It exists
/// so an explicit interrupt can tell an autonomous Goal continuation apart
/// from an ordinary Human attempt, including a Human attempt during which the
/// model called `create_goal`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AttemptProvenance {
    /// Ordinary adopted inbound that contains no Goal continuation: Human
    /// input, native results, and every other runtime-generated inbound.
    Inbound,
    /// The adopted batch carries the durable Goal continuation admitted at
    /// the Goal-round frontier. Exactly this attempt is an autonomous Goal
    /// continuation. The reference is the post-accounting authority, captured
    /// from the accepted inbound rather than a later current-Goal read.
    GoalContinuation(crate::goal::GoalRef),
    /// Ordinary recovery answer obligation with no autonomous Goal authority.
    RecoveredContinuation,
    /// Already-adopted Goal work: recovery retained the exact post-accounting
    /// reference from durable history. Admission consumes no additional round.
    RecoveredGoalContinuation(crate::goal::GoalRef),
}

/// The runtime-owned manual compaction currently holding the conversation.
///
/// Manual compaction is not an attempt: it allocates no `AttemptId`, emits no
/// attempt lifecycle, and cannot execute tools. The coordinator nevertheless
/// retains its cancellation trigger so runtime drain can supervise the
/// provider-backed summary to terminal settlement.
struct CurrentManualCompaction {
    cancellation: AgentCancellation,
}

struct ManualCompactionTaskResult {
    conversation: ConversationState,
    result: Result<ManualCompactionSuccess, ManualCompactionError>,
}

/// A successful task-local compaction whose durable facts have not yet been
/// published to Runtime Client observers. Publication belongs to coordinator
/// settlement, after the checked-out conversation and maintenance slot have
/// returned.
struct ManualCompactionSuccess {
    outcome: ManualCompactionOutcome,
    completed: ExecutedCompaction,
}

/// The semantic durable operations of the coordinator's durability-health
/// contract (Issue #63).
///
/// The retry budget is owned by one finite admission cycle, not by the last
/// operation that happened to fail. [`AdmissionRetryBudget`] retains the
/// consumed allowance for each transient stage while that cycle moves from
/// selection to adoption.
///
/// Only genuine transient storage failures earn a retry. A semantic
/// contract failure (a pending item that cannot be prepared for canonical
/// adoption, an incomplete tool turn observed by the admission guard) is
/// persistent by nature — retrying the identical transition is futile — and
/// an already-terminal durable failure (an active attempt's canonical-write
/// failure, an exhausted background publication budget) has already consumed
/// its settlement; all of these fail closed immediately.
///
/// The bounded transient retry allowance of one finite admission cycle.
///
/// Each transient stage may fail once and receive one immediate re-kick. The
/// bits are deliberately retained when the cycle advances from selection to
/// adoption, so a later failure cannot erase earlier retry debt. The budget
/// is reset only when the cycle reaches a semantic completion boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AdmissionRetryBudget {
    /// Whether the one select retry has already been consumed.
    select_retry_used: bool,
    /// Whether the one adopt retry has already been consumed.
    adopt_retry_used: bool,
}

impl AdmissionRetryBudget {
    /// Consumes the one retry allowance for a transient operation.
    ///
    /// Returns `true` when the allowance was available and is now consumed;
    /// returns `false` when that operation has already failed once in the
    /// current admission cycle.
    fn try_consume(&mut self, operation: DurableOperation) -> bool {
        let used = match operation {
            DurableOperation::SelectPendingBatch => &mut self.select_retry_used,
            DurableOperation::AdoptPendingBatch => &mut self.adopt_retry_used,
            _ => unreachable!("only transient operations have an admission retry budget"),
        };
        if *used {
            false
        } else {
            *used = true;
            true
        }
    }
}

/// The most recent transient failure whose bounded re-kick is in flight.
///
/// This is only wake/diagnostic state. It is intentionally separate from
/// [`AdmissionRetryBudget`], because operation identity must not own the
/// lifetime of retry debt.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingDurabilityRetry {
    /// The operation that will be retried by the re-kick.
    operation: DurableOperation,
    /// The first-failure diagnostic.
    diagnostic: String,
}

/// The coordinator's durable-authority health state (Issue #63).
///
/// A storage failure that a required transition cannot proceed without is
/// never silently swallowed and never retried forever. The admission-cycle
/// The coordinator-owned transient admission-cycle retry bookkeeping
/// (Issue #63).
///
/// This is **not** the runtime durability-health authority: the absorbing
/// `DurabilityFailed` fact lives in exactly one place, the runtime-owned
/// [`DurabilityGate`](crate::runtime::types::DurabilityGate). The
/// coordinator keeps only the finite-cycle retry allowances and the latest
/// pending re-kick here; the decision to upgrade to the absorbing failure
/// is made under the coordinator lock and committed through
/// `DurabilityGate::commit_failure` — the one mutation API of the failure
/// authority.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AdmissionDurabilityCycle {
    /// The independent select/adopt retry allowances for this cycle.
    budget: AdmissionRetryBudget,
    /// The latest transient failure whose re-kick is armed.
    pending_retry: Option<PendingDurabilityRetry>,
}

/// The one synchronized coordinator state (the admission linearization
/// owner).
#[allow(clippy::struct_excessive_bools)] // Independent admission, lifecycle, and maintenance facts share one lock.
struct CoordinatorState {
    binding_revision: u64,
    explicit_model: bool,
    /// The session's authoritative mutable model state.
    ///
    /// It lives under the *same* lock that owns attempt admission, so a
    /// model update and an attempt admission can never interleave
    /// ambiguously: whichever acquires the lock first linearizes first.
    model: SessionModelState,
    /// The complete resource/capability generation future attempts acquire.
    resources: Arc<RuntimeResourceSnapshot>,
    /// The runtime-owned persistent fence for an MCP physical settlement that
    /// is not proven. The retirement registry remains the complete diagnostic
    /// authority; this latch is the coordinator's admission authority and is
    /// published under the same lock that serializes activation.
    mcp_settlement_failure: Option<String>,
    /// The effective mode frozen for the currently admitted attempt boundary.
    effective_approval_mode: ApprovalMode,
    /// The latest requested runtime control mode.
    /// Monotonic control-plane revision; idempotent requests do not advance it.
    /// The one canonical conversation state, owned by the coordinator
    /// **only between attempts**.
    ///
    /// Ownership is structural, not conventional: admission moves the state
    /// out (`take`), so while an attempt runs this slot is `None` and the
    /// coordinator physically cannot mutate a competing copy. Settlement
    /// moves the authoritative state back in.
    conversation: Option<ConversationState>,
    /// The current attempt slot (None = idle).
    current_attempt: Option<CurrentAttempt>,
    /// The current idle-maintenance compaction, when it has checked out the
    /// sole mutable conversation state.
    manual_compaction: Option<CurrentManualCompaction>,
    /// The next attempt identity ordinal.
    ///
    /// Seeded by startup recovery to one past the highest ordinal that
    /// already entered durable authority (Issue #12, M9a), so a restart can
    /// never reuse an `AttemptId` that names a different logical attempt in
    /// durable history. The durable Event Journal rejects a second
    /// `AttemptStarted` for one identity, so the invariant is enforced on
    /// both sides.
    next_attempt_seq: u64,
    /// How many attempts this coordinator has admitted, counted at the one
    /// admission publication point (Issue #193).
    ///
    /// This is the child driver's completeness proof, not a lifecycle fact:
    /// every admitted attempt produces exactly one terminal observation, so
    /// a driver that has consumed `n` terminals knows an attempt it has not
    /// observed exists precisely when this counter exceeds `n`. It is read
    /// only under the coordinator lock, together with the current-attempt
    /// slot and the durable pending inbox, by
    /// [`ConversationRuntime::seal_parent_guidance`].
    admitted_attempts: u64,
    /// Native child turn permits are released only after parent lifecycle admission.
    parent_turn_permits: Option<u64>,
    /// Whether this conversation's parent-authored guidance admission is
    /// sealed (Issue #193).
    ///
    /// The seal is the child conversation's **terminal linearization point**
    /// for in-flight steering. It commits under the same coordinator lock
    /// that owns durable inbound acceptance, and only when no accepted
    /// guidance can still reach an Agent Loop boundary: the pending inbox is
    /// empty, no attempt is live, and no admitted attempt is unobserved.
    /// Once set, every later guidance submission is refused, so a durably
    /// accepted guidance and a sealed terminal are mutually exclusive by
    /// construction rather than by timing. It is absorbing, and only the
    /// subagent child driver ever sets it: an ordinary interactive
    /// conversation never seals.
    parent_guidance_sealed: bool,
    /// The committed one-shot child-cancellation intent (Issue #60 child
    /// side): armed by the subagent child control plane on
    /// `ParentFrame::Cancel` before any attempt exists, consumed by the
    /// next admission so the admitted attempt starts already-cancelled and
    /// its first model-turn arbitration resolves `CancelledBeforeStart`.
    /// The child is one-shot, so at most one admission ever consumes it;
    /// a parent conversation never arms it.
    one_shot_cancel: Option<CancellationReason>,
    /// Startup recovery's exact provenance for the already-canonical adopted
    /// turn that may continue through one new attempt.
    ///
    /// This is a one-shot permission, consumed by the first admission that
    /// finds no pending inbound. It is never set for an indeterminate
    /// external outcome (Class C), where continuing would risk duplicating an
    /// external side effect rustX cannot observe.
    recovered_continuation: Option<AttemptProvenance>,
    /// The coordinator-owned transient admission-cycle retry bookkeeping
    /// (Issue #63). The absorbing `DurabilityFailed` fact itself is NOT
    /// stored here: it lives in exactly one place, the runtime-owned
    /// [`DurabilityGate`], which every reader (`submit_inbound`, `model_set`,
    /// attempt admission, the registries, the shutdown diagnostic, the
    /// observation) consults. This field only tracks the finite-cycle retry
    /// allowances and the latest pending re-kick of the admission worker.
    admission_durability_cycle: AdmissionDurabilityCycle,
}

/// The runtime admission worker's wake boundary.
///
/// A leaf: one `Notify` plus a closed flag, owned by the worker task and the
/// runtime. The mailbox observer wakes it on every enqueue, so an idle
/// conversation admits asynchronous inbound without any client request.
struct WakeGate {
    /// The wake signal.
    notify: tokio::sync::Notify,
    /// Set by explicit drain or by the runtime's `Drop`. Terminal for the
    /// worker.
    closed: AtomicBool,
    /// Set by the worker after it has observed the closed gate and exited.
    exited: AtomicBool,
    /// Wakes a drain waiter after worker exit.
    exit_notify: tokio::sync::Notify,
    /// Test-only worker-exit signal.
    #[cfg(test)]
    worker_exit: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl WakeGate {
    fn new() -> Self {
        Self {
            notify: tokio::sync::Notify::new(),
            closed: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            exit_notify: tokio::sync::Notify::new(),
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

    fn mark_exited(&self) {
        self.exited.store(true, Ordering::Release);
        self.exit_notify.notify_waiters();
    }

    async fn wait_until_exited(&self) {
        loop {
            if self.exited.load(Ordering::Acquire) {
                return;
            }
            let notified = self.exit_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.exited.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
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

/// The one shared completion of the runtime drain. Every concurrent shutdown
/// caller waits on this same object; no caller creates a competing drain
/// state machine.
#[derive(Debug, Default)]
struct DrainCompletion {
    completed: AtomicBool,
    result: Mutex<Option<Result<(), ShutdownError>>>,
    notify: tokio::sync::Notify,
}

/// Conservative native idle refusal; no App Server copy of execution state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdleBusyReason {
    LifecycleOwner,
    Foreground,
    Recovery,
    Durability,
    AutonomousExtension,
    Inbound,
    Interaction,
    Background,
    Subagent,
    AdmissionChanged,
}

/// The narrow set of operations that can start the one runtime drain.
///
/// An MCP settlement failure is a drain trigger rather than a separate
/// pre-drain mutation: its diagnostic latch and lifecycle transition are
/// published by the same coordinator critical section.
enum DrainTrigger {
    /// Explicit runtime shutdown.
    RuntimeShutdown,
    /// An authoritative MCP physical-settlement failure.
    McpSettlementFailure(String),
}

/// One runtime-owned manual compaction completion. The provider operation is
/// spawned independently of the protocol waiter, so dropping an attachment
/// can never strand the checked-out conversation state.
#[derive(Debug, Default)]
struct ManualCompactionCompletion {
    completed: AtomicBool,
    result: Mutex<Option<Result<ManualCompactionOutcome, ManualCompactionError>>>,
    notify: tokio::sync::Notify,
}

impl ManualCompactionCompletion {
    fn complete(&self, result: Result<ManualCompactionOutcome, ManualCompactionError>) {
        *self
            .result
            .lock()
            .expect("manual compaction completion lock poisoned") = Some(result);
        self.completed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn wait(&self) -> Result<ManualCompactionOutcome, ManualCompactionError> {
        loop {
            if self.completed.load(Ordering::Acquire) {
                return self
                    .result
                    .lock()
                    .expect("manual compaction completion lock poisoned")
                    .clone()
                    .expect("completed manual compaction has a result");
            }
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.completed.load(Ordering::Acquire) {
                return self
                    .result
                    .lock()
                    .expect("manual compaction completion lock poisoned")
                    .clone()
                    .expect("completed manual compaction has a result");
            }
            notified.await;
        }
    }
}

impl DrainCompletion {
    fn complete(&self, result: Result<(), ShutdownError>) {
        *self.result.lock().expect("drain completion lock poisoned") = Some(result);
        self.completed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn wait(&self) -> Result<(), ShutdownError> {
        loop {
            if self.completed.load(Ordering::Acquire) {
                return self
                    .result
                    .lock()
                    .expect("drain completion lock poisoned")
                    .clone()
                    .expect("completed drain has a result");
            }
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.completed.load(Ordering::Acquire) {
                return self
                    .result
                    .lock()
                    .expect("drain completion lock poisoned")
                    .clone()
                    .expect("completed drain has a result");
            }
            notified.await;
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
/// - `activation_gate`: parked at the entrance of `ConversationRuntime::activate`,
///   **before** the coordinator lock is acquired and before the lifecycle
///   transition, so while the park holds the conversation is provably
///   still `Inactive` and every competing runtime-owned commit or host
///   bind can still proceed. Releasing the gate commits the one
///   `Inactive -> Running` transition.
/// - `manual_compaction_settlement_gate`: parked after the durable compaction
///   commit and task-local hot-state installation, but before the coordinator
///   restores `ConversationState`, clears the maintenance slot, or publishes
///   manual completion to Runtime Client observers.
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
    /// Parks the next activation before the lifecycle transition when
    /// armed.
    pub(crate) activation_gate: Option<Arc<Gate>>,
    /// Parks the next manual compaction immediately before coordinator
    /// settlement restores ownership and publishes completion.
    pub(crate) manual_compaction_settlement_gate: Option<Arc<Gate>>,
    /// Parks the next `submit_inbound` **after** the coordinator lock is
    /// acquired and the shutdown/activation decision is read, but **before**
    /// the durable acceptance. This is the exact critical-section window the
    /// Issue #63 (Finding 1) fix closes.
    pub(crate) submit_gate: Option<Arc<Gate>>,
    /// Signals entry to `submit_inbound` before it attempts the coordinator
    /// lock. This lets a race test prove that a competing submit was started
    /// while a failure-drain critical section still owns that lock.
    pub(crate) submit_arrival: Option<Arc<tokio::sync::Notify>>,
    /// Signals that `shutdown` reached the point just before it attempts the
    /// coordinator lock. This makes the submit-vs-shutdown ordering provable
    /// by mutex exclusion instead of a timing assumption.
    pub(crate) shutdown_arrival: Option<Arc<tokio::sync::Notify>>,
    /// Parks the first MCP failure-drain operation after it has published the
    /// failure latch, lifecycle transition, and current-attempt cancellation
    /// arbitration, but before the coordinator lock is released.
    pub(crate) mcp_failure_drain_gate: Option<Arc<Gate>>,
    /// Signals immediately after `Running -> Draining` linearizes.
    pub(crate) drain_linearization: Option<Arc<tokio::sync::Notify>>,
    /// Signals immediately before the drain task **parks on one concrete
    /// runtime-owned owner** (the current attempt, or one background
    /// execution's settlement). Observing it proves supervision is committed
    /// to awaiting that owner: a drain that short-circuited on an
    /// already-known failure could never reach the park.
    pub(crate) drain_supervision: Option<Arc<tokio::sync::Notify>>,
    /// Parks the settled attempt **task** after the current-attempt slot is
    /// cleared and before its final admission callback and task exit.
    pub(crate) attempt_exit_gate: Option<Arc<Gate>>,
    /// Parks each [`ConversationRuntime::seal_parent_guidance`] evaluation
    /// **before** it acquires the coordinator lock (Issue #193).
    ///
    /// While the park holds, the attempt whose terminal the child driver
    /// just observed has passed its last inbound safe boundary and the seal
    /// has provably not committed, so a guidance submitted during the park
    /// races the terminal linearization point itself — not some arbitrary
    /// earlier instant. Like every gate above it parks exactly one
    /// evaluation and then disarms, so a seal answered `Open` and repeated
    /// runs through unparked.
    pub(crate) parent_guidance_seal_gate: Option<Arc<Gate>>,
    /// Parks the background settlement continuation **inside** its last
    /// conversation-facing callback: entered at the top of
    /// [`BackgroundFailureSink::terminal_publication_failed`], before the
    /// coordinator lock is taken and before the durability-health mutation,
    /// and therefore before the registry publishes `publication_abandoned`.
    /// While the park holds, the failing execution has provably not crossed
    /// its abandoned settlement boundary.
    pub(crate) background_failure_gate: Option<Arc<Gate>>,
    /// Parks after runtime degradation, before the registry publishes abandonment.
    pub(crate) subagent_failure_published_gate: Option<Arc<Gate>>,
    /// Installed into the **next** admitted attempt's execution: the M9b
    /// model-turn start-boundary pause (Issue #12). `take`n by the next
    /// `run_attempt`, so it arms exactly one attempt.
    pub(crate) start_boundary_pause: Option<crate::agent::execution::test_sync::StartBoundaryPause>,
    /// Installed into the next admitted attempt's model stream: parks after
    /// the selected provider item has been fully processed and before the
    /// next provider/cancellation arbitration. `take`n by the next
    /// `run_attempt`, so a drain test can hold an actually-started provider
    /// turn live while shutdown cancellation is requested.
    pub(crate) model_arbitration_pause:
        Option<crate::agent::execution::test_sync::ModelArbitrationPause>,
    /// Installed into the next admitted attempt's foreground tool batch:
    /// parks after the first `ToolExecutionStarted` fact so a test can
    /// linearize drain against the sibling start frontier.
    pub(crate) tool_start_pause: Option<crate::agent::execution::test_sync::ToolStartPause>,
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
    /// Scope the parked worker to the test, including assertion unwinding.
    pub(crate) fn arm_scoped(self: &Arc<Self>) -> GateRelease {
        self.arm();
        GateRelease(self.clone())
    }
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

#[cfg(test)]
pub(crate) struct GateRelease(Arc<Gate>);
#[cfg(test)]
impl Drop for GateRelease {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// The shared state of one conversation runtime.
pub(crate) struct RuntimeInner {
    configuration_owner: Arc<()>,
    conversation_id: ConversationId,
    agent_id: AgentId,
    context: ConversationContextConfig,
    tool_runtime: ConversationToolRuntime,
    mailbox: ConversationInboundMailbox,
    /// The conversation-level durable authority shared with the mailbox and
    /// `AgentExecution`. Request/event history is read through this handle.
    store: Arc<dyn ConversationStore>,
    capability: CapabilityCoordinator,
    /// The sole live capability-publication authority. Ordinary clones of
    /// `capability` cannot advance a claimed runtime; configuration publication must present
    /// this token so capability and resource snapshots share one boundary.
    capability_publication: crate::capabilities::RuntimeCapabilityPublication,
    resource_loader: Arc<dyn RuntimeResourceLoader>,
    /// The conversation-owned subagent registry (Issue #60), when this
    /// runtime may delegate to child runtimes.
    subagents: Option<crate::runtime::subagent::SubagentRegistry>,
    /// The frozen Workflow Agent output authority, if this is a Workflow
    /// child runtime.
    workflow_output: Option<Arc<dyn crate::runtime::workflow::WorkflowOutputTerminal>>,
    /// It owns pending identity/state and terminal response coordination, but
    /// never owns Agent Loop execution or canonical history.
    interaction: Arc<InteractionCoordinator>,
    /// The one authoritative lifecycle and drain authority of this
    /// conversation (Issue #61 / M9c): the single `Inactive -> Running`
    /// activation transition and the `Running -> Draining -> Quiescent`
    /// shutdown transitions, shared with the mailbox, background registry,
    /// and capability coordinator. The coordinator keeps no lifecycle state
    /// of its own.
    lifecycle: ConversationLifecycle,
    clock: Arc<dyn RuntimeClock>,
    /// The current model request deadline policy. Admission copies this value
    /// into each actual attempt/operation; it is never part of model state.
    model_timeout_policy: ModelTimeoutPolicy,
    /// The current tool execution-liveness policy (Issue #204). Attempt
    /// admission copies this value into the attempt's frozen execution
    /// policy; it is immutable runtime state, never executor-visible.
    tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy,
    /// The one runtime-owned monotonic clock shared by publication, retry
    /// backoff, primary request deadlines, and summary request deadlines.
    monotonic_clock: Arc<dyn MonotonicClock>,
    /// The shared durability frontier (Issue #60): the same failed fact the
    /// coordinator commits as `DurabilityFailed`, carried to the
    /// conversation-owned registries so their new durable ownership commits
    /// linearize against it. Updated under the coordinator lock in
    /// [`RuntimeInner::record_durability_failure`]; the registries hold it
    /// across their ownership durable writes.
    durability_gate: Arc<DurabilityGate>,
    /// The immutable result of this runtime's startup recovery (Issue #12,
    /// M9a): the deterministic classification, exactly which recovery facts
    /// were committed, and what continuation is permitted.
    ///
    /// It is a *report*, never a second authority: the durable store remains
    /// the authority for what happened, and this value only records what this
    /// startup concluded from it.
    recovery: crate::runtime::recovery::RecoveryReport,
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
    /// The one shared semantic drain completion.
    drain: std::sync::OnceLock<Arc<DrainCompletion>>,
    /// Guards creation of the one drain task.
    drain_started: AtomicBool,
    /// The observation fan-out shared with the Runtime Client projection and
    /// any bounded local observation consumers; set exactly once when a
    /// projection consumer installs itself through
    /// [`RuntimeInner::install_observation_bridge`].
    pending: std::sync::OnceLock<Arc<ObservationFanout>>,
    /// Settlement signal: fired once per attempt settlement handoff, so
    /// headless drivers await the authoritative state transfer
    /// deterministically instead of by polling.
    settlement: tokio::sync::Notify,
    /// Test-only coordinator synchronization hooks.
    #[cfg(test)]
    probe: Mutex<Option<CoordinatorProbe>>,
    #[cfg(test)]
    subagent_transcript_store_reads: std::sync::atomic::AtomicUsize,
    /// Test-only one-shot pre-tool policy injection for a runtime-created
    /// attempt. Production constructs the required policy from the admitted
    /// effective `ApprovalMode`; this hook never changes the production
    /// configuration surface.
    #[cfg(test)]
    test_pre_tool_policy: Mutex<Option<Arc<dyn crate::agent::PreToolPolicy>>>,
}

/// Releasing the last semantic owner of a conversation runtime closes its
/// observation fan-out (if one was installed) and its admission wake gate.
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
    /// Fences ordinary runtime admission when a retired MCP generation proves
    /// that its physical terminal state is unproven. The logical resource
    /// publication already committed remains current; the existing drain
    /// lifecycle carries the failure to final settlement reporting. The
    /// failure latch and lifecycle admission closure are one coordinator
    /// linearization point.
    fn fence_mcp_settlement_failure(self: &Arc<Self>, detail: String) {
        let _ = self.begin_drain_internal(DrainTrigger::McpSettlementFailure(detail));
    }

    /// Acquires the one admission synchronization boundary.
    fn lock_state(&self) -> MutexGuard<'_, CoordinatorState> {
        self.state
            .lock()
            .expect("conversation runtime lock poisoned")
    }

    /// Begins or joins the one semantic runtime drain.
    ///
    /// The coordinator lock serializes the lifecycle transition with inbound
    /// acceptance, model updates, and attempt admission. The shared
    /// lifecycle admission guards serialize it with background ownership and
    /// capability commits that have their own native synchronization owner.
    fn idle_epoch_locked(&self, state: &CoordinatorState) -> Result<u64, IdleBusyReason> {
        use IdleBusyReason as Busy;
        let epoch = self.lifecycle.idle_epoch().ok_or(Busy::LifecycleOwner)?;
        if state.current_attempt.is_some() || state.manual_compaction.is_some() {
            return Err(Busy::Foreground);
        }
        if state.recovered_continuation.is_some() {
            return Err(Busy::Recovery);
        }
        if self.durability_gate.is_failed()
            || state.mcp_settlement_failure.is_some()
            || state.admission_durability_cycle.pending_retry.is_some()
        {
            return Err(Busy::Durability);
        }
        // Issue #351: residency follows real semantics. A composed Goal that
        // is durably Active with autonomous budget remaining still owns future
        // work, so evicting it would race its own eligible continuation.
        // Paused, Blocked, Complete, absent, an uncomposed extension, and an
        // Active Goal whose budget is exhausted all own nothing — an
        // exhausted Goal therefore cannot spin to pin residency forever.
        if self
            .goal_owns_future_work(state)
            .map_err(|_| Busy::Durability)?
        {
            return Err(Busy::AutonomousExtension);
        }
        if self.mailbox.has_pending().map_err(|_| Busy::Durability)? {
            return Err(Busy::Inbound);
        }
        if !self.interaction.pending_snapshot().is_empty() {
            return Err(Busy::Interaction);
        }
        if self.tool_runtime.background().owns_idle_work() {
            return Err(Busy::Background);
        }
        if self
            .subagents
            .as_ref()
            .is_some_and(|s| !s.unsettled_snapshot().is_empty())
        {
            return Err(Busy::Subagent);
        }
        if self.lifecycle.idle_epoch() != Some(epoch) {
            return Err(Busy::AdmissionChanged);
        }
        Ok(epoch)
    }

    /// Whether a composed Goal still owns future autonomous work.
    ///
    /// Both halves are required and neither is an activation bit: the current
    /// resource generation must actually compose the Goal extension (a
    /// Goal-disabled composition never executes stored Goal state), and the
    /// durable snapshot must still authorize continuation. This is the same
    /// answer the Goal-round admission decision uses, so residency and
    /// admission can never disagree.
    fn goal_owns_future_work(
        &self,
        state: &CoordinatorState,
    ) -> Result<bool, crate::durable::ConversationStoreError> {
        let Some(domain) = self.composed_goal(state) else {
            return Ok(false);
        };
        domain.authorizes_continuation()
    }

    /// The Goal domain of the current composition, or `None` when this
    /// runtime's frozen root profile does not compose the Goal extension.
    fn composed_goal<'a>(
        &'a self,
        state: &CoordinatorState,
    ) -> Option<&'a crate::goal::GoalDomain> {
        self.tool_runtime.goal().filter(|_| {
            state
                .resources
                .root_profile()
                .is_some_and(|profile| profile.extensions.goal().is_some())
        })
    }

    fn begin_drain(self: &Arc<Self>) -> Result<Arc<DrainCompletion>, ShutdownError> {
        self.begin_drain_internal(DrainTrigger::RuntimeShutdown)
    }

    /// Starts or joins the shared drain. For an MCP settlement failure, this
    /// operation owns the complete failure transition: it publishes the
    /// persistent diagnostic latch, arbitrates current-attempt cancellation,
    /// and closes lifecycle admission before releasing the coordinator lock.
    #[allow(clippy::too_many_lines)]
    fn begin_drain_internal(
        self: &Arc<Self>,
        trigger: DrainTrigger,
    ) -> Result<Arc<DrainCompletion>, ShutdownError> {
        let mut first = false;
        let mcp_failure = matches!(&trigger, DrainTrigger::McpSettlementFailure(_));
        // Runtime shutdown is only a cancellation contender. The active
        // attempt's AgentCancellation remains the one cause authority, so
        // every runtime-driven interaction settlement must use the winner it
        // records rather than blindly relabeling the work as RuntimeShutdown.
        let mut interaction_cancel_reason = CancellationReason::RuntimeShutdown;
        {
            let mut state = self.lock_state();
            let lifecycle_state = self.lifecycle.state();
            assert!(
                !(mcp_failure && lifecycle_state == ConversationLifecycleState::Quiescent),
                "an MCP physical-settlement failure arrived after Quiescent; the retirement ownership invariant is broken"
            );
            if let DrainTrigger::McpSettlementFailure(detail) = trigger
                && state.mcp_settlement_failure.is_none()
            {
                state.mcp_settlement_failure = Some(detail);
            }
            match lifecycle_state {
                ConversationLifecycleState::Inactive => {
                    if !mcp_failure {
                        return Err(ShutdownError::Inactive);
                    }
                    let transitioned = self.lifecycle.begin_failure_drain();
                    debug_assert!(
                        transitioned,
                        "the coordinator lock owns the inactive failure transition"
                    );
                    first = true;
                }
                ConversationLifecycleState::Running => {
                    if let Some(current) = &state.current_attempt {
                        // Take the same M9b model-turn start gate as user
                        // cancellation *before* publishing `Draining`. If a
                        // start critical section already owns the gate, it
                        // linearizes before runtime drain and is allowed to
                        // settle; if this request wins the gate, no later
                        // model arbitration can start a request.
                        let _ = current
                            .cancellation
                            .request_cancel(CancellationReason::RuntimeShutdown);
                        interaction_cancel_reason = current.cancellation.reason();
                    }
                    // Issue #351: runtime shutdown is not Goal pause. Drain
                    // closes runtime admission and supervises runtime-owned
                    // work; the durable `GoalPhase` is untouched, so a later
                    // explicit reopen resumes it through ordinary admission.
                    if let Some(compaction) = &state.manual_compaction {
                        let _ = compaction
                            .cancellation
                            .request_cancel(CancellationReason::RuntimeShutdown);
                    }
                    let transitioned = self.lifecycle.begin_drain();
                    debug_assert!(
                        transitioned,
                        "the coordinator lock owns the runtime drain transition"
                    );
                    first = true;
                    self.observe(ConversationObservation::Shutdown);
                    #[cfg(test)]
                    if let Some(linearized) = self
                        .probe
                        .lock()
                        .expect("coordinator probe lock poisoned")
                        .as_ref()
                        .and_then(|probe| probe.drain_linearization.clone())
                    {
                        linearized.notify_one();
                    }
                    self.wake.close();
                }
                ConversationLifecycleState::Draining => {
                    // An idle claim has already closed admission without starting
                    // external teardown under the manager registry lock.
                    first = !self.drain_started.load(Ordering::Acquire);
                    if first {
                        self.observe(ConversationObservation::Shutdown);
                    }
                    self.wake.close();
                }
                ConversationLifecycleState::Quiescent => {}
            }
            if first {
                // Reserve the existing supervisor under the same coordinator
                // boundary. A second shutdown cannot repeat external teardown.
                first = !self.drain_started.swap(true, Ordering::AcqRel);
            }
            // This test-only park is deliberately inside the coordinator
            // critical section. It proves that an MCP failure cannot expose
            // its latch and only later close lifecycle admission.
            #[cfg(test)]
            let mcp_failure_drain_gate = self
                .probe
                .lock()
                .expect("coordinator probe lock poisoned")
                .as_ref()
                .and_then(|probe| probe.mcp_failure_drain_gate.clone());
            #[cfg(test)]
            if mcp_failure
                && first
                && let Some(gate) = mcp_failure_drain_gate
            {
                gate.enter();
            }
        }

        let completion = self
            .drain
            .get_or_init(|| Arc::new(DrainCompletion::default()))
            .clone();
        if first {
            // Interaction publication is admitted through the same lifecycle
            // commit boundary as `Running -> Draining`, so this scan sees
            // every interaction that won the admission race. The pending
            // map may become empty before its waiter consumes the terminal
            // payload; the retained lifecycle guard keeps quiescence behind
            // that callback-authority settlement.
            // Cancellation intent is requested synchronously after the drain
            // transition wins, so a client-side background cancel cannot
            // create an unchecked post-drain ownership window. The registry
            // still owns the physical/native settlement and is awaited by the
            // drain task below.
            for execution in self.tool_runtime.background().active_snapshot() {
                self.tool_runtime.background().cancel_with_reason(
                    &execution.execution_id,
                    CancellationReason::RuntimeShutdown,
                );
            }
            self.tool_runtime.background().abort_prepared_for_drain();
            // Same containment for the subagent plane (Issue #60):
            // cancellation intent is committed synchronously; the registry
            // and its driver tasks own escalation, reap, and terminal
            // settlement, awaited by the drain task below.
            if let Some(subagents) = &self.subagents {
                subagents.cancel_all(CancellationReason::RuntimeShutdown);
            }
            // In-flight capability preparation owns real MCP processes. The
            // *owner* is cancelled here (never the caller's future), so each
            // one drives its physical process to settlement before releasing
            // the counted admission the drain below waits on.
            self.capability.cancel_conversation_preparation();
            // The published MCP generation is also retired at the drain
            // transition. Its explicit attempt/background leases, if any,
            // keep the physical runtime alive until those owners settle, but
            // the generation itself no longer blocks drain indefinitely.
            self.capability.retire_current_mcp_runtimes();
            let inner = Arc::clone(self);
            let completion_for_task = completion.clone();
            self.executor.spawn(async move {
                let result = inner
                    .drain_to_quiescence(completion_for_task.clone(), interaction_cancel_reason)
                    .await;
                // Release the drain task's native ownership before waking a
                // shutdown caller that may immediately relinquish the product.
                drop(inner);
                completion_for_task.complete(result);
            });
        }
        Ok(completion)
    }

    /// Waits for the current Agent Execution or manual compaction to return
    /// the checked-out conversation state. Cancellation alone never clears an
    /// ownership slot.
    async fn wait_for_foreground_operation(&self) {
        loop {
            let idle = {
                let state = self.lock_state();
                state.current_attempt.is_none() && state.manual_compaction.is_none()
            };
            if idle {
                return;
            }
            let notified = self.settlement.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let idle = {
                let state = self.lock_state();
                state.current_attempt.is_none() && state.manual_compaction.is_none()
            };
            if idle {
                return;
            }
            // The waiter is registered and the slot is still occupied: the
            // caller is now committed to awaiting this attempt's settlement.
            #[cfg(test)]
            self.signal_drain_supervision();
            notified.await;
        }
    }

    /// Supervises every runtime-owned operation to its strongest honest
    /// settlement, then publishes `Quiescent` only if nothing prevents it.
    ///
    /// # Failure is evidence, not a stop signal
    ///
    /// A settlement or durability failure is **collected**, never returned
    /// early. Returning at the first failure would release the supervisor
    /// from siblings — an active provider turn, another background
    /// execution, a retained MCP process — that are still externally capable
    /// of acting. The drain therefore runs the full supervision sequence
    /// (current attempt → background executions → counted subsystem
    /// admissions → admission worker → capability/MCP processes) and only
    /// afterwards decides between `Quiescent` and an aggregated settlement
    /// failure.
    ///
    /// Every waited-for owner has a *native terminal boundary*: a background
    /// record settles terminally or explicitly abandons its bounded durable
    /// publication (its runner has returned either way), an MCP runtime
    /// closes and proves or disproves physical settlement, and a counted
    /// admission is released by its owner. No wait here depends on a global
    /// health flag, so one owner's failure can never be mistaken for
    /// another's settlement.
    async fn drain_to_quiescence(
        self: &Arc<Self>,
        _completion: Arc<DrainCompletion>,
        interaction_cancel_reason: CancellationReason,
    ) -> Result<(), ShutdownError> {
        let mut failures: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        // This is the async reliable route boundary for child-owned
        // interactions. It runs after `Running -> Draining` has closed new
        // admission, but before the drain waits for foreground ownership, so
        // every child settlement can reach the parent control lane without a
        // lossy synchronous try-send.
        self.interaction
            .cancel_pending_async(interaction_cancel_reason)
            .await;
        loop {
            self.wait_for_foreground_operation().await;

            // The subagent plane (Issue #60): cancellation intent is already
            // committed (begin_drain); here the drain awaits every child's
            // native terminal boundary — the driver task has reaped the
            // process and the registry has settled its durable terminal or
            // explicitly abandoned the publication.
            if let Some(subagents) = &self.subagents {
                let active = subagents.unsettled_snapshot();
                if !active.is_empty() {
                    for subagent in &active {
                        let _ = subagents
                            .cancel(&subagent.subagent_id, CancellationReason::RuntimeShutdown);
                    }
                    for subagent in active {
                        subagents.wait_until_settled(&subagent.subagent_id).await;
                    }
                    continue;
                }
            }

            // Records whose durable terminal publication was abandoned are
            // excluded: their runners have returned, so cancelling and
            // awaiting them again would spin. They remain explicit evidence
            // below.
            let active = self.tool_runtime.background().unsettled_snapshot();
            if !active.is_empty() {
                for execution in &active {
                    self.tool_runtime.background().cancel_with_reason(
                        &execution.execution_id,
                        CancellationReason::RuntimeShutdown,
                    );
                }
                for execution in active {
                    #[cfg(test)]
                    self.signal_drain_supervision();
                    self.tool_runtime
                        .background()
                        .wait_until_settled(&execution.execution_id)
                        .await;
                }
                continue;
            }

            self.lifecycle.wait_for_no_admissions().await;
            self.tool_runtime.background().abort_prepared_for_drain();
            if !self
                .tool_runtime
                .background()
                .unsettled_snapshot()
                .is_empty()
            {
                continue;
            }
            if self.worker_started.load(Ordering::Acquire) {
                self.wake.wait_until_exited().await;
            }
            self.lifecycle.wait_for_no_admissions().await;
            if let Err(details) = self.capability.drain_conversation_owned().await {
                failures.extend(details);
            }
            self.lifecycle.wait_for_no_admissions().await;

            // Supervision has reached every owner's native terminal
            // boundary. Only now is the runtime allowed to decide.
            for execution_id in self.tool_runtime.background().abandoned_publications() {
                failures.insert(format!(
                    "background execution {execution_id}: the durable terminal publication is unresolved"
                ));
            }
            if let Some(subagents) = &self.subagents {
                for snapshot in subagents.abandoned_publications() {
                    failures.insert(format!(
                        "subagent {}: the durable terminal publication is unresolved",
                        snapshot.subagent_id
                    ));
                }
            }
            if let Some(subagents) = &self.subagents {
                for activation_id in subagents.unproven_settlements() {
                    failures.insert(format!(
                        "subagent {activation_id}: physical settlement is unresolved"
                    ));
                }
            }
            if let Some(detail) = self.durability_failure_diagnostic() {
                failures.insert(format!("durable authority: {detail}"));
            }
            if let Some(detail) = self.lock_state().mcp_settlement_failure.clone() {
                failures.insert(detail);
            }
            if !failures.is_empty() {
                return Err(ShutdownError::RuntimeOwnedSettlement {
                    detail: aggregate_settlement_failures(&failures),
                });
            }
            if self.lifecycle.mark_quiescent() {
                return Ok(());
            }
        }
    }

    /// Test-only: announces that the drain task has begun supervising
    /// runtime-owned owners.
    #[cfg(test)]
    fn signal_drain_supervision(&self) {
        if let Some(signal) = self
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.drain_supervision.clone())
        {
            signal.notify_one();
        }
    }

    /// Takes the one-shot model-arbitration pause for the next attempt.
    #[cfg(test)]
    fn take_model_arbitration_pause(
        &self,
    ) -> Option<crate::agent::execution::test_sync::ModelArbitrationPause> {
        self.probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_mut()
            .and_then(|probe| probe.model_arbitration_pause.take())
    }

    /// Returns the durable failure that prevents this runtime from claiming
    /// successful quiescence, if one has been recorded.
    fn durability_failure_diagnostic(&self) -> Option<String> {
        // The one authoritative absorbing failure fact lives in the
        // runtime-owned DurabilityGate; the coordinator keeps no second
        // failed-state copy.
        self.durability_gate
            .failure()
            .map(|failure| failure.diagnostic)
    }

    fn context_runtime_with_assembly(
        &self,
        model: &AttemptModelSnapshot,
        assembly: crate::context::ContextAssembly,
        model_timeout_policy: ModelTimeoutPolicy,
        resources: &RuntimeResourceSnapshot,
    ) -> Result<ContextRuntime, crate::context::ContextError> {
        let configuration = resources.configuration();
        let policy = configuration.map_or(self.context.policy, |generation| {
            generation.config.context_policy()
        });
        let status_engine = match configuration {
            Some(generation) => generation
                .config
                .extension_composition()
                .agent_status_engine(Arc::new(crate::context::SystemClock)),
            None => self
                .context
                .status_engine
                .as_ref()
                .map(AgentStatusEngine::for_attempt),
        };
        let mut context = ContextRuntime::for_attempt_with_assembly(
            policy,
            Arc::clone(&self.context.estimator),
            status_engine,
            assembly,
            model,
            model_timeout_policy,
            Arc::clone(&self.monotonic_clock),
        )?;
        if let Some(owner) = &self.tool_runtime.uploads {
            context.engine.set_upload_resolver(owner.clone());
        }
        Ok(context)
    }

    /// Publishes one semantic observation into the shared observation
    /// fan-out, when a projection consumer exists and the fan-out is open.
    ///
    /// This is a leaf publication: the runtime keeps no second fold of the
    /// observation vocabulary. Because the queue is installed while the
    /// runtime is still inactive (see
    /// [`RuntimeInner::install_observation_bridge`]) and an inactive
    /// runtime publishes nothing, an installed consumer observes every
    /// observation this runtime ever emits.
    #[allow(clippy::needless_pass_by_value)] // observer callers construct owned observations
    fn observe(&self, observation: ConversationObservation) {
        if let Some(pending) = self.pending.get() {
            pending.push(&observation);
        }
    }

    /// Records a durable-authority failure without silently swallowing it
    /// (Issue #63, Finding 5).
    ///
    /// A transient storage failure ([`DurableOperation::is_transient`])
    /// consumes the allowance for that stage in the current finite admission
    /// cycle, publishes a [`ConversationObservation::DurableFailure`], and
    /// arms exactly one bounded re-kick. A second failure of that stage in
    /// the same cycle — or any non-transient failure — upgrades to the
    /// absorbing `DurabilityFailed` fact, committed through the one
    /// authority ([`DurabilityGate::commit_failure`]) and published as a
    /// [`ConversationObservation::DurabilityFailed`] exactly once; no
    /// further re-kick is armed, so a persistent or alternating fault cannot
    /// become a hot loop. The observation is published only when the commit
    /// reports that THIS call installed the absorbing fact, and it carries
    /// the authoritative operation and diagnostic returned by the commit
    /// (see [`Self::commit_failure_and_observe`]) — a later caller can
    /// never publish a second observation. A failure of a different
    /// transient stage consumes its own allowance without erasing the first
    /// stage's debt.
    fn record_durability_failure(
        &self,
        state: &mut CoordinatorState,
        operation: DurableOperation,
        diagnostic: String,
    ) {
        // Absorbing winner: the absorbing commit itself reports whether this
        // call installed the fact, so a caller-side guard is only needed
        // where retry bookkeeping must not run on an already-failed runtime.
        if !operation.is_transient() {
            self.commit_failure_and_observe(operation, diagnostic);
            return;
        }
        // The transient path arms coordinator-owned retry bookkeeping; that
        // bookkeeping must never run once the absorbing fact exists, so the
        // failed-gate check stays ahead of the admission cycle.
        if self.durability_gate.is_failed() {
            return;
        }
        let retry_armed = {
            let cycle = &mut state.admission_durability_cycle;
            if cycle.budget.try_consume(operation) {
                cycle.pending_retry = Some(PendingDurabilityRetry {
                    operation,
                    diagnostic: diagnostic.clone(),
                });
                true
            } else {
                false
            }
        };
        if retry_armed {
            self.observe(ConversationObservation::DurableFailure {
                message: diagnostic,
            });
            self.wake.notify.notify_one();
        } else {
            self.commit_failure_and_observe(operation, diagnostic);
        }
    }

    /// Commits the absorbing `DurabilityFailed` fact through the one
    /// authority and publishes the single
    /// [`ConversationObservation::DurabilityFailed`] only when THIS call
    /// installed the fact; the observation's operation and diagnostic are
    /// the authoritative ones returned by the commit.
    fn commit_failure_and_observe(&self, operation: DurableOperation, diagnostic: String) {
        let outcome = self.durability_gate.commit_failure(operation, diagnostic);
        if let DurabilityFailureCommit::Committed(failure) = outcome {
            self.observe(ConversationObservation::DurabilityFailed {
                operation: failure.operation.as_str().to_owned(),
                diagnostic: failure.diagnostic,
            });
        }
    }

    /// Records progress through one durable stage. This clears only the
    /// matching pending re-kick marker; it never resets the admission-cycle
    /// budget. A stage success is not a semantic completion boundary.
    fn record_durability_success(state: &mut CoordinatorState, operation: DurableOperation) {
        if let Some(pending_retry) = &mut state.admission_durability_cycle.pending_retry
            && pending_retry.operation == operation
        {
            state.admission_durability_cycle.pending_retry = None;
        }
    }

    /// Completes the current finite admission cycle and starts a fresh one.
    ///
    /// This is intentionally called only after selection proves there is no
    /// pending work or after the selected batch is durably adopted. Success
    /// of an intermediate select/adopt stage must retain the consumed bits.
    fn complete_admission_cycle(state: &mut CoordinatorState) {
        state.admission_durability_cycle.budget = AdmissionRetryBudget::default();
        state.admission_durability_cycle.pending_retry = None;
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
    /// observation bridge already exists, or [`RuntimeBootstrapError::Durable`]
    /// when the native durable bootstrap cannot be read coherently.
    #[allow(clippy::too_many_lines)] // One inactive-runtime bootstrap synchronization boundary.
    fn install_observation_bridge(
        self: &Arc<Self>,
        queue: Arc<PendingObservations>,
        client_projection: bool,
    ) -> Result<RuntimeBootstrapSnapshot, RuntimeBootstrapError> {
        // The one coordinator lock is held across every phase below: it is
        // the freeze that makes the combined seed one real global state.
        let state = self.lock_state();
        // Binding a Runtime Client host is a pre-activation composition
        // decision. Rejecting here, under the lock `activate` also takes
        // for its lifecycle transition, is what makes the host-binding
        // decision race atomically against activation: a bootstrap that
        // acquires the lock first sees `Inactive` and completes before
        // activation, one that acquires it after sees `Running` and is
        // refused.
        if self.lifecycle.is_activated() {
            return Err(RuntimeBootstrapError::RuntimeAlreadyActivated {
                conversation_id: self.conversation_id.clone(),
            });
        }
        if self.pending.get().is_some() {
            return Err(RuntimeBootstrapError::BridgeAlreadyInstalled {
                conversation_id: self.conversation_id.clone(),
            });
        }
        // Read the native durable bootstrap before installing any observer
        // seam. A corrupt/unavailable Pending Inbound or Surface authority
        // must fail explicitly and leave the runtime unbridged.
        let head = self
            .store
            .load_head()
            .map_err(|error| RuntimeBootstrapError::Durable(error.to_string()))?;
        let messages = self
            .store
            .load_messages(&head.active_message_ids)
            .map_err(|error| RuntimeBootstrapError::Durable(error.to_string()))?;
        let transcript = self
            .store
            .load_transcript_page(None, TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT)
            .map_err(|error| RuntimeBootstrapError::Durable(error.to_string()))?;
        // ---- T0: the coordinator-owned facts ----
        //
        // An inactive runtime never moved its conversation state into an
        // attempt. Bootstrap only hydrates the current Surface working set;
        // the append-only Ledger remains a paged durable read authority.
        // The observation bridge is installed only before activation, so its
        // shutdown projection seed is necessarily false.
        let shutting_down = false;
        let model = state.model.view();
        // The resource generation is part of the same freeze as every other
        // seeded fact: a client attaching before the first configuration application still sees
        // exactly which project instruction files the runtime loaded.
        let resources = Arc::clone(&state.resources);
        let approval_mode = state.effective_approval_mode;
        let observer: Arc<RuntimeObserver> = Arc::new(RuntimeObserver::new(self));
        // Interaction pending state is an ephemeral runtime observation, but
        // it still participates in the same bootstrap cut as every other
        // live projection fact. Installing the observer here means all later
        // pending/settled transitions enter the one observation queue.
        let mut pending_interactions: Vec<RoutedInteraction> = self
            .interaction
            .pending_snapshot()
            .into_iter()
            .map(RoutedInteraction::primary)
            .collect();
        let child_pending_interactions = self
            .subagents
            .as_ref()
            .map(crate::runtime::subagent::SubagentRegistry::pending_interaction_projection)
            .unwrap_or_default();
        // The list is not a live subsystem with an observer of its own: it
        // is a derivation of canonical tool results, and it moves only when
        // one of those commits. Reading the committed list inside the same
        // freeze keeps the seed and the live stream on one cut.
        let todos = self.tool_runtime.todo_snapshot();
        let goal = self
            .tool_runtime
            .goal()
            .map(crate::goal::GoalDomain::view)
            .transpose()
            .map_err(|error| RuntimeBootstrapError::Durable(error.to_string()))?;
        let journal_through = if client_projection {
            let through = self
                .store
                .observe_journal(queue.clone())
                .map_err(|error| RuntimeBootstrapError::Durable(error.to_string()))?;
            let workflow_observer = observer.clone();
            self.tool_runtime
                .workflows()
                .install_publication_observer(Arc::new(move |snapshot, sequence| {
                    workflow_observer.push(ConversationObservation::Published {
                        journal_sequence: sequence,
                        observation: Box::new(ConversationObservation::Workflow(snapshot)),
                    });
                }));
            through
        } else {
            0
        };
        self.interaction.install_observer(observer.clone());
        // ---- T1: the mailbox (frozen: an inactive conversation refuses
        //          inbound) ----
        let inbound_pending = self
            .mailbox
            .install_observer_and_pending(observer.clone())
            .map_err(|error| RuntimeBootstrapError::Durable(error.to_string()))?;
        if let Some(domain) = self.tool_runtime.goal() {
            domain.install_observer(observer.clone());
        }
        // ---- T2: the background registry (frozen: the registry refuses
        //          commits while its mailbox is bound inactive) ----
        let background = self
            .tool_runtime
            .background()
            .install_observer_and_snapshots(observer.clone());
        // ---- T2b: the subagent registry (frozen by the same inactive
        //           mailbox binding) ----
        let agents = self
            .subagents
            .as_ref()
            .map(|subagents| subagents.install_observer_and_agent_snapshots(observer.clone()))
            .unwrap_or_default();
        pending_interactions.extend(child_pending_interactions);
        pending_interactions.sort_by(|left, right| left.interaction.cmp(&right.interaction));
        // No fallible subsystem has been mutated after the pending read. The
        // pre-check above and the coordinator lock make this set infallible;
        // keep the explicit branch as a defensive invariant assertion.
        if self
            .pending
            .set(Arc::new(ObservationFanout::new(queue)))
            .is_err()
        {
            return Err(RuntimeBootstrapError::BridgeAlreadyInstalled {
                conversation_id: self.conversation_id.clone(),
            });
        }
        // ---- R: the capability coordinator, the cut itself ----
        let (capabilities, capability_availability) =
            self.capability.install_observer_and_snapshot(observer);
        drop(state);
        Ok(RuntimeBootstrapSnapshot {
            journal_through,
            conversation_id: self.conversation_id.clone(),
            shutting_down,
            messages,
            transcript,
            model,
            approval_mode,
            inbound_pending,
            background,
            agents,
            pending_interactions,
            todos,
            goal,
            capabilities,
            capability_availability,
            resources,
        })
    }

    /// Adds one bounded local consumer to the already-installed observation
    /// stream before activation.
    ///
    /// This is intentionally not another Runtime Client bootstrap path. The
    /// Runtime Client host owns the primary queue and its coherent seed; the
    /// returned queue is for an existing local observation surface that needs
    /// the same post-bootstrap semantic stream. Requiring the runtime to be
    /// inactive makes the subscription linearize before the activation cut,
    /// so it cannot miss a live observation and does not need a second seed.
    fn subscribe_observations(
        &self,
    ) -> Result<Arc<PendingObservations>, RuntimeObservationSubscriptionError> {
        let _state = self.lock_state();
        if self.lifecycle.is_activated() {
            return Err(
                RuntimeObservationSubscriptionError::RuntimeAlreadyActivated {
                    conversation_id: self.conversation_id.clone(),
                },
            );
        }
        let Some(fanout) = self.pending.get() else {
            return Err(RuntimeObservationSubscriptionError::BridgeNotInstalled {
                conversation_id: self.conversation_id.clone(),
            });
        };
        Ok(fanout.subscribe())
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
            wake.mark_exited();
            #[cfg(test)]
            wake.signal_worker_exit();
        });
    }

    /// Runs one attempt to settlement against the coordinator-owned
    /// cancellation trigger (the same handle `cancel_current_attempt`
    /// requests cancellation on).
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)] // one ordered ownership transaction
    async fn run_attempt(
        self: &Arc<Self>,
        attempt_id: AttemptId,
        conversation: ConversationState,
        fresh: Option<FreshInboundTurn>,
        cancellation: &AgentCancellation,
        model: AttemptModelSnapshot,
        model_timeout_policy: ModelTimeoutPolicy,
        tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy,
        approval_mode: ApprovalMode,
        resources: Arc<RuntimeResourceSnapshot>,
        model_config: crate::model::session::SessionModelConfig,
        model_registry: Option<crate::model::invocation::ModelBindingRegistry>,
        lease: crate::capabilities::AttemptCapabilityLease,
    ) -> crate::agent::AgentExecutionResult {
        let observer = RuntimeObserver::new(self);
        let monotonic_clock = Arc::clone(&self.monotonic_clock);
        // The context runtime is derived from the frozen snapshot, so the
        // attempt's window, output budget, and summary invocation all agree
        // with the model it was admitted with.
        let context_runtime = self
            .context_runtime_with_assembly(
                &model,
                resources.context_assembly().clone(),
                model_timeout_policy,
                &resources,
            )
            .expect("admission validated this model against the session context policy");
        let context_runtime = context_runtime.with_runtime_resources(&resources);
        let execution_policy = crate::agent::execution::AgentExecutionRuntimePolicy {
            model_timeout_policy,
            tool_deadline_policy,
            monotonic_clock,
            // The attempt-scoped subagent seam. It is derived from the very
            // generation and model configuration this attempt was admitted
            // with, under the same admission linearization, so a subagent
            // invoked by this attempt resolves exactly that generation — not
            // whatever generation happens to be runtime-current when the
            // model issues the call.
            // A runtime whose model authority is frozen (a subagent child)
            // owns neither a subagent registry nor a catalog to resolve a
            // named definition's explicit model against, so the seam is
            // absent on both counts rather than half-present.
            subagent_context: self
                .subagents
                .is_some()
                .then_some(model_registry)
                .flatten()
                .map(|models| {
                    crate::runtime::subagent::AttemptSubagentContext::new(
                        self.capability.clone(),
                        attempt_id.clone(),
                        Arc::clone(&resources),
                        model_config,
                        models,
                        approval_mode,
                        crate::runtime::subagent::InheritedExecutionPolicy {
                            model_timeout: model_timeout_policy,
                            tool_deadline: tool_deadline_policy,
                            context: resources
                                .configuration()
                                .map_or(self.context.policy, |generation| {
                                    generation.config.context_policy()
                                }),
                        },
                    )
                }),
            workflow_output: self.workflow_output.clone(),
        };
        let request = AgentExecutionRequest {
            agent_id: self.agent_id.clone(),
            conversation_id: self.conversation_id.clone(),
            attempt_id,
            conversation,
            initial_turn_trigger: match fresh {
                Some(fresh) => InitialTurnTrigger::FreshInbound(fresh),
                None => InitialTurnTrigger::Continuation,
            },
            model,
        };
        let lifecycle = crate::agent::AttemptLifecycle::inert()
            .with_native_interaction(self.interaction.clone())
            .with_approval_mode(approval_mode);
        #[cfg(test)]
        let lifecycle = {
            let test_policy = self
                .test_pre_tool_policy
                .lock()
                .expect("test pre-tool policy lock")
                .take();
            match test_policy {
                Some(policy) => lifecycle.with_pre_tool_policy(policy),
                None => lifecycle,
            }
        };
        let mut execution = AgentExecution::new(
            request,
            lease,
            cancellation,
            execution_policy,
            context_runtime,
            &self.tool_runtime,
            lifecycle,
        )
        // Neither rejection is reachable: `conversation_id` *is* the tool
        // runtime's own identity (the runtime has no independent
        // conversation authority to disagree with it), and construction
        // validated the coordinator against that same runtime.
        .expect("the conversation runtime derives its identity from this tool runtime");
        // Test-only: hand the armed M9b start-boundary pause to exactly this
        // attempt's execution.
        #[cfg(test)]
        {
            let pause = self
                .probe
                .lock()
                .expect("coordinator probe lock poisoned")
                .as_mut()
                .and_then(|probe| probe.start_boundary_pause.take());
            if let Some(pause) = pause {
                execution.install_start_boundary_pause(pause);
            }
            if let Some(pause) = self.take_model_arbitration_pause() {
                execution.install_model_arbitration_pause(pause);
            }
            let pause = self
                .probe
                .lock()
                .expect("coordinator probe lock poisoned")
                .as_mut()
                .and_then(|probe| probe.tool_start_pause.take());
            if let Some(pause) = pause {
                execution.install_tool_start_pause(pause);
            }
        }
        execution.observe(&observer);
        execution.run().await
    }

    /// Runs one manual compaction over the conversation state checked out by
    /// the coordinator. This is runtime-owned work: the task continues to
    /// settlement even if the requesting attachment disappears.
    async fn run_manual_compaction(
        self: &Arc<Self>,
        mut conversation: ConversationState,
        context_runtime: ContextRuntime,
        tools: Vec<ModelToolDefinition>,
        system_sections: Vec<crate::context::AcceptedSystemSection>,
        cancellation: &AgentCancellation,
    ) -> ManualCompactionTaskResult {
        let effective_system_prompt = render_effective_system_prompt(&system_sections);
        let signal = cancellation.signal();
        let executed = execute_compaction(
            &mut conversation,
            &context_runtime,
            &self.conversation_id,
            self.store.as_ref(),
            &tools,
            None,
            &effective_system_prompt,
            &CompactionConstraints::default(),
            &signal,
            CompactionAttribution::default(),
        )
        .await;
        let result = match executed {
            Ok(completed) => {
                let RuntimeEvent::CompactionCompleted {
                    generation,
                    summary_message_id,
                    surface_revision,
                    tokens_before,
                    estimated_tokens_after,
                } = &completed.persisted_event.event
                else {
                    return ManualCompactionTaskResult {
                        conversation,
                        result: Err(ManualCompactionError::Durable {
                            message: "the durable compaction returned a non-compaction event"
                                .to_owned(),
                        }),
                    };
                };
                let outcome = ManualCompactionOutcome {
                    generation: *generation,
                    summary_message_id: summary_message_id.clone(),
                    surface_revision: *surface_revision,
                    tokens_before: *tokens_before,
                    estimated_tokens_after: *estimated_tokens_after,
                };
                Ok(ManualCompactionSuccess { outcome, completed })
            }
            Err(CompactionExecutionError::Context(error)) => {
                Err(ManualCompactionError::Context(error))
            }
            Err(CompactionExecutionError::Durable(message)) => {
                Err(ManualCompactionError::Durable { message })
            }
        };
        ManualCompactionTaskResult {
            conversation,
            result,
        }
    }

    /// Restores the sole conversation state, records any durable failure,
    /// clears the maintenance slot, and hands pending inbound back to the one
    /// admission path.
    fn finish_manual_compaction(
        self: &Arc<Self>,
        task: ManualCompactionTaskResult,
        completion: &ManualCompactionCompletion,
    ) {
        let ManualCompactionTaskResult {
            conversation,
            result,
        } = task;
        let completion_result;
        {
            let mut state = self.lock_state();
            state.conversation = Some(conversation);
            state
                .manual_compaction
                .take()
                .expect("manual compaction settlement owns the maintenance slot");
            if let Err(ManualCompactionError::Durable { message }) = &result {
                self.record_durability_failure(
                    &mut state,
                    DurableOperation::CanonicalCommit,
                    format!(
                        "the manual compaction transition cannot be committed durably: {message}"
                    ),
                );
            }
            completion_result = match result {
                Ok(success) => {
                    // Durable commit already happened in the task, but the
                    // live completion is published only after coordinator
                    // ownership is restored and the maintenance slot is
                    // clear. Neither observation has an attempt identity.
                    self.observe(ConversationObservation::Committed {
                        attempt_id: None,
                        block: success.completed.summary_block,
                        transcript_cursor: Some(success.completed.transcript_cursor),
                    });
                    self.observe(ConversationObservation::Published {
                        journal_sequence: success.completed.persisted_event.sequence,
                        observation: Box::new(ConversationObservation::ManualCompactionEvent {
                            event: success.completed.persisted_event.event,
                        }),
                    });
                    Ok(success.outcome)
                }
                Err(error) => {
                    self.observe(ConversationObservation::ManualCompactionEvent {
                        event: RuntimeEvent::CompactionFailed {
                            error: error.to_string(),
                        },
                    });
                    Err(error)
                }
            };
        }
        self.settlement.notify_waiters();
        completion.complete(completion_result);
        self.admit_next_attempt();
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
            // An active-attempt durable canonical-write failure means the
            // durable authority rejected a required commit while the
            // conversation's single mutable state was checked out: the
            // runtime must not silently return to a false `Healthy` state
            // and admit future work as though storage were fine (Issue
            // #63). This is an already-terminal durable failure, so it
            // enters the explicit `DurabilityFailed` state immediately;
            // the settled attempt's conversation state is still restored
            // (its in-memory content stayed consistent with the durable
            // Ledger: the failed commit installed nothing).
            let durable_failure = result.durable_failure.clone();
            state.conversation = Some(result.conversation);
            if let Some(diagnostic) = durable_failure {
                let operation = match result.durable_failure_kind {
                    Some(crate::agent::DurableFailureKind::Compaction) => {
                        DurableOperation::CanonicalCommit
                    }
                    Some(crate::agent::DurableFailureKind::RequestStart) => {
                        DurableOperation::RequestStart
                    }
                    Some(crate::agent::DurableFailureKind::EventJournal) => {
                        DurableOperation::EventJournal
                    }
                    _ => DurableOperation::CanonicalCommit,
                };
                self.record_durability_failure(&mut state, operation, diagnostic);
            }
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
            // parks only when armed and disarms after one park. The gate
            // handle is extracted before the park so the probe mutex is not
            // held while parked.
            #[cfg(test)]
            let settlement_gate = self
                .probe
                .lock()
                .expect("coordinator probe lock poisoned")
                .as_ref()
                .and_then(|probe| probe.settlement_gate.clone());
            #[cfg(test)]
            if let Some(gate) = settlement_gate {
                gate.enter();
            }
        }
        // Test-only gate: the coordinator lock is released and the
        // current-attempt slot is already clear, but this task has not run
        // its final admission callback and has not returned, so it still
        // holds the attempt-task admission. A drain that observed the empty
        // slot must not be able to publish quiescence here.
        #[cfg(test)]
        let attempt_exit_gate = self
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.attempt_exit_gate.clone());
        #[cfg(test)]
        if let Some(gate) = attempt_exit_gate {
            gate.enter();
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
    #[allow(clippy::too_many_lines)]
    fn admit_next_attempt(self: &Arc<Self>) {
        // Test-only gate: parks before the coordinator lock, so a competing
        // publish can still enqueue while the admission is gated. The gate
        // parks only when armed and disarms after one park. The gate handle
        // is extracted before the park so the probe mutex is not held while
        // parked (a parked admission must never block a submit that holds the
        // coordinator lock and only briefly probes the gate).
        #[cfg(test)]
        let admission_gate = self
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.admission_gate.clone());
        #[cfg(test)]
        if let Some(gate) = admission_gate {
            gate.enter();
        }
        let mut state = self.lock_state();
        if !self.lifecycle.is_running()
            || state.current_attempt.is_some()
            || state.manual_compaction.is_some()
            || state
                .parent_turn_permits
                .is_some_and(|permits| state.admitted_attempts >= permits)
        {
            return;
        }
        // Persistent durable failure: no new admission may begin. The one
        // authoritative failure fact lives in the runtime-owned DurabilityGate;
        // this is not a hot loop because the fact is absorbing and the worker
        // is not re-kicked once it exists.
        if self.durability_gate.is_failed() {
            return;
        }
        // Live admission guard: the coordinator may only adopt inbound when
        // the active conversation is at a safe boundary (no incomplete tool
        // call without its committed ToolResult sibling). This closes the
        // window a failed tool-result batch would otherwise leave open.
        {
            let conversation = state
                .conversation
                .as_ref()
                .expect("the coordinator owns the conversation state while idle");
            if let Ok(active) = conversation.active_messages()
                && let Some(tool_call_id) = crate::conversation::pending_tool_call(&active)
            {
                // A semantic contract failure, not a transient storage
                // failure: the broken boundary cannot heal by retrying the
                // identical admission, so it fails closed immediately.
                self.record_durability_failure(
                    &mut state,
                    DurableOperation::IncompleteToolTurn,
                    format!(
                        "the active conversation ends inside an incomplete tool turn: tool call {tool_call_id} has no committed ToolResult"
                    ),
                );
                return;
            }
        }
        // Selection freezes the finite watermark (non-destructive): an
        // acceptance that linearizes after this point can never join the
        // selected batch. A storage failure here is never silently
        // swallowed (Finding 5): it is recorded, observed, and re-kicked
        // exactly once.
        let batch = match self.mailbox.select_pending_batch() {
            Ok(Some(batch)) => {
                Self::record_durability_success(&mut state, DurableOperation::SelectPendingBatch);
                // A fresh inbound batch subsumes the recovered continuation:
                // the new attempt sees the already-canonical unanswered turn
                // in its context anyway, so the one-shot permission is
                // consumed here rather than starting a second attempt later.
                batch
            }
            Ok(None) => {
                Self::record_durability_success(&mut state, DurableOperation::SelectPendingBatch);
                // No pending work is a semantic completion boundary for the
                // finite admission cycle. Only here, or after successful
                // batch adoption below, is the retry budget reset.
                Self::complete_admission_cycle(&mut state);
                // Issue #12 (M9a), phase 4 — resume only proven-safe work.
                // Startup recovery classified the crash as "admitted, no
                // external side effect ever crossed a start commit", so the
                // already-canonical user turn may continue through one **new**
                // attempt. Nothing is re-adopted and no `UserMessage` is
                // duplicated: the turn is already in the Ledger and on the
                // Surface, and the attempt runs as an explicit
                // `InitialTurnTrigger::Continuation`.
                //
                // A Class C conversation never reaches this branch: an
                // indeterminate external outcome leaves the permission unset,
                // so recovery starts nothing at all.
                if state.recovered_continuation.is_some() {
                    self.admit_continuation(state);
                    return;
                }
                // The one autonomous continuation decision (Issue #351).
                // Eligibility here is: this is the ordinary safe idle
                // admission boundary, no pending inbound won it, the Goal
                // extension is composed, no owned background or Subagent work
                // is outstanding, and the durable phase plus remaining budget
                // authorize another round. There is no activation precondition.
                if let Some(goal) = self.composed_goal(&state) {
                    let admission = self
                        .tool_runtime
                        .background()
                        .with_goal_idle(|| {
                            let request = || {
                                crate::goal::GoalRoundDriver::request(
                                    goal,
                                    &self.mailbox,
                                    self.clock.now(),
                                )
                            };
                            match &self.subagents {
                                Some(registry) => registry.with_goal_idle(request),
                                None => Some(request()),
                            }
                        })
                        .flatten();
                    if let Some(Err(error)) = admission {
                        // Issue #351: a Goal-round durability failure is a
                        // runtime-health fact, not a Goal intent change. The
                        // absorbing durability fence closes further admission
                        // — which is why this cannot become a hot retry loop —
                        // while the durable Goal phase stays truthful.
                        self.record_durability_failure(
                            &mut state,
                            DurableOperation::GoalRoundAdmission,
                            error.to_string(),
                        );
                    }
                }
                return;
            }
            Err(error) => {
                self.record_durability_failure(
                    &mut state,
                    DurableOperation::SelectPendingBatch,
                    error.to_string(),
                );
                return;
            }
        };
        // Prepare the canonical transition **before** the durable adoption
        // commit: validate every fallible in-memory condition now, so the
        // post-commit installation is infallible (Finding 2). These checks
        // validate stable identities; committed receipts supply the content. On a validation failure
        // nothing is durably adopted and the items remain pending.
        {
            let conversation = state
                .conversation
                .as_ref()
                .expect("the coordinator owns the conversation state while idle");
            for item in batch.items() {
                let block = crate::durable::inbox::canonical_block(item.message());
                match conversation.prepare_commit(&block) {
                    Ok(_) => {}
                    Err(error) => {
                        // A semantic contract failure (the durable pending
                        // item conflicts with canonical memory), not a
                        // transient storage failure: it fails closed
                        // immediately instead of consuming a storage retry.
                        self.record_durability_failure(
                            &mut state,
                            DurableOperation::PrepareAdoption,
                            format!(
                                "a pending inbound item cannot be prepared for canonical adoption: {error}"
                            ),
                        );
                        return;
                    }
                }
            }
        }
        let adopted = match self.mailbox.adopt_pending_batch(&batch, None) {
            Ok(adopted) => adopted,
            Err(error) => {
                self.record_durability_failure(
                    &mut state,
                    DurableOperation::AdoptPendingBatch,
                    error.to_string(),
                );
                return;
            }
        };
        let adoption = adopted.adoption;
        let adopted = adopted.items;
        if adopted.is_empty() {
            Self::complete_admission_cycle(&mut state);
            self.mailbox.wake().notify_one();
            return;
        }
        state.recovered_continuation = None;
        // Mutation preserves identity. Every committed row is a subset of the
        // prevalidated identities; only the committed receipt supplies content.
        let conversation = state.conversation.as_ref().expect("idle conversation");
        let prepared_commits: Vec<_> = adopted
            .iter()
            .map(|item| {
                conversation
                    .prepare_commit(&crate::durable::inbox::canonical_block(item.message()))
                    .expect("adopted identity was validated under exclusive ownership")
            })
            .collect();
        let fresh = FreshInboundTurn::new(
            adopted
                .iter()
                .map(|item| item.message().id.clone())
                .collect(),
        )
        .expect("nonempty ordered committed identities");
        // Issue #351: admission provenance is derived here, from the durable
        // inbound this coordinator actually adopted — the only place that
        // knows it. A Goal continuation is accepted into an empty pending
        // queue at the Goal-round frontier, so a batch carrying one is
        // pursuing that committed round even if ordinary inbound joined it
        // before adoption.
        let provenance = adopted
            .iter()
            .find_map(|item| {
                if let InboundKind::GoalContinuation(reference) = &item.message().kind {
                    // Atomic acceptance advanced this exact authority once. Never
                    // sample today's Goal here: it may already have been replaced.
                    let mut admitted = reference.clone();
                    admitted.revision = admitted
                        .revision
                        .checked_add(1)
                        .expect("accepted Goal round advanced its revision");
                    Some(AttemptProvenance::GoalContinuation(admitted))
                } else {
                    None
                }
            })
            .unwrap_or(AttemptProvenance::Inbound);
        // Durable adoption completes this finite admission cycle. The next
        // cycle starts with a fresh select/adopt retry allowance.
        Self::complete_admission_cycle(&mut state);
        // Ownership transfer: the coordinator hands its conversation state
        // to the attempt. From here until settlement the coordinator holds
        // `None` and the attempt is the single mutable conversation
        // authority.
        let mut conversation = state
            .conversation
            .take()
            .expect("the coordinator owns the conversation state while idle");
        for (prepared, item) in prepared_commits.into_iter().zip(&adopted) {
            // Infallible: every adopted identity was validated by
            // `prepare_commit` above under exclusive ownership.
            let block = prepared.message().clone();
            conversation.install_prepared(prepared);
            self.observe(ConversationObservation::Committed {
                attempt_id: None,
                block,
                transcript_cursor: item.transcript_cursor(),
            });
        }
        // The adoption fact is published after its canonical messages, with
        // its exact Journal sequence, releasing the receipt the observation
        // queue is holding for it.
        if let Some(adoption) = adoption {
            self.observe(ConversationObservation::Published {
                journal_sequence: adoption.sequence,
                observation: Box::new(ConversationObservation::InboundAdopted),
            });
        }
        self.publish_attempt(state, conversation, Some(fresh), provenance);
    }

    /// Admits one **continuation** attempt over the already-canonical adopted
    /// turn recovered by startup recovery (Issue #12, M9a, recovery Class B).
    ///
    /// This shares the one admission linearization with
    /// [`RuntimeInner::admit_next_attempt`]: the caller still holds the
    /// coordinator lock, and every idle/shutdown/health/structure check has
    /// already run above. The only difference is the trigger — there is no new
    /// inbound to adopt, so the attempt runs as an explicit
    /// `InitialTurnTrigger::Continuation` and no `UserMessage` is committed a
    /// second time.
    fn admit_continuation(self: &Arc<Self>, mut state: MutexGuard<'_, CoordinatorState>) {
        let conversation = state
            .conversation
            .take()
            .expect("the coordinator owns the conversation state while idle");
        let provenance = state
            .recovered_continuation
            .take()
            .expect("recovered answer obligation");
        self.publish_attempt(state, conversation, None, provenance);
    }

    /// The shared tail of every admission: allocate the attempt identity,
    /// publish the current-attempt slot, freeze the model snapshot, release
    /// the lock, and spawn the attempt task.
    ///
    /// The attempt ordinal comes from the coordinator's recovered allocator,
    /// so it is never an ordinal that already entered durable authority before
    /// a restart.
    fn publish_attempt(
        self: &Arc<Self>,
        mut state: MutexGuard<'_, CoordinatorState>,
        conversation: ConversationState,
        fresh: Option<FreshInboundTurn>,
        provenance: AttemptProvenance,
    ) {
        let attempt_id = AttemptId::for_conversation(&self.conversation_id, state.next_attempt_seq);
        state.next_attempt_seq = state.next_attempt_seq.saturating_add(1);
        // The coordinator-owned cancellation handle is the exact trigger
        // `cancel_current_attempt` requests on: the attempt task runs
        // against the same signal, so protocol cancellation always reaches
        // the loop.
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        // A one-shot child-cancellation intent (Issue #60 child side) that
        // committed before this admission linearizes here: the admitted
        // attempt starts already-cancelled, so its first model-turn
        // arbitration resolves `CancelledBeforeStart` and no provider
        // request ever starts. The intent is consumed exactly once; the
        // child is one-shot, so at most one admission can ever consume it.
        if let Some(reason) = state.one_shot_cancel.take() {
            // First winner by construction: the fresh attempt signal has no
            // prior cause.
            let _ = cancellation.request_cancel(reason);
        }
        state.current_attempt = Some(CurrentAttempt {
            configuration: crate::local_runtime::configuration::settings::AdmittedConfiguration {
                approval_mode: state.effective_approval_mode,
                model_timeout: state
                    .resources
                    .configuration()
                    .map(|generation| generation.config.model_timeout_policy),
                tool_deadline: state
                    .resources
                    .configuration()
                    .map(|generation| generation.config.tool_deadline_policy),
                attempt: attempt_id.clone(),
                generation: state.resources.revision(),
                model: state.model.view(),
                resources: state.resources.inspection().clone(),
            },
            attempt_id: attempt_id.clone(),
            cancellation: cancellation.clone(),
            provenance,
        });
        // The one admission publication point is also where the admitted
        // attempt becomes countable (Issue #193): the subagent child driver
        // compares this against the terminals it has observed to prove it
        // has seen every attempt an accepted steer could have opened.
        state.admitted_attempts = state.admitted_attempts.saturating_add(1);
        // The attempt model snapshot is taken at exactly this admission
        // linearization boundary, under the same lock that publishes the
        // attempt. A `model_set` that linearizes before this point is
        // observed by the attempt; one that linearizes after it affects only
        // future attempts.
        //
        // This is also the historical/future configuration boundary a restart
        // must respect: a recovered attempt is a **future** attempt and uses
        // the current session model, while a historical Request Snapshot is
        // reconstructed only from its own frozen durable facts and is never
        // rewritten to resemble the new configuration.
        let model = state.model.snapshot();
        // The attempt's frozen *effective* model configuration, taken at the
        // same linearization point as its resolved snapshot. A named
        // subagent with no explicit model inherits exactly this — never live
        // mutable session state, and never a composition-time capture.
        let model_config = state.model.config().clone();
        let model_registry = state.model.registry().cloned();
        // Freeze runtime execution policy at the same admission boundary as
        // the attempt's model snapshot. A later configuration change can
        // therefore affect only a future admitted attempt.
        let model_timeout_policy =
            state
                .resources
                .configuration()
                .map_or(self.model_timeout_policy, |generation| {
                    generation
                        .config
                        .timeout_policy()
                        .expect("validated generation")
                });
        // The tool execution-liveness policy freezes at the same boundary
        // (Issue #204): every foreground ToolCall of this attempt runs under
        // exactly this policy value.
        let tool_deadline_policy =
            state
                .resources
                .configuration()
                .map_or(self.tool_deadline_policy, |generation| {
                    generation
                        .config
                        .tool_deadline_policy()
                        .expect("validated generation")
                });
        let approval_mode = state.effective_approval_mode;
        let resources = state.resources.clone();
        let lease = self
            .capability
            .acquire_attempt_lease_for(resources.capability().clone());
        self.observe(ConversationObservation::AttemptAdmitted {
            attempt_id: attempt_id.clone(),
            model: Box::new(model.view()),
            resource_revision: resources.revision(),
            approval_mode,
        });
        // The attempt **task** is a runtime-owned operation in its own
        // right, distinct from the current-attempt slot it settles into
        // (Issue #12, M9c). `finish_attempt` clears the slot and then still
        // calls back into the coordinator, so quiescence must cover the task
        // body, not just the slot. The admission is taken under the same
        // coordinator lock that publishes the slot — drain cannot linearize
        // in between because it needs that lock — and it is released only
        // after the task's final callback has returned.
        let attempt_admission = self
            .lifecycle
            .try_enter_running()
            .expect("the coordinator lock owns the attempt admission boundary");
        drop(state);
        let inner = Arc::clone(self);
        self.executor.spawn(async move {
            let result = inner
                .run_attempt(
                    attempt_id.clone(),
                    conversation,
                    fresh,
                    &cancellation,
                    model,
                    model_timeout_policy,
                    tool_deadline_policy,
                    approval_mode,
                    resources,
                    model_config,
                    model_registry,
                    lease,
                )
                .await;
            inner.finish_attempt(attempt_id, result);
            drop(attempt_admission);
        });
    }
}

/// The background-settlement failure sink of one conversation runtime
/// (Issue #63): the narrow seam through which the background registry
/// reports an exhausted terminal-publication budget.
///
/// The runtime is the durability-health owner of its background plane, so
/// the report moves it into the explicit `DurabilityFailed` state while the
/// unresolved terminal candidate stays retained and observable in the
/// registry. The sink is invoked by the background runner without the
/// registry lock held; it acquires only the coordinator lock, so the lock
/// graph keeps its single coordinator -> registry edge direction.
struct BackgroundFailureSink {
    inner: Weak<RuntimeInner>,
}

impl crate::tools::background::BackgroundDurabilityFailureSink for BackgroundFailureSink {
    fn terminal_publication_failed(
        &self,
        execution_id: &crate::runtime::identity::ToolExecutionId,
        diagnostic: String,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        // The real callback boundary: parked here, the runner has entered its
        // last conversation-facing callback and has provably not yet
        // published `publication_abandoned`. The coordinator lock is taken
        // only after the park, so drain and every other coordinator caller
        // stay live while the callback is held.
        #[cfg(test)]
        let background_failure_gate = inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.background_failure_gate.clone());
        // The probe lock is released before the park: `shutdown` reads the
        // same probe, so holding it across the gate would block the very
        // caller this boundary exists to race.
        #[cfg(test)]
        if let Some(gate) = background_failure_gate {
            gate.enter();
        }
        let mut state = inner.lock_state();
        inner.record_durability_failure(
            &mut state,
            DurableOperation::BackgroundTerminalPublication,
            format!("background execution {execution_id}: {diagnostic}"),
        );
    }
}

/// The subagent-settlement failure sink of one conversation runtime
/// (Issue #60). The registry retains its immutable `PublishingTerminal`
/// candidate; this seam only transfers exhausted durable-health ownership to
/// the coordinator.
struct SubagentFailureSink {
    inner: Weak<RuntimeInner>,
}

impl crate::runtime::subagent::SubagentDurabilityFailureSink for SubagentFailureSink {
    fn terminal_publication_failed(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
        diagnostic: &str,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        // This callback is reached only after the registry's bounded
        // publication attempts are exhausted. It is intentionally invoked
        // outside the registry mutex, preserving the one-way lock graph:
        // ConversationRuntime/coordinator -> SubagentRegistry.
        let mut state = inner.lock_state();
        inner.record_durability_failure(
            &mut state,
            DurableOperation::SubagentTerminalPublication,
            format!("subagent {subagent_id}: {diagnostic}"),
        );
        drop(state);
        #[cfg(test)]
        let gate = inner
            .probe
            .lock()
            .expect("probe lock")
            .as_ref()
            .and_then(|probe| probe.subagent_failure_published_gate.clone());
        #[cfg(test)]
        if let Some(gate) = gate {
            gate.enter();
        }
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
    /// Creates the runtime in its **inactive** lifecycle state, unless an
    /// authoritative MCP physical-settlement failure is replayed during
    /// construction; that failure moves the runtime directly into the
    /// explicit draining/failure lifecycle so activation cannot reopen
    /// healthy admission.
    ///
    /// An ordinary inactive runtime is inert: its mailbox refuses inbound, it
    /// has no admission worker, it admits no attempt, and it publishes no
    /// observation. The sole exception is an authoritative MCP settlement
    /// failure replayed during construction, which enters the explicit
    /// draining/failure lifecycle and is never activatable. Otherwise the
    /// composition may now optionally bind a `RuntimeClientHost` over it, and
    /// must then call [`ConversationRuntime::activate`] before semantic
    /// execution can begin. A headless composition simply activates directly.
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
    /// [`ConversationRuntimeError::InvalidModelTimeoutPolicy`] when either
    /// runtime model deadline is zero,
    /// [`ConversationRuntimeError::InvalidToolDeadlinePolicy`] when the tool
    /// hard deadline or any idle-liveness window is zero,
    /// [`ConversationRuntimeError::Context`] when the context engine
    /// configuration is impossible,
    /// [`ConversationRuntimeError::InvalidInitialConversation`] when the
    /// initial canonical messages are invalid,
    /// [`ConversationRuntimeError::RuntimeAlreadyBound`] when the tool
    /// runtime or the capability coordinator identity is already bound to a
    /// conversation runtime, and
    /// [`ConversationRuntimeError::ToolRuntimeNotQuiescent`] when the tool
    /// runtime's background plane already holds prepared or committed
    /// background work.
    pub fn new(config: RuntimeConversationConfig) -> Result<Self, ConversationRuntimeError> {
        Self::new_with_monotonic_clock(config, None)
    }

    /// Creates a runtime with one explicitly supplied monotonic clock for a
    /// deterministic composition test. This seam is restricted to the crate
    /// test build and remains at the conversation-runtime composition root:
    /// normal runtime admission still constructs both sibling consumers from
    /// `RuntimeInner`'s one clock field.
    #[cfg(test)]
    pub(crate) fn with_test_monotonic_clock(
        config: RuntimeConversationConfig,
        monotonic_clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, ConversationRuntimeError> {
        Self::new_with_monotonic_clock(config, Some(monotonic_clock))
    }

    #[allow(clippy::too_many_lines)]
    fn new_with_monotonic_clock(
        config: RuntimeConversationConfig,
        injected_monotonic_clock: Option<Arc<dyn MonotonicClock>>,
    ) -> Result<Self, ConversationRuntimeError> {
        // The one conversation authority at this boundary: every identity
        // this runtime publishes or derives comes from the tool runtime it
        // coordinates, so runtime and tool runtime cannot disagree.
        let conversation_id = config.tool_runtime.conversation_id().clone();

        // ---- Fallible validation: nothing below is observable yet. ----
        // Validate generic runtime execution policy before store
        // initialization, ownership claims, or any other observable work.
        // Admission can therefore rely on this invariant and keep its
        // per-attempt wiring infallible.
        if !config.model_timeout_policy.is_positive() {
            return Err(ConversationRuntimeError::InvalidModelTimeoutPolicy);
        }
        if !config.tool_deadline_policy.is_positive() {
            return Err(ConversationRuntimeError::InvalidToolDeadlinePolicy);
        }
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
        if !Arc::ptr_eq(config.resources.capability(), &snapshot) {
            return Err(ConversationRuntimeError::OwnershipMismatch {
                capability_conversation: config.resources.capability().conversation_id().clone(),
                runtime_conversation: conversation_id,
            });
        }
        // The one native Agent Extension composition invariant (Issue #259).
        //
        // Domain state is Conversation-owned. Configuration selects Plugin
        // capabilities; selected Tools must have real backing owners.
        let composition = config.tool_runtime.extensions();
        let materialized = crate::extensions::NativeAgentExtensions::from_materialized(
            config.context.status_engine.as_ref(),
            config.tool_runtime.todos(),
            config.tool_runtime.goal(),
        );
        let required = composition.expected_tool_plane();
        let configured_plane = config.capability.extension_tool_plane_shape();
        let active_plane = crate::extensions::ExtensionToolPlaneShape::of_published_registry(
            snapshot.tool_registry(),
        );
        if (configured_plane.todo && materialized.todo().is_none())
            || (configured_plane.goal && materialized.goal().is_none())
            || configured_plane.invalid_goal
            || active_plane != required
            || (required.todo && materialized.todo().is_none())
            || (required.goal && materialized.goal().is_none())
            || materialized.agent_status() != composition.agent_status()
        {
            return Err(ConversationRuntimeError::ExtensionCompositionMismatch {
                conversation_id,
                composed_todo: composition.todo().is_some(),
                materialized_todo_state: config.tool_runtime.todos().is_some(),
                configured_todo_tool: configured_plane.todo,
                active_todo_tool: active_plane.todo,
                agent_status_agrees: materialized.agent_status() == composition.agent_status(),
                goal_agrees: materialized.goal() == composition.goal()
                    && configured_plane.goal == required.goal
                    && active_plane.goal == required.goal
                    && !configured_plane.invalid_goal
                    && !active_plane.invalid_goal,
            });
        }
        // The subagent registry is a conversation-owned logical plane: the
        // runtime must not adopt a registry that belongs to another
        // conversation/agent domain or another canonical mailbox. The typed
        // ownership domain is validated here, before any claim, so a
        // rejected construction consumes nothing (Issue #60 hardening).
        // Pristine authority is deliberately **not** decided here: a
        // standalone child ownership commit can still be in flight, so the
        // authoritative pristine check runs after the mailbox ownership
        // transfer below (the one total-order point).
        if let Some(subagents) = &config.subagents {
            let registry_conversation = subagents.conversation_id().clone();
            if registry_conversation != conversation_id
                || subagents.parent_agent_id() != &config.agent_id
                || !subagents.shares_mailbox_domain(&config.tool_runtime.mailbox())
            {
                return Err(ConversationRuntimeError::SubagentOwnershipMismatch {
                    registry_conversation,
                    runtime_conversation: conversation_id,
                });
            }
        }
        // The initial session model must be able to run under the session
        // context policy. Validating here (and again in `model_set`) is what
        // makes the per-attempt context runtime construction infallible at
        // admission, where there is no caller left to report to.
        validate_context_policy(&config.context.policy, &config.model.snapshot())
            .map_err(|error| ConversationRuntimeError::Context(error.message))?;
        // The durable store is composed once by the tool runtime's
        // `ConversationStoreBinding`. The runtime receives the full handle
        // from that binding, while the mailbox/background plane receives
        // only the derived narrow capability. An independently selected
        // mailbox and store therefore have no production construction path.
        let store = config.tool_runtime.durable_store();
        store
            .initialize(&config.initial_messages)
            .map_err(|error| ConversationRuntimeError::Storage(error.to_string()))?;
        let clock = config
            .clock
            .unwrap_or_else(|| Arc::new(SystemClock) as Arc<dyn RuntimeClock>);
        // Activation spawns the admission worker, and a runtime with no
        // worker would silently never admit anything. The execution
        // runtime is required — and captured — here, still in the fallible
        // section and before any claim, so `activate` spawns the worker
        // unconditionally and the impossible composition is rejected as
        // early as it can be seen.
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            return Err(ConversationRuntimeError::NoExecutionRuntime);
        };

        // ---- Ownership commit: the one tool-runtime ownership transfer. ----
        //
        // The conversation runtime composes the one shared activation
        // lifecycle and claims the tool runtime through one
        // ownership-transfer contract (Issue #61): the transfer runs under
        // the background registry synchronization boundary, requires a
        // pristine background plane (no prepared dispatch, no committed
        // record), claims the one-time coordinator binding, and binds the
        // canonical mailbox runtime-owned with this fresh `Inactive`
        // lifecycle at the same linearization point. A standalone
        // background commit therefore either wins first (this construction
        // fails typed with `ToolRuntimeNotQuiescent`, and the claim is
        // never consumed) or this transfer wins first (a later commit
        // observes the runtime-owned inactive mailbox and is refused with
        // `BackgroundDispatchError::ConversationInactive`). An inactive
        // runtime can never inherit detached background work.
        let lifecycle = ConversationLifecycle::new();
        match config
            .tool_runtime
            .claim_conversation_runtime_inactive(&lifecycle)
        {
            Ok(()) => {}
            Err(crate::tools::runtime::ConversationRuntimeClaimError::AlreadyBound) => {
                return Err(ConversationRuntimeError::RuntimeAlreadyBound { conversation_id });
            }
            Err(crate::tools::runtime::ConversationRuntimeClaimError::NotQuiescent) => {
                return Err(ConversationRuntimeError::ToolRuntimeNotQuiescent { conversation_id });
            }
        }
        // The capability coordinator is a separate identity: it claims the
        // same shared lifecycle under its own state lock, so its commit
        // gate observes exactly the activation decision the mailbox and
        // the background registry observe.
        let Some(capability_publication) = config.capability.claim_conversation_runtime(&lifecycle)
        else {
            // Transactional construction: the tool-runtime ownership
            // transfer is rolled back to its exact previous standalone
            // state — mailbox unbound, coordinator claim released — so a
            // rejected construction leaves no trace.
            config.tool_runtime.release_conversation_runtime_claim();
            return Err(ConversationRuntimeError::RuntimeAlreadyBound { conversation_id });
        };

        // ---- Final subagent ownership arbitration (Issue #60) ----
        //
        // The tool-runtime/mailbox ownership claim above bound the canonical
        // mailbox to this runtime's `Inactive` lifecycle. A standalone
        // `SubagentRegistry::commit` can therefore no longer enter: its
        // mailbox `begin_running_admission`/`with_running_commit` observes
        // the runtime-owned `Inactive` lifecycle and is refused. This is
        // the deterministic total-order point against any standalone child
        // ownership commit that won the race before the bind. That commit
        // holds the registry mutex through its durable ownership write and
        // record publication, so this authoritative pristine check blocks
        // until it finishes and then observes the non-pristine plane — a
        // live child started outside this runtime's ownership transfer is
        // never silently adopted.
        if let Some(subagents) = &config.subagents
            && !subagents.is_pristine()
        {
            // Transactional construction: roll back every claim acquired so
            // far — the capability claim and the tool-runtime claim (which
            // also unbinds the mailbox) — so the failed construction leaves
            // no runtime/mailbox/capability residue and a later correct
            // construction with a fresh plane remains possible.
            config.capability.release_conversation_runtime_claim();
            config.tool_runtime.release_conversation_runtime_claim();
            return Err(ConversationRuntimeError::SubagentRegistryNotPristine { conversation_id });
        }

        // ---- Startup recovery (Issue #12, M9a) ----
        //
        // Recovery runs **after** the ownership transfer and **before** the
        // runtime object exists. Both halves of that placement are
        // load-bearing:
        //
        // - *after* the claim, because reconciliation commits new durable
        //   facts. A construction that loses the ownership race must leave no
        //   trace, so it must never have reconciled anything; and the claim's
        //   pristine-background-plane precondition is exactly what proves the
        //   durably-owned-but-unpublished background executions this phase
        //   terminalizes have **no** live in-process record — they really are
        //   remnants of a dead process, never work this process owns.
        // - *before* the runtime exists, because activation and admission
        //   must not race an unfinished reconciliation. There is no
        //   coordinator lock yet, so no `SQLite` work here can ever be
        //   performed under the admission mutex.
        //
        // The conversation runtime is the recovery-policy owner and runs the
        // complete pipeline:
        //
        //     reconstruct -> classify -> reconcile -> recovered state
        //
        // The store contributes durable evidence and semantic transactions
        // only; it never decides whether an ambiguous request is replayable.
        //
        // A recovery failure rolls the ownership transfer back to its exact
        // previous standalone state and returns typed, so no runtime exists
        // that could admit work as though recovery had completed.
        let recovery = match crate::runtime::recovery::recover(store.as_ref(), clock.as_ref()) {
            Ok(recovery) => recovery,
            Err(error) => {
                config.capability.release_conversation_runtime_claim();
                config.tool_runtime.release_conversation_runtime_claim();
                return Err(match error {
                    crate::runtime::recovery::RecoveryError::Durable(detail) => {
                        ConversationRuntimeError::Storage(detail)
                    }
                    crate::runtime::recovery::RecoveryError::Unrecoverable(reason) => {
                        ConversationRuntimeError::RecoveryRequired { reason }
                    }
                });
            }
        };
        // The recovered hot read model is built from the durable head **after**
        // reconciliation, so it reflects the repaired canonical structure
        // rather than the crash-time one.
        let conversation = match Self::recovered_conversation(store.as_ref()) {
            Ok(conversation) => conversation,
            Err(error) => {
                config.capability.release_conversation_runtime_claim();
                config.tool_runtime.release_conversation_runtime_claim();
                return Err(error);
            }
        };

        // ---- Infallible wiring: from here construction always succeeds. ----
        let mailbox = config.tool_runtime.mailbox();
        // The conversation is inert until `activate`: the ownership
        // transfer already bound its mailbox with the Inactive lifecycle,
        // so nothing can be admitted and nothing can be observed while the
        // optional Runtime Client host binds.
        //
        // The subagent ordinal is the same kind of durable identity domain
        // (Issue #60): reseed above every ordinal already in durable
        // authority before any start can race this.
        if let Some(subagents) = &config.subagents {
            subagents.restore_sequence_watermark(recovery.highest_subagent_ordinal());
        }
        let recovered_continuation = match recovery.resume() {
            crate::runtime::recovery::ResumeDisposition::ContinueAdoptedTurn { goal } => {
                Some(match goal {
                    Some(goal) => AttemptProvenance::RecoveredGoalContinuation(goal),
                    None => AttemptProvenance::RecoveredContinuation,
                })
            }
            _ => None,
        };
        let next_attempt_seq = recovery.next_attempt_ordinal();
        // The coordinator receives the narrow interaction audit capability
        // only (Issue #109): it may commit the requested/settled facts of its
        // own interactions and reach no other durable domain.
        let interaction = Arc::new(InteractionCoordinator::new(
            conversation_id.clone(),
            lifecycle.clone(),
            crate::durable::interaction_audit_capability(store.clone()),
        ));
        // The shared durability frontier: one gate carries the
        // `DurabilityFailed` fact to the conversation-owned registries so
        // their new durable ownership commits linearize against it (Issue
        // #60). It is created here and installed on the registries below,
        // after the ownership transfer and the pristine arbitration, while
        // the runtime is still inactive — no ownership commit can race the
        // installation.
        let durability_gate = Arc::new(DurabilityGate::new());
        let inner = Arc::new(RuntimeInner {
            configuration_owner: Arc::new(()),
            conversation_id,
            agent_id: config.agent_id,
            context: config.context,
            tool_runtime: config.tool_runtime,
            mailbox,
            store,
            capability: config.capability,
            capability_publication,
            resource_loader: config.resource_loader,
            subagents: config.subagents,
            workflow_output: config.workflow_output,
            interaction,
            lifecycle,
            clock,
            model_timeout_policy: config.model_timeout_policy,
            tool_deadline_policy: config.tool_deadline_policy,
            monotonic_clock: injected_monotonic_clock.unwrap_or_else(|| {
                Arc::new(SystemMonotonicClock::new()) as Arc<dyn MonotonicClock>
            }),
            durability_gate: durability_gate.clone(),
            recovery,
            executor,
            state: Mutex::new(CoordinatorState {
                binding_revision: 1,
                explicit_model: config.explicit_model,
                model: config.model,
                resources: config.resources,
                mcp_settlement_failure: None,
                effective_approval_mode: config.approval_mode,
                conversation: Some(conversation),
                current_attempt: None,
                manual_compaction: None,
                next_attempt_seq,
                admitted_attempts: 0,
                parent_turn_permits: None,
                parent_guidance_sealed: false,
                one_shot_cancel: None,
                recovered_continuation,
                // Durability health after a successful recovery is an
                // explicit transition, not a silent reset (Issue #12, M9a):
                // recovery either established a coherent durable state — in
                // which case the runtime starts a fresh admission cycle — or
                // it failed, in which case construction already returned and
                // no runtime exists to be healthy. A previous process's crash
                // never poisons a runtime whose classification and
                // reconciliation succeeded. The absorbing DurabilityFailed
                // fact itself lives only in the DurabilityGate.
                admission_durability_cycle: AdmissionDurabilityCycle {
                    budget: AdmissionRetryBudget::default(),
                    pending_retry: None,
                },
            }),
            wake: Arc::new(WakeGate::new()),
            worker_started: AtomicBool::new(false),
            drain: std::sync::OnceLock::new(),
            drain_started: AtomicBool::new(false),
            pending: std::sync::OnceLock::new(),
            settlement: tokio::sync::Notify::new(),
            #[cfg(test)]
            probe: Mutex::new(None),
            #[cfg(test)]
            subagent_transcript_store_reads: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            test_pre_tool_policy: Mutex::new(None),
        });
        // Recovery has already durably terminalized every orphaned child.
        // Restore only terminal read-model records and their durable
        // workspace resource facts (proven handoffs or unresolved ownership);
        // no child process, policy, or frozen resource is reconstructed from
        // current configuration.
        if let Some(subagents) = &inner.subagents {
            for handoff in inner.recovery.settled_subagent_handoffs() {
                subagents.restore_recovered_handoff(handoff);
            }
            for handoff in &inner.recovery.reconciliation().subagent_handoffs {
                subagents.restore_recovered_handoff(handoff);
            }
            for unresolved in inner.recovery.settled_subagent_unresolved() {
                subagents.restore_recovered_unresolved(unresolved);
            }
            for unresolved in &inner.recovery.reconciliation().subagent_unresolved {
                subagents.restore_recovered_unresolved(unresolved);
            }
            for disposal in inner.recovery.settled_subagent_disposals() {
                subagents.restore_recovered_disposal(disposal);
            }
            subagents
                .restore_agents(inner.store.as_ref())
                .map_err(|error| ConversationRuntimeError::Storage(error.to_string()))?;
        }
        // The runtime is the durability-health owner of its background
        // plane (Issue #63): install the narrow failure seam the
        // background settlement owner reports an exhausted
        // terminal-publication budget through. No background execution can
        // exist before activation — the registry refused every commit
        // while the mailbox was bound inactive — so the installation can
        // never race a settlement.
        inner
            .tool_runtime
            .background()
            .install_failure_sink(Arc::new(BackgroundFailureSink {
                inner: Arc::downgrade(&inner),
            }));
        // The runtime durability frontier is shared with both
        // conversation-owned durable ownership planes: a new subagent or
        // background ownership commit must linearize against the runtime's
        // `DurabilityFailed` commit on the one gate.
        inner
            .tool_runtime
            .background()
            .install_durability_gate(durability_gate.clone());
        // The conversation runtime is also the durability-health owner of
        // its subagent plane. The registry calls this seam only after its
        // bounded terminal-publication retry budget is exhausted, and never
        // while its own registry mutex is held.
        if let Some(subagents) = &inner.subagents {
            subagents.install_failure_sink(Arc::new(SubagentFailureSink {
                inner: Arc::downgrade(&inner),
            }));
            subagents.install_durability_gate(durability_gate.clone());
        }
        // A retired MCP generation whose close reports PhysicalSettlement
        // has not been reclaimed. The registry retains the evidence; this
        // runtime-owned callback closes healthy admission immediately while
        // preserving the newly published logical resource generation for
        // final drain reporting.
        let mcp_failure_callback: Arc<dyn Fn(String) + Send + Sync> = Arc::new({
            let weak = Arc::downgrade(&inner);
            move |detail| {
                if let Some(inner) = weak.upgrade() {
                    inner.fence_mcp_settlement_failure(detail);
                }
            }
        });
        inner
            .capability
            .install_mcp_retirement_failure_callback(&mcp_failure_callback);
        Ok(Self { inner })
    }

    /// Hydrates the bounded hot conversation read model from the durable
    /// Surface head, after startup recovery has reconciled it.
    ///
    /// Only the current active working set is materialized: retired Ledger
    /// facts and historical Surface revisions stay in the durable store and
    /// are read on demand.
    fn recovered_conversation(
        store: &dyn ConversationStore,
    ) -> Result<ConversationState, ConversationRuntimeError> {
        let head = store
            .load_head()
            .map_err(|error| ConversationRuntimeError::Storage(error.to_string()))?;
        let active = store
            .load_messages(&head.active_message_ids)
            .map_err(|error| ConversationRuntimeError::Storage(error.to_string()))?;
        ConversationState::from_durable_head(
            active,
            head.active_message_ids,
            head.revision,
            head.compaction_generation,
        )
        .map_err(|error| ConversationRuntimeError::InvalidInitialConversation(error.to_string()))
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

    #[cfg(test)]
    pub(crate) fn install_configuration_admission_gate(&self, gate: Arc<Gate>) {
        self.inner
            .probe
            .lock()
            .expect("probe")
            .get_or_insert_with(Default::default)
            .admission_gate = Some(gate);
    }

    #[cfg(test)]
    pub(crate) fn install_drain_signals(
        &self,
        arrival: Arc<tokio::sync::Notify>,
        linearization: Arc<tokio::sync::Notify>,
    ) {
        let mut probe = self.inner.probe.lock().expect("probe lock");
        let probe = probe.get_or_insert_with(CoordinatorProbe::default);
        probe.shutdown_arrival = Some(arrival);
        probe.drain_linearization = Some(linearization);
    }

    #[cfg(test)]
    pub(crate) fn install_admission_gate(&self, gate: Arc<Gate>) {
        self.inner
            .probe
            .lock()
            .expect("probe lock")
            .get_or_insert_with(CoordinatorProbe::default)
            .admission_gate = Some(gate);
    }

    #[cfg(test)]
    pub(crate) fn install_activation_gate(&self, gate: Arc<Gate>) {
        self.inner
            .probe
            .lock()
            .expect("probe lock")
            .get_or_insert_with(CoordinatorProbe::default)
            .activation_gate = Some(gate);
    }

    /// Install the existing owner-boundary probes on a natively composed runtime.
    #[cfg(test)]
    pub(crate) fn install_residency_probe(
        &self,
        submit: Option<Arc<Gate>>,
        attempt_exit: Option<Arc<Gate>>,
    ) {
        let mut probe = self.inner.probe.lock().expect("probe lock");
        let probe = probe.get_or_insert_with(CoordinatorProbe::default);
        probe.submit_gate = submit;
        probe.attempt_exit_gate = attempt_exit;
    }

    /// Inject at the existing native settlement-failure callback, not at the
    /// manager's shutdown result, so residency tests exercise the real drain.
    #[cfg(test)]
    pub(crate) fn fail_residency_settlement(&self) {
        self.inner
            .fence_mcp_settlement_failure("residency test settlement failure".into());
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

    /// Context inputs projected from one immutable published generation.
    /// Admitted attempts retain their own captured resources and engine state.
    #[must_use]
    pub fn context_config(&self) -> ConversationContextConfig {
        let resources = self.runtime_resources();
        let configuration = resources.configuration();
        ConversationContextConfig {
            policy: configuration.map_or(self.inner.context.policy, |g| g.config.context_policy()),
            estimator: Arc::clone(&self.inner.context.estimator),
            status_engine: configuration.map_or_else(
                || {
                    self.inner
                        .context
                        .status_engine
                        .as_ref()
                        .map(AgentStatusEngine::for_attempt)
                },
                |g| {
                    g.config
                        .extension_composition()
                        .agent_status_engine(Arc::new(crate::context::SystemClock))
                },
            ),
        }
    }

    /// The closed Plugins selected by the published Root or admitted child profile.
    /// Conversation-owned Plugin state is retained independently of this selection.
    #[must_use]
    pub fn native_extensions(&self) -> crate::extensions::NativeAgentExtensions {
        self.runtime_resources().root_profile().map_or_else(
            || self.inner.tool_runtime.extensions().clone(),
            |profile| profile.extensions.clone(),
        )
    }

    /// The one capability coordinator of this runtime.
    #[must_use]
    pub fn capability(&self) -> &CapabilityCoordinator {
        &self.inner.capability
    }

    /// Answers one live native interaction through its originating
    /// conversation. The root runtime is only a routing surface: it never
    /// creates a parent interaction for a child and never settles a child
    /// coordinator itself.
    pub(crate) async fn respond_interaction(
        &self,
        interaction: &InteractionRef,
        response: crate::runtime::interaction::InteractionResponse,
    ) -> Result<(), RoutedInteractionError> {
        self.control_interaction(
            interaction,
            crate::runtime::interaction::InteractionControl::Respond { response },
        )
        .await
    }

    #[cfg(test)]
    pub(crate) fn interaction_test_owner(
        &self,
    ) -> Arc<crate::runtime::interaction::InteractionCoordinator> {
        self.inner.interaction.clone()
    }

    pub(crate) async fn control_interaction(
        &self,
        interaction: &InteractionRef,
        control: crate::runtime::interaction::InteractionControl,
    ) -> Result<(), RoutedInteractionError> {
        if interaction.conversation_id == self.inner.conversation_id {
            let result = match control {
                crate::runtime::interaction::InteractionControl::Respond { response } => {
                    self.inner
                        .interaction
                        .respond_async(&interaction.interaction_id, response)
                        .await
                }
                crate::runtime::interaction::InteractionControl::Cancel => {
                    self.inner
                        .interaction
                        .cancel_async(
                            &interaction.interaction_id,
                            CancellationReason::UserRequested,
                        )
                        .await
                }
            };
            return result.map_err(|error| route_error(interaction, error));
        }
        if let Some(subagents) = &self.inner.subagents {
            return subagents.control_interaction(interaction, control).await;
        }
        Err(RoutedInteractionError::NotPending {
            interaction: interaction.clone(),
        })
    }

    /// Installs the reliable parent route on a child conversation's own
    /// coordinator. The route receives only already-authoritative semantic
    /// request/settlement facts and no interaction ownership state.
    pub(crate) fn install_interaction_route(&self, route: Arc<dyn InteractionRoute>) {
        self.inner.interaction.install_route(route);
    }

    /// Installs the root Runtime Client's synchronized publication-admission
    /// frontier for child interaction routes. The authority answers only
    /// whether a capable root runtime projection exists; the child
    /// coordinator remains the semantic owner of every interaction.
    pub(crate) fn install_interaction_publication_authority(
        &self,
        authority: Arc<dyn crate::runtime::subagent::InteractionPublicationAuthority>,
    ) {
        if let Some(subagents) = &self.inner.subagents {
            subagents.install_interaction_publication_authority(authority);
        }
    }

    /// Publishes an early provider-availability hint for future interactions.
    /// Runtime Client host binding is the production caller; the
    /// root host's publication authority is the actual admission frontier,
    /// and existing pending work is intentionally unaffected.
    pub(crate) fn set_interaction_provider_available(&self, available: bool) {
        self.inner.interaction.set_provider_available(available);
        if let Some(subagents) = &self.inner.subagents {
            subagents.set_interaction_provider_available(available);
        }
    }

    /// The immutable result of this runtime's startup recovery (Issue #12,
    /// M9a).
    ///
    /// The report is *observable* downstream — a Runtime Client, a headless
    /// driver, or a regression may read it — but it is never authoritative:
    /// a client inspects the recovered state and never decides it. Recovery
    /// completed before this runtime existed, so the report cannot change.
    #[must_use]
    pub fn recovery(&self) -> &crate::runtime::recovery::RecoveryReport {
        &self.inner.recovery
    }

    /// Installs the observation bridge shared with the Runtime Client
    /// adapter (or a headless observer) and captures the semantic
    /// bootstrap snapshot, as one linearizable handoff.
    ///
    /// See the module documentation ("Adapter bootstrap linearization")
    /// for the cut contract: every authority contributes its seed under
    /// its own lock in the same section that installs its observation
    /// seam, and the queue is installed under the same section that
    /// captures the coordinator-owned facts, so a transition can never be
    /// lost between a seed and the live observation stream and can never
    /// be applied twice.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeBootstrapError::BridgeAlreadyInstalled`] when an
    /// observation bridge was already installed (a previous headless
    /// consumer). The one-time Runtime Client binding claim guarantees
    /// this cannot happen for a production adapter, and a failed host
    /// construction releases that claim again.
    #[cfg(test)]
    pub(crate) fn install_observation_bridge(
        &self,
        queue: Arc<PendingObservations>,
    ) -> Result<RuntimeBootstrapSnapshot, RuntimeBootstrapError> {
        self.inner.install_observation_bridge(queue, false)
    }

    /// Runtime Client composition additionally stages durable publication receipts.
    /// Native/headless consumers retain ordinary semantic observation delivery.
    pub(crate) fn install_client_observation_bridge(
        &self,
        queue: Arc<PendingObservations>,
    ) -> Result<RuntimeBootstrapSnapshot, RuntimeBootstrapError> {
        self.inner.install_observation_bridge(queue, true)
    }

    /// Adds a bounded local consumer to the Runtime Client observation
    /// stream before this runtime is activated.
    ///
    /// The Runtime Client host remains the primary projection owner. This
    /// seam exists so an existing local observation surface can consume the
    /// same runtime-owned observations without turning the parent into a
    /// transcript authority or stealing the host's queue.
    pub(crate) fn subscribe_observations(
        &self,
    ) -> Result<Arc<PendingObservations>, RuntimeObservationSubscriptionError> {
        self.inner.subscribe_observations()
    }

    /// Test-only: the observation bridge installed through
    /// [`ConversationRuntime::install_observation_bridge`], when one is.
    ///
    /// Boundary tests park/unpark the parent's projection input through
    /// this handle to own the fold schedule deterministically.
    #[cfg(test)]
    pub(crate) fn installed_observation_bridge(&self) -> Option<Arc<PendingObservations>> {
        self.inner.pending.get().map(|fanout| fanout.primary())
    }

    /// Claims the one-time Runtime Client binding of the tool runtime and of
    /// the capability coordinator.
    ///
    /// The Runtime Client protocol binds one runtime identity to at most one
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
    /// This is the one explicit lifecycle boundary of Issue #61: the single
    /// `Inactive -> Running` transition of the shared
    /// [`ConversationLifecycle`](crate::runtime::types::ConversationLifecycle)
    /// every runtime-owned semantic boundary observes. Before it, the
    /// runtime is inert and a `RuntimeClientHost` may bind over it; at it,
    /// the Runtime Client host-binding decision is frozen. Activation is
    /// the bootstrap cut a bound Runtime Client projection is seeded
    /// against.
    ///
    /// The `compare_exchange` runs while the one coordinator lock is held and
    /// the lifecycle's native commit boundary is acquired, so the
    /// host-binding decision races atomically against activation and the
    /// lifecycle also has a total order with non-coordinator runtime commits.
    /// A bootstrap that acquires the coordinator lock first sees `Inactive`
    /// and completes before activation, one that acquires it after sees
    /// `Running` and is refused with
    /// [`RuntimeBootstrapError::RuntimeAlreadyActivated`]. The transition
    /// itself is the linearization point; spawning the admission worker and
    /// the initial admission kick are the one-time post-transition steps of
    /// the single winning caller.
    ///
    /// Runtime Client *attachments* remain fully dynamic afterwards: this
    /// boundary freezes only which adapter (if any) observes the runtime,
    /// never how long a client stays attached.
    ///
    /// Activating twice is a no-op: exactly one concurrent call commits the
    /// transition and performs the one-time post-activation work; every
    /// other call observes `Running` and returns without changing anything.
    /// An authoritative MCP physical-settlement failure is a separate
    /// terminal fence: activation observes it under the coordinator lock and
    /// returns without transitioning the lifecycle to `Running`.
    ///
    /// # Panics
    ///
    /// Panics only if the test-only coordinator probe lock is poisoned,
    /// which would mean a previous test hook panicked while holding it.
    pub fn activate(&self) {
        // Test-only activation gate: parked before the coordinator lock and
        // before the lifecycle transition, so while the park holds the
        // conversation is provably still Inactive and every competing
        // runtime-owned commit or host bind can still proceed. The gate
        // parks only when armed and disarms after one park. The gate handle
        // is extracted before the park so the probe mutex is not held while
        // parked.
        #[cfg(test)]
        let activation_gate = self
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.activation_gate.clone());
        #[cfg(test)]
        if let Some(gate) = activation_gate {
            gate.enter();
        }
        {
            // The lock serializes the lifecycle transition against the
            // host-binding handshake; the CAS is the transition itself.
            let state = self.inner.lock_state();
            if state.mcp_settlement_failure.is_some() {
                // An authoritative physical-settlement failure may have been
                // replayed while the runtime was inactive. The callback has
                // either already moved the lifecycle to Draining or is
                // ordered immediately before this check; in neither case may
                // activation open healthy admission.
                return;
            }
            if !self.inner.lifecycle.activate() {
                return;
            }
            // Spawn the worker before releasing the same lock that drain
            // takes. This closes the activation-vs-drain window in which a
            // successful activation could otherwise leave `worker_started`
            // false while drain observes quiescence.
            self.inner.ensure_worker();
        }
        // Any inbound published before activation (there can be none: the
        // mailbox refused it) and any inbound racing this activation is
        // admitted here rather than depending on a wake permit.
        self.inner.admit_next_attempt();
    }

    /// Whether this runtime has left the inactive lifecycle, including an
    /// explicit MCP-failure drain that never opened healthy admission.
    #[must_use]
    pub fn is_activated(&self) -> bool {
        self.inner.lifecycle.is_activated()
    }

    /// Submits one ordinary inbound user message.
    ///
    /// The durable Pending Inbound Inbox owns authoritative metadata: the
    /// message identity (deterministic from the allocated sequence), the
    /// inbound sequence, the persisted timestamp, and the provenance are
    /// all owner-assigned and commit in one durable acceptance transaction.
    /// Success means durably accepted, never assistant-finished: the runtime
    /// wake gate admits the next attempt when the runtime is idle, and while
    /// an attempt is running the message stays durably pending for the next
    /// safe-boundary adoption.
    ///
    /// # Errors
    ///
    /// Returns [`InboundAdmissionError::Inactive`] before activation,
    /// [`InboundAdmissionError::Shutdown`] after shutdown,
    /// [`InboundAdmissionError::EmptyContent`] for empty content,
    /// [`InboundAdmissionError::DurabilityFailed`] while the runtime's
    /// durable authority is in the explicit failed state, and
    /// [`InboundAdmissionError::Mailbox`] for a durable acceptance failure.
    ///
    /// # Panics
    ///
    /// Panics only if the test-only coordinator probe lock is poisoned,
    /// which would mean a previous test hook panicked while holding it.
    pub fn submit_inbound(
        &self,
        content: Vec<UserContentBlock>,
    ) -> Result<InboundAdmission, InboundAdmissionError> {
        self.submit_sourced_inbound(UserSource::Human, content)
    }

    /// Prepare a source binding for its explicit selection without holding
    /// execution admission. Availability uses the captured default; rebinding
    /// to an existing Session is a separate preparation step.
    pub(crate) async fn prepare_configuration(
        &self,
        capture: crate::local_runtime::configuration::ProspectiveSessionConfig,
        selection: crate::model::session::SessionModelConfig,
        cancellation: &crate::runtime::cancellation::CancellationSignal,
    ) -> Result<crate::local_runtime::configuration::application::PreparedConfiguration, String>
    {
        let (baseline, old, old_model) = {
            let state = self.inner.lock_state();
            (
                state.binding_revision,
                state.resources.clone(),
                state.model.clone(),
            )
        };
        let prepared = self
            .inner
            .resource_loader
            .prepare(&self.inner.capability, Some(capture), cancellation)
            .await
            .map_err(|error| error.message)?;
        let (capability, resources) = prepared.into_parts();
        let generation = resources
            .configuration
            .as_ref()
            .ok_or("candidate has no configuration")?;
        let model = SessionModelState::new(generation.models.clone(), selection)
            .map_err(|error| error.to_string())?;
        validate_context_policy(&generation.config.context_policy(), &model.snapshot())
            .map_err(|error| error.message)?;
        let preview = Arc::new(RuntimeResourceSnapshot::from_prepared(
            old.revision().next(),
            resources.clone(),
            capability.configuration_snapshot(old.capability()),
        ));
        let mut impact = match (old.request_shape(&old_model), preview.request_shape(&model)) {
            (Ok(old_shape), Ok(candidate)) => old_shape.compare(&candidate),
            (Err(error), _) | (_, Err(error)) => {
                #[cfg(test)]
                eprintln!("configuration shape unavailable: {error}");
                let _ = error;
                crate::model::request_shape::CacheImpact::Unproven
            }
        };
        if old
            .configuration()
            .map(|configuration| configuration.config.context)
            != preview
                .configuration()
                .map(|configuration| configuration.config.context)
            && impact == crate::model::request_shape::CacheImpact::Preserved
        {
            impact = crate::model::request_shape::CacheImpact::Unproven;
        }
        Ok(
            crate::local_runtime::configuration::application::PreparedConfiguration {
                selection_only: false,
                owner: self.inner.configuration_owner.clone(),
                baseline,
                preview,
                model,
                capability: Some(capability),
                resources: Some(resources),
                impact,
            },
        )
    }

    pub(crate) fn complete_candidate_context(
        &self,
        candidate: &mut crate::local_runtime::configuration::application::PreparedConfiguration,
        capture: &crate::local_runtime::configuration::ProspectiveSessionConfig,
        models: crate::model::invocation::ModelBindingRegistry,
    ) -> Result<(), String> {
        let model = SessionModelState::new(models.clone(), capture.session_model().clone())
            .map_err(|error| error.to_string())?;
        validate_context_policy(&capture.config.context_policy(), &model.snapshot())
            .map_err(|error| error.message)?;
        if let Some(capability) = &mut candidate.capability {
            capability.set_context_profile(capture);
        }
        if let Some(resources) = &mut candidate.resources {
            resources.set_context_binding(capture, models.clone());
        }
        candidate.preview = Arc::new(candidate.preview.with_context_binding(capture, models)?);
        candidate.model = model;
        let state = self.inner.lock_state();
        candidate.impact = match (
            state.resources.request_shape(&state.model),
            candidate.preview.request_shape(&candidate.model),
        ) {
            (Ok(old), Ok(new)) => old.compare(&new),
            (Err(error), _) | (_, Err(error)) => {
                #[cfg(test)]
                eprintln!("configuration shape unavailable: {error}");
                let _ = error;
                crate::model::request_shape::CacheImpact::Unproven
            }
        };
        if state
            .resources
            .configuration()
            .map(|config| config.config.context)
            != candidate
                .preview
                .configuration()
                .map(|config| config.config.context)
            && candidate.impact == crate::model::request_shape::CacheImpact::Preserved
        {
            candidate.impact = crate::model::request_shape::CacheImpact::Unproven;
        }
        Ok(())
    }

    pub(crate) fn prepare_context_configuration(
        &self,
        capture: &crate::local_runtime::configuration::ProspectiveSessionConfig,
        models: crate::model::invocation::ModelBindingRegistry,
        selection: Option<crate::model::session::SessionModelConfig>,
    ) -> Result<crate::local_runtime::configuration::application::PreparedConfiguration, String>
    {
        let (baseline, old, old_model) = {
            let state = self.inner.lock_state();
            (
                state.binding_revision,
                state.resources.clone(),
                state.model.clone(),
            )
        };
        let model = SessionModelState::new(
            models.clone(),
            selection.unwrap_or_else(|| old_model.config().clone()),
        )
        .map_err(|error| error.to_string())?;
        validate_context_policy(&capture.config.context_policy(), &model.snapshot())
            .map_err(|error| error.message)?;
        let preview = Arc::new(old.with_context_binding(capture, models)?);
        let mut impact = match (old.request_shape(&old_model), preview.request_shape(&model)) {
            (Ok(old_shape), Ok(candidate)) => old_shape.compare(&candidate),
            (Err(error), _) | (_, Err(error)) => {
                #[cfg(test)]
                eprintln!("configuration shape unavailable: {error}");
                let _ = error;
                crate::model::request_shape::CacheImpact::Unproven
            }
        };
        if old.configuration().map(|config| config.config.context)
            != preview.configuration().map(|config| config.config.context)
            && impact == crate::model::request_shape::CacheImpact::Preserved
        {
            impact = crate::model::request_shape::CacheImpact::Unproven;
        }
        Ok(
            crate::local_runtime::configuration::application::PreparedConfiguration {
                selection_only: false,
                owner: self.inner.configuration_owner.clone(),
                baseline,
                preview,
                model,
                capability: None,
                resources: None,
                impact,
            },
        )
    }

    pub(crate) fn configuration_allocation_matches(
        &self,
        candidate: &crate::local_runtime::configuration::application::PreparedConfiguration,
    ) -> bool {
        Arc::ptr_eq(&candidate.owner, &self.inner.configuration_owner)
    }

    /// Publish readiness while holding the same binding fence as model changes
    /// and Attempt admission. The caller holds the source/application fence.
    pub(crate) fn with_configuration_baseline(
        &self,
        candidate: &crate::local_runtime::configuration::application::PreparedConfiguration,
        publish: impl FnOnce(),
    ) -> bool {
        let state = self.inner.lock_state();
        if !self.inner.lifecycle.is_running()
            || state.binding_revision != candidate.baseline
            || !Arc::ptr_eq(&candidate.owner, &self.inner.configuration_owner)
        {
            return false;
        }
        publish();
        true
    }

    pub(crate) fn configuration_adoption_eligibility(
        &self,
    ) -> crate::local_runtime::configuration::application::AdoptionEligibility {
        use crate::local_runtime::configuration::application::AdoptionEligibility;
        let state = self.inner.lock_state();
        if !self.inner.lifecycle.is_running() {
            AdoptionEligibility::Unavailable
        } else if self.inner.idle_epoch_locked(&state).is_err()
            || self.inner.tool_runtime.background().configuration_busy()
            || self
                .inner
                .subagents
                .as_ref()
                .is_some_and(crate::runtime::subagent::SubagentRegistry::configuration_busy)
        {
            AdoptionEligibility::Busy
        } else {
            AdoptionEligibility::Eligible
        }
    }

    /// Final commit primitive used by the native configuration owner. It holds
    /// the source fence around this call; this gate is also Attempt admission's
    /// gate. A refusal leaves the ready candidate available to inspect/retry.
    pub(crate) fn adopt_configuration(
        &self,
        prepared: &mut Option<
            crate::local_runtime::configuration::application::PreparedConfiguration,
        >,
        expected_binding: u64,
        explicit: bool,
        retain: impl FnOnce() -> Result<
            (),
            crate::local_runtime::configuration::application::AdoptionError,
        >,
    ) -> Result<u64, crate::local_runtime::configuration::application::AdoptionError> {
        use crate::local_runtime::configuration::application::AdoptionError;
        let mut state = self.inner.lock_state();
        if explicit
            && (self.inner.idle_epoch_locked(&state).is_err()
                || self.inner.tool_runtime.background().configuration_busy()
                || self
                    .inner
                    .subagents
                    .as_ref()
                    .is_some_and(crate::runtime::subagent::SubagentRegistry::configuration_busy))
        {
            return Err(AdoptionError::Busy);
        }
        let candidate = prepared.as_ref().ok_or(AdoptionError::NotReady)?;
        if !self.inner.lifecycle.is_running()
            || state.binding_revision != expected_binding
            || candidate.baseline != expected_binding
            || !Arc::ptr_eq(&candidate.owner, &self.inner.configuration_owner)
            || (!explicit
                && candidate.impact != crate::model::request_shape::CacheImpact::Preserved)
        {
            return Err(AdoptionError::Conflict);
        }
        let candidate = prepared.take().expect("checked candidate");
        let revision = if candidate.selection_only {
            state.resources.revision()
        } else {
            state.resources.revision().next()
        };
        let (resources, availability) = if let Some(capability) = candidate.capability {
            let committed = self
                .inner
                .capability
                .commit_runtime(&self.inner.capability_publication, capability)
                .map_err(|error| AdoptionError::Failed {
                    diagnostic: error.to_string(),
                })?;
            (
                Arc::new(RuntimeResourceSnapshot::from_prepared(
                    revision,
                    candidate.resources.expect("capability resource closure"),
                    committed.snapshot,
                )),
                committed.availability,
            )
        } else {
            (
                Arc::new(candidate.preview.as_ref().clone().with_revision(revision)),
                candidate.preview.capability_availability().clone(),
            )
        };
        retain()?;
        state.model = candidate.model;
        state.effective_approval_mode = resources
            .configuration()
            .map_or(state.effective_approval_mode, |generation| {
                generation.config.approval_mode
            });
        state.resources = resources.clone();
        state.binding_revision = state
            .binding_revision
            .checked_add(1)
            .expect("Session binding revision exhausted");
        if candidate.selection_only {
            state.explicit_model = true;
            self.inner
                .observe(ConversationObservation::SessionModelChanged {
                    model: Box::new(state.model.view()),
                });
        } else {
            self.inner.observe(ConversationObservation::Resources {
                model: Box::new(state.model.view()),
                approval_mode: state.effective_approval_mode,
                snapshot: resources,
                availability,
            });
        }
        Ok(state.binding_revision)
    }

    pub(crate) fn apply_shared_capacity(
        &self,
        desired: &crate::local_runtime::configuration::IndependentPolicy,
    ) {
        if let Some(subagents) = &self.inner.subagents {
            subagents.publish_configuration(desired);
        }
        let mut state = self.inner.lock_state();
        if let Some(resources) = state.resources.with_shared_capacity(desired) {
            state.resources = Arc::new(resources);
        }
    }

    pub(crate) async fn settle_configuration_resources(&self) {
        // Retirement only collects generations whose immutable leases are gone.
        // Physical failure is reported by the existing native health owner.
        let _ = self.inner.capability.settle_ready_mcp_runtimes().await;
    }

    pub(crate) fn restore_configuration_binding(&self, revision: u64) {
        let mut state = self.inner.lock_state();
        assert_eq!(
            self.inner.lifecycle.state(),
            ConversationLifecycleState::Inactive
        );
        state.binding_revision = revision;
    }

    /// Publish independent policy without changing the adopted context or any
    /// admitted Attempt. The native configuration owner holds its input fence
    /// across this call; this lock orders the swap against Attempt capture.
    pub(crate) fn apply_execution_policy(
        &self,
        desired: &crate::local_runtime::configuration::IndependentPolicy,
    ) -> Result<bool, String> {
        desired.timeout_policy().map_err(|e| e.to_string())?;
        desired.tool_deadline_policy().map_err(|e| e.to_string())?;
        let mut state = self.inner.lock_state();
        if !self.inner.lifecycle.is_running() {
            return Err("runtime is not running".into());
        }
        let Some(resources) = state.resources.with_execution_policy(desired) else {
            return Ok(false);
        };
        let resources = Arc::new(resources);
        state.effective_approval_mode = desired.approval_mode;
        state.resources = resources.clone();
        self.inner.observe(ConversationObservation::Resources {
            model: Box::new(state.model.view()),
            approval_mode: state.effective_approval_mode,
            availability: resources.capability_availability().clone(),
            snapshot: resources,
        });
        Ok(true)
    }

    /// Read configuration, Session intent and admitted work in one coherent cut.
    #[must_use]
    pub fn configuration_view(
        &self,
    ) -> Option<crate::local_runtime::configuration::settings::EffectiveConfiguration> {
        let state = self.inner.lock_state();
        let resources = &state.resources;
        let generation = resources.configuration()?;
        let config = &generation.config;
        Some(
            crate::local_runtime::configuration::settings::EffectiveConfiguration {
                process_bindings: None,
                application: None,
                adopted_binding: state.binding_revision,
                source_revisions: generation.source_revisions.clone(),
                generation: resources.revision(),
                document: generation.effective.clone(),
                root_agent: config.agent.clone(),
                context: config.context,
                model_timeout: config.model_timeout_policy,
                tool_deadline: config.tool_deadline_policy,
                child_capacity: config.subagents.clone(),
                approval_mode: state.effective_approval_mode,
                provenance: generation.provenance.clone(),
                resources: resources.inspection().clone(),
                available_tools: resources
                    .capability()
                    .available_tools()
                    .tools()
                    .iter()
                    .map(|tool| tool.definition.clone())
                    .collect(),
                session_model: state.explicit_model.then(|| state.model.view().configured),
                effective_model: state.model.view(),
                admitted_attempt: state
                    .current_attempt
                    .as_ref()
                    .map(|attempt| attempt.configuration.clone()),
            },
        )
    }

    /// The complete generation future attempts currently acquire.
    #[must_use]
    pub fn runtime_resources(&self) -> Arc<RuntimeResourceSnapshot> {
        self.inner.lock_state().resources.clone()
    }

    /// Manually compacts the current canonical Conversation Surface.
    ///
    /// This is an idle runtime-maintenance operation, not an Agent attempt:
    /// it allocates no attempt/turn identity and never invokes tools. The
    /// current session model and capability snapshot are frozen at admission,
    /// the provider-backed summary runs in a runtime-owned task, and success
    /// is returned only after the canonical summary and Surface replacement
    /// committed atomically. Inbound accepted while compaction runs remains
    /// pending and is admitted after the conversation state is restored.
    ///
    /// # Errors
    ///
    /// Returns [`ManualCompactionError::Busy`] while an attempt or another
    /// manual compaction owns the state, plus typed lifecycle, durability,
    /// planning, summary, cancellation, and durable-commit failures.
    ///
    /// # Panics
    ///
    /// Panics only if an idle coordinator has lost ownership of its sole
    /// conversation state, which would violate the runtime ownership
    /// invariant, or if the configured executor rejects the spawned task.
    #[allow(clippy::too_many_lines)] // Admission and terminal settlement remain visible in one transaction.
    pub async fn compact_context(&self) -> Result<ManualCompactionOutcome, ManualCompactionError> {
        let completion = {
            let mut state = self.inner.lock_state();
            match self.inner.lifecycle.state() {
                ConversationLifecycleState::Inactive => {
                    return Err(ManualCompactionError::Inactive);
                }
                ConversationLifecycleState::Draining | ConversationLifecycleState::Quiescent => {
                    return Err(ManualCompactionError::Shutdown);
                }
                ConversationLifecycleState::Running => {}
            }
            if let Some(failure) = self.inner.durability_gate.failure() {
                return Err(ManualCompactionError::DurabilityFailed {
                    message: failure.diagnostic,
                });
            }
            if state.current_attempt.is_some() || state.manual_compaction.is_some() {
                return Err(ManualCompactionError::Busy);
            }
            let model = state.model.snapshot();
            let resources = state.resources.clone();
            let context_runtime = self
                .inner
                .context_runtime_with_assembly(
                    &model,
                    resources.context_assembly().clone(),
                    resources.configuration().map_or(
                        self.inner.model_timeout_policy,
                        |generation| {
                            generation
                                .config
                                .timeout_policy()
                                .expect("validated generation")
                        },
                    ),
                    &resources,
                )
                .map_err(ManualCompactionError::Context)?;
            // Manual maintenance has no attempt lease, but it still freezes
            // the exact capability definitions used for planning at the
            // same admission boundary as the model/context snapshot.
            let tools = resources.capability().tool_registry().model_definitions();
            let system_sections = resources
                .context_assembly()
                .system_sections(&NativeContextInput {
                    workspace_instructions: resources.project_instructions().map(str::to_owned),
                    skill_guidance: resources.skill_catalog().map(str::to_owned),
                    agent_profile: resources.agent_profile().map(str::to_owned),
                    ..NativeContextInput::default()
                })
                .map_err(|error| {
                    ManualCompactionError::Context(ContextError::new(
                        ContextErrorKind::Internal,
                        format!("the frozen capability system guidance is invalid: {error}"),
                    ))
                })?;
            let conversation = state
                .conversation
                .take()
                .expect("the idle coordinator owns the conversation state");
            let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
            state.manual_compaction = Some(CurrentManualCompaction {
                cancellation: cancellation.clone(),
            });
            self.inner
                .observe(ConversationObservation::ManualCompactionEvent {
                    event: RuntimeEvent::CompactionStarted,
                });
            let admission = self
                .inner
                .lifecycle
                .try_enter_running()
                .expect("the coordinator lock owns manual compaction admission");
            let completion = Arc::new(ManualCompactionCompletion::default());
            let completion_for_task = completion.clone();
            let inner = Arc::clone(&self.inner);
            drop(state);
            self.inner.executor.spawn(async move {
                let task = inner
                    .run_manual_compaction(
                        conversation,
                        context_runtime,
                        tools,
                        system_sections,
                        &cancellation,
                    )
                    .await;
                #[cfg(test)]
                {
                    let gate = inner
                        .probe
                        .lock()
                        .expect("coordinator probe lock poisoned")
                        .as_mut()
                        .and_then(|probe| probe.manual_compaction_settlement_gate.take());
                    if let Some(gate) = gate {
                        gate.enter();
                    }
                }
                inner.finish_manual_compaction(task, &completion_for_task);
                drop(admission);
            });
            completion
        };
        completion.wait().await
    }

    /// Submits one ordinary inbound message under an explicit provenance.
    ///
    /// This is the exact same admission path as
    /// [`ConversationRuntime::submit_inbound`] — the same lifecycle gate,
    /// the same durability-health gate, the same one durable acceptance
    /// linearization — with the provenance supplied by a trusted in-process
    /// owner instead of fixed to `Human`. The subagent child runtime driver
    /// (Issue #60) uses it to enter the delegated task with
    /// `UserSource::Agent(parent)`; IPC itself never appends anything.
    pub(crate) fn submit_sourced_inbound(
        &self,
        source: UserSource,
        content: Vec<UserContentBlock>,
    ) -> Result<InboundAdmission, InboundAdmissionError> {
        self.admit_sourced_inbound(source, content, SemanticInboundClass::Ordinary)
    }

    /// Submits one **parent-authored in-flight guidance** message into this
    /// child conversation (Issue #193).
    ///
    /// This is the child half of steering, and it is deliberately the same
    /// admission path as every other semantic inbound: the guidance becomes
    /// an ordinary durable inbound item that the ordinary Agent Loop adopts
    /// at its ordinary safe boundary. It carries no model, tool, skill,
    /// instruction, workspace, definition, or execution authority, so it can
    /// never re-author the child's frozen launch authority, and it never
    /// creates an attempt, a conversation, or a lifecycle of its own.
    ///
    /// # The acceptance contract
    ///
    /// Success means exactly: *the guidance is durably accepted into this
    /// child conversation's Pending Inbound Inbox, ahead of this
    /// conversation's terminal seal.* Because the seal is what lets the
    /// activation driver publish a terminal at all, it follows that a
    /// **naturally completing** child cannot publish an answer that
    /// predates this guidance.
    ///
    /// It does **not** mean the child model has observed it, that the
    /// in-flight provider request or tool call was interrupted, that
    /// anything changed yet, or that observation is guaranteed: a later
    /// cancellation of this activation, or physical loss of the child
    /// process, legitimately ends the activation with accepted guidance
    /// unobserved. Cancellation stays authoritative.
    ///
    /// The parent registry owns admission against stopping and interruption.
    /// Messages already admitted there must remain durably deliverable even
    /// after child-local cancellation. The child coordinator serializes the
    /// durable acceptance against these remaining refusal boundaries:
    ///
    /// - the conversation's terminal seal already committed
    ///   ([`InboundAdmissionError::GuidanceSealed`]);
    /// - the ordinary lifecycle/durability gates.
    ///
    /// The seal grant follows all previously admitted messages on the same
    /// FIFO control lane. Child cancellation cannot revoke their acceptance.
    ///
    /// Multiple accepted guidance messages preserve their acceptance order
    /// by the durable inbox's own `InboundSequence` domain; no scheduler
    /// ordering is involved.
    ///
    /// # Errors
    ///
    /// Returns the same variants as
    /// [`ConversationRuntime::submit_sourced_inbound`], plus
    /// [`InboundAdmissionError::GuidanceSealed`].
    pub(crate) fn submit_parent_guidance(
        &self,
        source: UserSource,
        content: Vec<UserContentBlock>,
    ) -> Result<InboundAdmission, InboundAdmissionError> {
        self.admit_sourced_inbound(source, content, SemanticInboundClass::ParentGuidance)
    }

    /// Commits this child conversation's **terminal seal** for
    /// parent-authored guidance (Issue #193), reports that semantic work is
    /// still owed, or fails closed.
    ///
    /// `observed_terminals` is how many attempt terminals the caller has
    /// already consumed. Under the one coordinator lock — the same lock that
    /// owns durable inbound acceptance — the seal commits only when the
    /// runtime **positively proves** that nothing can still carry accepted
    /// guidance into a model turn:
    ///
    /// - no admitted attempt is unobserved (`admitted_attempts` does not
    ///   exceed `observed_terminals`);
    /// - no attempt is live;
    /// - the durable Pending Inbound Inbox is *proven* empty.
    ///
    /// Because acceptance and the seal share that lock, a guidance accepted
    /// before the seal is necessarily visible to it — the seal then answers
    /// [`ParentGuidanceSeal::Open`] and the ordinary coordinator admits the
    /// turn that observes it — and a guidance arriving after the seal is
    /// necessarily refused. There is no interleaving in which a durably
    /// accepted guidance is silently discarded by a natural terminal.
    ///
    /// # Failing closed
    ///
    /// A durable read failure is **not** evidence that the inbox is empty,
    /// so it can never be folded into `Ok(false)`. The three answers are
    /// exactly:
    ///
    /// - `Ok(true)`  -> [`ParentGuidanceSeal::Open`]: semantic work remains;
    /// - `Ok(false)` -> eligible to seal;
    /// - `Err(..)`   -> [`ParentGuidanceSeal::DurabilityFailed`]: the seal
    ///   does **not** commit and the caller must not publish a successful
    ///   terminal.
    ///
    /// The failure is routed into the runtime's one absorbing
    /// durability-failure authority ([`DurabilityGate::commit_failure`], via
    /// [`DurableOperation::ParentGuidanceSeal`], which is non-transient by
    /// construction) before it is returned, so the degraded state is
    /// observable exactly once and the runtime rejects further durable work.
    ///
    /// The seal is absorbing for one activation. It is used by that activation
    /// driver; an ordinary interactive conversation never seals, because its
    /// coordinator simply admits the next attempt.
    ///
    /// # Who may seal
    ///
    /// Only a **steerable** normal asynchronous subagent child ever consults
    /// the seal. A Workflow-owned `AgentRun` (terminal mode
    /// [`crate::runtime::subagent::SubagentTerminalMode::WorkflowOutput`]) is
    /// structurally not steerable:
    /// the generic control plane refuses a steer before any `Guidance` frame
    /// exists, so no accepted guidance can ever be pending in its
    /// conversation. Its natural completion is therefore its terminal, and
    /// `serve_child_delegation` never calls this method for it — the
    /// steering-specific seal can never add a failure surface to Workflow
    /// execution. The isolation is decided from the child's frozen terminal
    /// mode (explicit ownership), never from incidental timing.
    pub(crate) fn gate_child_turns(&self) {
        let mut state = self.inner.lock_state();
        state.parent_turn_permits = Some(0);
    }

    pub(crate) fn release_child_turn(&self) {
        let mut state = self.inner.lock_state();
        if let Some(permits) = &mut state.parent_turn_permits {
            *permits = permits.saturating_add(1);
        }
        self.inner.wake.notify.notify_one();
    }

    pub(crate) async fn seal_parent_guidance(&self, observed_terminals: u64) -> ParentGuidanceSeal {
        // Test-only gate: parked before the coordinator lock, so a competing
        // guidance submission can still take that lock and durably accept
        // while the seal is provably pending. The gate handle is extracted
        // before the park so the probe mutex is not held while parked.
        #[cfg(test)]
        let seal_gate = self
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.parent_guidance_seal_gate.clone());
        #[cfg(test)]
        if let Some(gate) = seal_gate {
            tokio::task::block_in_place(|| gate.enter());
        }
        loop {
            // Armed before the state is read, so a settlement that fires
            // while the lock is held is never missed.
            let settled = self.inner.settlement.notified();
            tokio::pin!(settled);
            settled.as_mut().enable();
            {
                let mut state = self.inner.lock_state();
                if state.parent_guidance_sealed {
                    return ParentGuidanceSeal::Sealed;
                }
                if state.admitted_attempts > observed_terminals {
                    // An attempt exists whose terminal the caller has not
                    // consumed: accepted guidance may still be running.
                    return ParentGuidanceSeal::Open;
                }
                if state.current_attempt.is_none() {
                    // Accepted guidance still pending means the ordinary
                    // coordinator will admit the attempt that adopts it, so
                    // the child owes one more terminal observation.
                    //
                    // The seal needs a *positive* proof of emptiness. An
                    // unreadable inbox proves nothing, so it can never be
                    // read as "nothing pending": the runtime enters its
                    // absorbing durability-failure state and the caller is
                    // told the seal did not commit.
                    match self.inner.mailbox.has_pending() {
                        Ok(true) => return ParentGuidanceSeal::Open,
                        Ok(false) => {}
                        Err(error) => {
                            let diagnostic = format!(
                                "the child terminal seal could not verify the pending inbound inbox: {error}"
                            );
                            drop(state);
                            self.inner.commit_failure_and_observe(
                                DurableOperation::ParentGuidanceSeal,
                                diagnostic.clone(),
                            );
                            return ParentGuidanceSeal::DurabilityFailed { diagnostic };
                        }
                    }
                    state.parent_guidance_sealed = true;
                    return ParentGuidanceSeal::Sealed;
                }
            }
            // The observed attempt is still handing its state back; wait for
            // exactly that settlement rather than polling.
            settled.await;
        }
    }

    /// The runtime's one absorbing durability-failure fact, if committed
    /// (test-only): the same fact every durable admission gate reads.
    #[cfg(test)]
    pub(crate) fn durability_failure(&self) -> Option<crate::runtime::types::DurabilityFailure> {
        self.inner.durability_gate.failure()
    }

    /// How many times the terminal seal's pending-inbox probe
    /// ([`ConversationInboundMailbox::has_pending`]) has been invoked on
    /// this child's mailbox (test-only, Issue #193).
    ///
    /// [`ConversationInboundMailbox::has_pending`] has exactly one
    /// production caller — the seal in
    /// [`ConversationRuntime::seal_parent_guidance`] — so this counter is a
    /// direct non-invocation proof that the parent-guidance seal machinery
    /// was never consulted (for example by a Workflow-owned child, which
    /// must never enter the generic-steering terminal protocol).
    #[cfg(test)]
    pub(crate) fn seal_probe_calls(&self) -> usize {
        self.inner.mailbox.pending_probe_calls()
    }

    /// Arms `count` consecutive durable failures of the terminal seal's
    /// pending-inbox probe (test-only, Issue #193).
    ///
    /// It targets exactly [`ConversationInboundMailbox::has_pending`], which
    /// only the seal calls, so the fail-closed contract can be proven
    /// without arming a store-wide select fault that the coordinator's own
    /// admission loop would race for.
    #[cfg(test)]
    pub(crate) fn arm_seal_probe_failures(&self, count: usize) {
        self.inner.mailbox.arm_pending_probe_failures(count);
    }

    /// The shared admission path of every trusted in-process inbound
    /// producer.
    fn admit_sourced_inbound(
        &self,
        source: UserSource,
        content: Vec<UserContentBlock>,
        class: SemanticInboundClass,
    ) -> Result<InboundAdmission, InboundAdmissionError> {
        if content.is_empty() {
            return Err(InboundAdmissionError::EmptyContent);
        }
        #[cfg(test)]
        let submit_arrival = self
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.submit_arrival.clone());
        #[cfg(test)]
        if let Some(arrival) = submit_arrival {
            arrival.notify_one();
        }
        // Issue #63 (Finding 1): the one coordinator lock is held across the
        // lifecycle/shutdown check, the durability-failure check, **and**
        // the durable acceptance, so a successful acceptance and shutdown
        // have one total ordering. Shutdown therefore linearizes either
        // entirely before the acceptance (and the acceptance fails with
        // `Shutdown`) or entirely after it (and the acceptance is a legal
        // pre-shutdown success). Holding the coordinator lock here nests
        // only the mailbox/store and the DurabilityGate locks inside it, the
        // same order the admission worker already takes; no mailbox/store →
        // coordinator edge exists, so the lock graph stays acyclic. The
        // guard is deliberately kept alive (underscore binding) for the
        // whole acceptance even though the absorbing failure fact itself is
        // read from the DurabilityGate.
        let state = self.inner.lock_state();
        // The child conversation's guidance seal (Issue #193) is absorbing
        // and outranks every other admission gate: once it commits, no
        // parent-authored guidance can reach an Agent Loop boundary, and
        // saying so precisely is what makes the terminal race provable. It
        // is read here, inside the very critical section that performs the
        // durable acceptance below, so acceptance and the seal have one
        // total order. Only a native child activation seals its local inbox.
        if class.is_parent_guidance() && state.parent_guidance_sealed {
            return Err(InboundAdmissionError::GuidanceSealed);
        }
        match self.inner.lifecycle.state() {
            ConversationLifecycleState::Inactive => {
                return Err(InboundAdmissionError::Inactive);
            }
            ConversationLifecycleState::Draining | ConversationLifecycleState::Quiescent => {
                return Err(InboundAdmissionError::Shutdown);
            }
            ConversationLifecycleState::Running => {}
        }
        if let Some(failure) = self.inner.durability_gate.failure() {
            return Err(InboundAdmissionError::DurabilityFailed {
                message: failure.diagnostic,
            });
        }
        // The parent Agent registry owns message-versus-interrupt arbitration.
        // Envelopes already on its reliable lane won admission before stopping;
        // child cancellation must not retroactively reject their durable input.
        // Test-only gate: parked while holding the coordinator lock, after
        // the shutdown/activation decision and before the durable acceptance,
        // so a race regression can prove shutdown cannot slip between the
        // decision and the commit. The gate handle is extracted before the
        // park so the probe mutex is not held while parked.
        #[cfg(test)]
        let submit_gate = self
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.submit_gate.clone());
        #[cfg(test)]
        if let Some(gate) = submit_gate {
            gate.enter();
        }
        // The durable acceptance linearization point: the sequence, the
        // deterministic message identity, the pending record, and (when a
        // producer supplies one) the correlation commit here before success
        // is returned.
        let accepted = self
            .inner
            .mailbox
            .accept_draft(InboundDraft {
                message_id: None,
                source,
                kind: InboundKind::Message,
                content,
                timestamp: self.inner.clock.now(),
                correlation: None,
            })
            .map_err(InboundAdmissionError::Mailbox)?;
        drop(state);
        Ok(InboundAdmission {
            message_id: accepted.message_id,
            inbound_sequence: accepted.sequence,
        })
    }

    /// Requests cancellation of one named current attempt, durably pausing
    /// the Goal when that attempt is an autonomous Goal continuation.
    ///
    /// Acceptance is not terminal settlement: actual settlement remains
    /// owned by the Agent Loop and is observed asynchronously. The
    /// attempt-id check under the coordinator lock guarantees that the
    /// cancellation signal is never delivered to a *different* attempt that
    /// was admitted after the named one settled.
    ///
    /// # Ordering (Issue #351)
    ///
    /// Under the one coordinator lock, which also owns Goal-round admission
    /// and the Goal control commit boundary:
    ///
    /// 1. prove the named attempt is the current attempt and read its
    ///    runtime-owned [`AttemptProvenance`];
    /// 2. if — and only if — it is an autonomous Goal continuation, durably
    ///    commit `Active -> Paused` only while its admitted `GoalRef` is current;
    /// 3. request cancellation of that same attempt;
    /// 4. report success once the matching pause (if required) and cancellation
    ///    have won. Stale authority requires no pause and still succeeds.
    ///
    /// Because the durable pause commits **before** cancellation is
    /// requested, the matching authority cannot immediately restart. A newer
    /// Goal revision is left unchanged: the old attempt has no pause authority
    /// over later user intent. Settlement clears the named attempt under this
    /// same lock; a later interrupt returns `NoCurrentAttempt`.
    /// Because provenance decides step 2, cancelling an ordinary Human
    /// attempt never pauses an Active Goal merely because one exists — not
    /// even a Human attempt during which the model called `create_goal`.
    ///
    /// # Errors
    ///
    /// Returns [`CancelAttemptError::NoCurrentAttempt`] when no attempt with
    /// the given identity is currently cancellable, and
    /// [`CancelAttemptError::GoalPauseFailed`] when storage or lifecycle refusal
    /// prevents the pause operation. A stale/superseded `GoalRef` returning
    /// `Ok(None)` is success: no newer authority is substituted or retried.
    /// The failure case never fabricates a pause: cancellation of
    /// that exact attempt is still requested (containment), and a genuine
    /// storage failure is recorded through the runtime's own absorbing
    /// durability authority, which fences all further admission — so no later
    /// Goal round can be admitted even though the phase still reads `Active`.
    pub fn cancel_current_attempt(
        &self,
        attempt_id: &AttemptId,
    ) -> Result<AttemptId, CancelAttemptError> {
        let mut state = self.inner.lock_state();
        let Some(current) = state
            .current_attempt
            .as_ref()
            .filter(|current| current.attempt_id == *attempt_id)
        else {
            return Err(CancelAttemptError::NoCurrentAttempt);
        };
        let attempt_id = current.attempt_id.clone();
        let cancellation = current.cancellation.clone();
        let pause = match &current.provenance {
            AttemptProvenance::GoalContinuation(expected)
            | AttemptProvenance::RecoveredGoalContinuation(expected) => {
                // Composition gates future automation, not interrupt intent
                // for already-admitted work. Recovery can retain exact Goal
                // authority even when the reopened profile disables Goal.
                self.inner.tool_runtime.goal().map(|domain| {
                    self.inner
                        .mailbox
                        .with_running_commit(|| domain.pause_if_current(expected))
                })
            }
            _ => None,
        };
        let failure = match pause {
            None | Some(Ok(Ok(_))) => None,
            // A genuine durable failure: fail closed through the existing
            // absorbing runtime durability authority.
            Some(Ok(Err(error))) => {
                let diagnostic = error.to_string();
                self.inner.record_durability_failure(
                    &mut state,
                    DurableOperation::GoalPause,
                    diagnostic.clone(),
                );
                Some(diagnostic)
            }
            // The runtime no longer admits semantic commits, so it can no
            // longer admit a Goal round either. This is a lifecycle refusal,
            // not a storage fault, and earns no durability fact.
            Some(Err(error)) => Some(error.to_string()),
        };
        // Containment is unconditional: the interrupted attempt is cancelled
        // whether or not the durable pause committed.
        let _ = cancellation.request_cancel(CancellationReason::UserRequested);
        match failure {
            None => Ok(attempt_id),
            Some(diagnostic) => Err(CancelAttemptError::GoalPauseFailed { diagnostic }),
        }
    }

    /// Authoritative Conversation Goal state, independent of model Plugin selection.
    ///
    /// # Errors
    /// Returns a durable Goal read failure.
    pub fn goal_view(
        &self,
    ) -> Result<Option<crate::goal::GoalView>, crate::durable::ConversationStoreError> {
        self.inner
            .tool_runtime
            .goal()
            .map(crate::goal::GoalDomain::view)
            .transpose()
    }

    /// User control uses the existing coordinator and lifecycle commit boundary.
    ///
    /// # Errors
    /// Returns disabled, inactive, storage, transition, or stale-revision diagnostics.
    ///
    /// # Panics
    /// Panics only if a runtime synchronization lock was poisoned.
    pub fn control_goal(
        &self,
        control: crate::goal::GoalControl,
    ) -> Result<crate::goal::GoalView, String> {
        let state = self.inner.lock_state();
        let domain = self
            .inner
            .tool_runtime
            .goal()
            .ok_or("Goal state is unavailable")?;
        let write = match control {
            crate::goal::GoalControl::Show => {
                return domain.view().map_err(|error| error.to_string());
            }
            crate::goal::GoalControl::Create { objective, budget } => {
                crate::goal::GoalWrite::Create {
                    objective,
                    budget,
                    origin: crate::goal::GoalOrigin::RuntimeControl,
                }
            }
            crate::goal::GoalControl::Mutate { expected, mutation } => {
                crate::goal::GoalWrite::Mutate { expected, mutation }
            }
        };
        let committed = self
            .inner
            .mailbox
            .with_running_commit(|| domain.write(write))
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?
            .map_err(|rejection| {
                serde_json::to_string(&rejection).expect("Goal rejection serializes")
            })?;
        let view = domain.view().map_err(|error| error.to_string())?;
        drop(state);
        // Issue #351: a successful Create or Resume commits durable
        // authorization to continue, so the ordinary admission owner is woken
        // here — by runtime control, not by Goal code holding a wake handle.
        // If an attempt is already running the worker finds the runtime busy
        // and changes nothing; the settlement handoff then reaches the next
        // eligible idle boundary. This wake is a liveness notification only:
        // every eligibility question is re-answered under the coordinator
        // lock against durable state.
        if committed.phase == crate::goal::GoalPhase::Active {
            self.inner.wake.notify.notify_one();
        }
        Ok(view)
    }

    /// Commits a one-shot child cancellation intent into the runtime-owned
    /// cancellation state (Issue #60 child side).
    ///
    /// This is the subagent child control plane's `ParentFrame::Cancel`
    /// sink: the cancellation never waits for an observation to be
    /// delivered. Under the one coordinator lock that also owns attempt
    /// admission:
    ///
    /// - a current attempt exists ⇒ its [`AgentCancellation`] receives the
    ///   cancellation immediately, through the same M9b model-turn start
    ///   gate as every other cancellation request;
    /// - no attempt exists yet ⇒ the intent is retained as the coordinator's
    ///   one-shot sticky cancellation and the next admission consumes it, so
    ///   the admitted attempt starts already-cancelled and its first
    ///   model-turn arbitration resolves `CancelledBeforeStart`.
    ///
    /// The intent is sticky but one-shot: the child is a one-shot runtime,
    /// so at most one admission ever consumes it. The child control plane
    /// is its only producer; a parent conversation never arms it.
    ///
    /// Returns the cancelled current attempt identity, or `None` when the
    /// intent was retained for the next admission.
    pub(crate) fn cancel_current_or_next_attempt(
        &self,
        reason: CancellationReason,
    ) -> Option<AttemptId> {
        let mut state = self.inner.lock_state();
        if let Some(current) = &state.current_attempt {
            let _ = current.cancellation.request_cancel(reason);
            return Some(current.attempt_id.clone());
        }
        state.one_shot_cancel = Some(reason);
        None
    }

    /// Reads the authoritative session model catalog.
    #[must_use]
    pub fn model_catalog(&self) -> ModelCatalogView {
        self.inner.lock_state().model.catalog_view()
    }

    /// Reads the authoritative effective/desired `ApprovalMode` control state.
    #[must_use]
    pub fn approval_mode(&self) -> ApprovalMode {
        self.inner.lock_state().effective_approval_mode
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
    /// configuration and no observation is published. Product adapters that
    /// persist the selected configuration use the same transaction seam and
    /// persist before this live state is replaced.
    ///
    /// # Errors
    ///
    /// Returns [`ModelUpdateError::Inactive`] before activation,
    /// [`ModelUpdateError::DurabilityFailed`] while the runtime's durable
    /// authority is in the explicit failed state, and
    /// [`ModelUpdateError::InvalidConfiguration`] when the configuration
    /// cannot be resolved against the catalog or cannot run under the
    /// session context policy.
    pub fn model_set(
        &self,
        config: SessionModelConfig,
    ) -> Result<SessionModelView, ModelUpdateError> {
        self.model_set_with_persistence(config, |_| Ok(()))
    }

    /// Replaces the live model only after an optional product persistence
    /// callback has accepted the candidate configuration.
    ///
    /// The callback runs while the coordinator state is held, so a failure
    /// leaves both the live model and the catalog unchanged. This ordering is
    /// used by the native Session host to avoid reporting an error after a
    /// live model mutation has already taken effect.
    pub(crate) fn model_set_with_persistence(
        &self,
        config: SessionModelConfig,
        persist: impl FnOnce(SessionModelConfig) -> Result<(), ModelUpdateError>,
    ) -> Result<SessionModelView, ModelUpdateError> {
        use crate::local_runtime::configuration::application::{
            AdoptionError, PreparedConfiguration,
        };
        let (baseline, resources, mut candidate) = {
            let state = self.inner.lock_state();
            if !self.inner.lifecycle.is_running() {
                return Err(ModelUpdateError::Inactive);
            }
            if let Some(failure) = self.inner.durability_gate.failure() {
                return Err(ModelUpdateError::DurabilityFailed {
                    message: failure.diagnostic,
                });
            }
            (
                state.binding_revision,
                state.resources.clone(),
                state.model.clone(),
            )
        };
        candidate
            .apply(config.clone())
            .map_err(|error| invalid_model(&error))?;
        let policy = resources
            .configuration()
            .map_or(self.inner.context.policy, |generation| {
                generation.config.context_policy()
            });
        validate_context_policy(&policy, &candidate.snapshot())
            .map_err(|error| ModelUpdateError::InvalidConfiguration(error.message))?;
        let view = candidate.view();
        let mut prepared = Some(PreparedConfiguration {
            selection_only: true,
            owner: self.inner.configuration_owner.clone(),
            baseline,
            preview: resources,
            model: candidate,
            capability: None,
            resources: None,
            impact: crate::model::request_shape::CacheImpact::Unproven,
        });
        let mut persistence_error = None;
        let outcome = self.adopt_configuration(&mut prepared, baseline, true, || {
            persist(config).map_err(|error| {
                persistence_error = Some(error);
                AdoptionError::Failed {
                    diagnostic: "Session model persistence failed".into(),
                }
            })
        });
        outcome.map_err(|rejection| {
            persistence_error.unwrap_or(ModelUpdateError::ConfigurationAdoption { rejection })
        })?;
        Ok(view)
    }

    pub(crate) fn model_is_frozen(&self) -> bool {
        self.inner.lock_state().model.registry().is_none()
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

    pub(crate) fn has_current_attempt(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("runtime lock")
            .current_attempt
            .is_some()
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_configuration_admissions(&self) {
        self.inner.lifecycle.wait_for_no_admissions().await;
    }

    pub(crate) fn idle_epoch(&self) -> Result<u64, IdleBusyReason> {
        self.inner.idle_epoch_locked(&self.inner.lock_state())
    }

    /// Closes native admission through the existing shared drain owner. No
    /// busy owner is cancelled by an idle claim. Settlement remains asynchronous.
    pub(crate) fn claim_idle(&self, epoch: u64) -> bool {
        // Residency holds its own publication lock. Never wait there behind
        // coordinator storage work: contention is a conservative idle refusal.
        let Ok(state) = self.inner.state.try_lock() else {
            return false;
        };
        if state.current_attempt.is_some()
            || state.manual_compaction.is_some()
            || state.conversation.is_none()
            || state.recovered_continuation.is_some()
            || !self.inner.durability_gate.healthy_for_idle_claim()
            || state.mcp_settlement_failure.is_some()
            || state.admission_durability_cycle.pending_retry.is_some()
        {
            return false;
        }
        self.inner.lifecycle.begin_idle_drain(epoch)
    }

    /// Drains the conversation runtime to quiescence.
    ///
    /// Successful completion means the current attempt, all conversation-
    /// owned background executions, required durable terminal publication,
    /// counted subsystem commits, and the admission worker have settled. It
    /// is therefore stronger than cancellation request acceptance.
    ///
    /// # Lifecycle
    ///
    /// The `Running -> Draining` transition is linearized under the same
    /// coordinator lock as inbound acceptance and attempt admission. Repeated
    /// callers join the same drain completion, and only the first transition
    /// publishes [`ConversationObservation::Shutdown`].
    ///
    /// # Errors
    ///
    /// Returns [`ShutdownError::Inactive`] before activation. After
    /// activation, repeated shutdown calls are idempotent.
    ///
    /// `Ok(())` means exactly one thing: the lifecycle reached `Quiescent`,
    /// so no runtime-owned model, tool, background, capability, MCP,
    /// process, preparation, attempt task, or stale callback source can still
    /// produce an external effect or call back into the conversation.
    ///
    /// [`ShutdownError::RuntimeOwnedSettlement`] means admission is closed
    /// and supervision ran **every** settleable owner to its strongest
    /// available boundary, but rustX could not truthfully prove all required
    /// ownership/physical/durable terminal conditions. It never means
    /// supervision stopped early: a failure in one participant is collected
    /// as evidence and never releases the supervisor from a sibling that can
    /// still act.
    ///
    /// # Panics
    ///
    /// Panics only if the test-only coordinator probe lock is poisoned,
    /// which would mean a previous test hook panicked while holding it.
    pub async fn shutdown(&self) -> Result<(), ShutdownError> {
        // Test-only: signal that shutdown reached the point just before it
        // attempts the coordinator lock. Combined with the submit gate this
        // makes the submit-vs-shutdown ordering provable by mutex exclusion.
        #[cfg(test)]
        if let Some(arrival) = self
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.shutdown_arrival.clone())
        {
            arrival.notify_one();
        }
        let completion = self.inner.begin_drain()?;
        completion.wait().await?;
        self.inner.lifecycle.wait_until_quiescent().await;
        Ok(())
    }

    /// Returns a durable read handle for historical Request Snapshots.
    ///
    /// The returned value is a read-only handle over the durable request-fact
    /// authority; it does not retain another collection or create another
    /// conversation/transcript authority.
    #[must_use]
    pub fn request_history(&self) -> RequestHistory {
        RequestHistory::new(self.inner.store.clone())
    }

    /// Returns a durable read handle for the derived transcript.
    ///
    /// The handle retains no transcript rows. Each page resolves references
    /// through Pending Inbound, the Message Ledger, the publication plane, or
    /// the Event Journal at read time.
    #[must_use]
    pub fn transcript_history(&self) -> TranscriptHistory {
        TranscriptHistory::new(self.inner.store.clone())
    }

    /// Reads one bounded page of the derived transcript.
    ///
    /// `before` is an exclusive durable transcript cursor. With no cursor the
    /// newest page is returned; passing `next_cursor` walks older history.
    ///
    /// # Errors
    ///
    /// Returns the durable store error when the ordering spine or one of its
    /// canonical owners cannot be read coherently.
    pub fn transcript_page(
        &self,
        before: Option<crate::durable::TranscriptCursor>,
        limit: usize,
    ) -> Result<TranscriptPage, ConversationStoreError> {
        self.inner.store.load_transcript_page(before, limit)
    }

    /// Reads one exact historical canonical Surface revision through the
    /// durable `ConversationStore`. This is a materialization seam for the
    /// native Session layer: it returns evidence of the selected revision and
    /// never recomputes history with today's context or provider rules.
    ///
    /// The returned messages are a snapshot of the selected linear lineage;
    /// they carry no runtime lifecycle facts such as attempts, requests, or
    /// Event Journal ownership.
    ///
    /// # Errors
    ///
    /// Returns the durable store error when the requested revision is not
    /// retained or its canonical facts cannot be read.
    pub fn historical_surface_snapshot(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        self.inner.store.load_surface_snapshot(revision)
    }

    /// Reads the retained Surface operations through the selected revision,
    /// in revision order.
    ///
    /// This is the third part of a lineage copy. A Surface snapshot says what
    /// the model can see and the canonical history says what the conversation
    /// is; this says how the one became the other. A copy that dropped it
    /// would show the right Surface over a history that never happened, and
    /// the copy's *own* fork and tree boundaries are read out of that history
    /// — see [`crate::durable::LineageSeed`].
    ///
    /// # Errors
    ///
    /// Returns the durable store error when the selected revision is not
    /// retained or its operation history cannot be read.
    pub fn historical_surface_history(
        &self,
        through: SurfaceRevision,
    ) -> Result<Vec<crate::conversation::SurfaceOp>, ConversationStoreError> {
        self.inner.store.load_surface_history(through)
    }

    /// Reads the first retained Surface revision for each ordinary inbound
    /// user message through the selected revision. This is the native Session
    /// boundary read and avoids replaying and materializing every revision.
    ///
    /// # Errors
    ///
    /// Returns the durable store error when the selected revision or its
    /// canonical facts cannot be read.
    pub fn historical_user_message_boundaries(
        &self,
        through: SurfaceRevision,
    ) -> Result<Vec<SurfaceUserMessageBoundary>, ConversationStoreError> {
        self.inner.store.load_user_message_boundaries(through)
    }

    /// Reads one bounded page of historical user-message boundaries for the
    /// native Session tree projection.
    ///
    /// # Errors
    ///
    /// Returns the durable store error when the selected revision is not
    /// retained, its canonical facts cannot be read, or the page limit is
    /// invalid.
    pub fn historical_user_message_boundaries_page(
        &self,
        through: SurfaceRevision,
        offset: usize,
        limit: usize,
    ) -> Result<SurfaceUserMessageBoundaryPage, ConversationStoreError> {
        self.inner
            .store
            .load_user_message_boundaries_page(through, offset, limit)
    }

    /// Reads the current committed Surface head revision alone.
    ///
    /// A boundary page is selected against the head the reader is looking at,
    /// and materializing every Surface message to learn that one number would
    /// be a durable read the caller never uses.
    ///
    /// # Errors
    ///
    /// Returns the durable store error when the head cannot be read.
    pub fn historical_head_revision(&self) -> Result<SurfaceRevision, ConversationStoreError> {
        Ok(self.inner.store.load_head()?.revision)
    }

    /// Selects the current committed Surface head and materializes that exact
    /// revision. The head read is the clone linearization point; a later
    /// append or compaction creates a later revision and cannot mutate this
    /// selected historical meaning.
    ///
    /// # Errors
    ///
    /// Returns the durable store error when the head or selected Surface
    /// facts cannot be read.
    pub fn historical_head_snapshot(
        &self,
    ) -> Result<(SurfaceRevision, Vec<MessageBlock>), ConversationStoreError> {
        let head = self.inner.store.load_head()?;
        let messages = self.inner.store.load_surface_snapshot(head.revision)?;
        Ok((head.revision, messages))
    }

    /// Reads this conversation's complete durable canonical history, in
    /// Ledger commit order.
    ///
    /// This is the other half of a lineage copy. A Surface snapshot says what
    /// the model can currently see; this says what the conversation durably
    /// *is*, retired facts included. The two differ exactly when a compaction
    /// has run, and a copy that carried only the first half would inherit a
    /// compacted conversation's meaning and an uncompacted one's meaning
    /// differently — see [`crate::durable::LineageSeed`].
    ///
    /// The read races nothing it needs: every canonical row is immutable once
    /// committed, and the caller cuts this history at the exact Surface
    /// revision it selected, so facts committed afterwards are excluded by
    /// the cut rather than by the timing of this read.
    ///
    /// # Errors
    ///
    /// Returns the durable store error when canonical history cannot be read.
    pub fn historical_canonical_history(
        &self,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        self.inner.store.load_canonical()
    }

    /// Reconstructs one retained provider-neutral request from its durable
    /// snapshot, exact historical Surface revision, and keyed Ledger bodies.
    ///
    /// # Errors
    ///
    /// Returns a lookup or historical reconstruction error for an unknown or
    /// invalid request.
    pub fn reconstruct_request(
        &self,
        identity: &RequestIdentity,
    ) -> Result<ModelRequest, crate::runtime::request_history::RequestHistoryError> {
        RequestHistory::new(self.inner.store.clone()).reconstruct(identity)
    }

    /// The conversation-owned subagent registry (tests only).
    ///
    /// The FND-06 process-death suite needs the registry the *real*
    /// composition built so it can stage one child through
    /// [`SubagentRegistry::push_staged_override`](crate::runtime::subagent::SubagentRegistry)
    /// — the same seam the in-crate registry tests use — and then drive the
    /// ownership commit through the real Agent Loop and the real `subagent`
    /// intrinsic. It is never part of the published API.
    #[cfg(test)]
    pub(crate) fn subagents(&self) -> Option<&crate::runtime::subagent::SubagentRegistry> {
        self.inner.subagents.as_ref()
    }

    pub(crate) fn subagent_registry(&self) -> Option<&crate::runtime::subagent::SubagentRegistry> {
        self.inner.subagents.as_ref()
    }

    pub(crate) fn background_registry(
        &self,
    ) -> &crate::tools::background::ConversationBackgroundRegistry {
        self.inner.tool_runtime.background()
    }

    /// Inspects one subagent child through the authoritative registry
    /// (Issue #60).
    #[must_use]
    pub fn subagent_status(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
    ) -> Option<crate::runtime::subagent::SubagentSnapshot> {
        self.inner
            .subagents
            .as_ref()
            .and_then(|subagents| subagents.snapshot(subagent_id))
    }

    /// Resolve an exact owned child's read-only canonical store.
    pub(crate) fn subagent_transcript_store(
        &self,
        id: &crate::runtime::identity::SubagentId,
    ) -> Result<
        crate::durable::SqliteConversationStore,
        crate::runtime::subagent::SubagentTranscriptError,
    > {
        #[cfg(test)]
        self.inner
            .subagent_transcript_store_reads
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .subagents
            .as_ref()
            .ok_or_else(|| crate::runtime::subagent::SubagentTranscriptError::Unknown(id.clone()))?
            .transcript_store(id)
    }

    /// Requests cancellation of one subagent child through the
    /// authoritative registry. Acceptance and eventual settlement remain
    /// distinct; the snapshot is the state after the intent commit.
    #[must_use]
    pub fn subagent_cancel(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
    ) -> Option<crate::runtime::subagent::SubagentSnapshot> {
        self.inner
            .subagents
            .as_ref()
            .and_then(|subagents| subagents.cancel(subagent_id, CancellationReason::UserRequested))
    }

    /// Disposes the exact retained workspace of one terminal subagent through
    /// the registry/workspace resource lifecycle. The logical subagent state
    /// remains terminal; resource disposal never adds a logical `Disposed`
    /// state.
    ///
    /// # Errors
    ///
    /// Returns a typed disposal error when the subagent is unknown, non-terminal,
    /// has no proven retained resource, or the workspace backend cannot prove or
    /// remove the retained resource.
    pub async fn subagent_workspace_dispose(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
    ) -> Result<
        crate::runtime::subagent::SubagentWorkspaceDisposal,
        crate::runtime::subagent::SubagentWorkspaceDisposalError,
    > {
        let Some(subagents) = self.inner.subagents.as_ref() else {
            return Err(
                crate::runtime::subagent::SubagentWorkspaceDisposalError::UnknownSubagent {
                    subagent_id: subagent_id.clone(),
                },
            );
        };
        subagents.dispose_retained_workspace(subagent_id).await
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
        let reason = if matches!(
            self.inner.lifecycle.state(),
            ConversationLifecycleState::Draining | ConversationLifecycleState::Quiescent
        ) {
            CancellationReason::RuntimeShutdown
        } else {
            CancellationReason::UserRequested
        };
        self.inner
            .tool_runtime
            .background()
            .cancel_with_reason(execution_id, reason)
    }

    /// The authoritative lifecycle state of this runtime.
    #[must_use]
    pub fn lifecycle_state(&self) -> ConversationLifecycleState {
        self.inner.lifecycle.state()
    }

    /// Whether explicit drain has reached the terminal ownership boundary.
    #[must_use]
    pub fn is_quiescent(&self) -> bool {
        self.inner.lifecycle.state() == ConversationLifecycleState::Quiescent
    }

    /// The settlement handoff signal of this runtime: fired once per
    /// attempt settlement, so a headless driver can await the
    /// authoritative state transfer deterministically instead of by
    /// polling.
    #[must_use]
    pub fn settlement_signal(&self) -> &tokio::sync::Notify {
        &self.inner.settlement
    }

    /// The runtime-owned Message Ledger records, or `None` while an attempt
    /// owns the conversation state.
    ///
    /// This is a read-only handout of canonical state the runtime already
    /// owns between attempts; the subagent child driver (Issue #60) reads
    /// its final answer here after the attempt's canonical terminal event.
    #[must_use]
    /// Reads the canonical ledger from the durable authority.
    ///
    /// Every committed message is durable by definition, so a terminal
    /// observer (Issue #60's child result extraction) races nothing even
    /// while an attempt still owns the in-memory conversation state.
    pub(crate) fn durable_ledger(&self) -> Option<Vec<MessageBlock>> {
        self.inner.store.load_canonical().ok()
    }
}

/// The failure of a bounded local observation subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeObservationSubscriptionError {
    /// No Runtime Client host (or other primary consumer) installed the
    /// observation bridge yet.
    BridgeNotInstalled {
        /// The conversation whose observation stream was requested.
        conversation_id: ConversationId,
    },
    /// The runtime crossed its activation cut before the subscription was
    /// admitted, so adding it would create a gap in the live stream.
    RuntimeAlreadyActivated {
        /// The conversation whose observation stream was requested.
        conversation_id: ConversationId,
    },
}

impl core::fmt::Display for RuntimeObservationSubscriptionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BridgeNotInstalled { conversation_id } => write!(
                f,
                "the conversation runtime of {conversation_id} has no observation bridge"
            ),
            Self::RuntimeAlreadyActivated { conversation_id } => write!(
                f,
                "the conversation runtime of {conversation_id} is already activated; local observation subscriptions bind before activation"
            ),
        }
    }
}

impl std::error::Error for RuntimeObservationSubscriptionError {}

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
    /// The native durable authority could not provide a coherent bootstrap
    /// head or active working set.
    Durable(String),
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
            Self::Durable(message) => {
                write!(f, "durable conversation bootstrap failed: {message}")
            }
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
    pub journal_through: u64,
    pub goal: Option<crate::goal::GoalView>,
    /// The conversation identity.
    pub conversation_id: ConversationId,
    /// Whether runtime drain has begun and new admission is closed.
    pub shutting_down: bool,
    /// The current model-visible Surface at the cut. Historical Ledger rows
    /// are deliberately not hydrated into the client projection; callers
    /// needing them use the durable store's paged read APIs.
    pub messages: Vec<MessageBlock>,
    /// The bounded newest page of the derived transcript. Bodies remain
    /// owned by their canonical durable domains; this seed is only a read
    /// result for the Runtime Client bootstrap.
    pub transcript: TranscriptPage,
    /// The authoritative session model view.
    pub model: SessionModelView,
    /// The authoritative effective/desired `ApprovalMode` state at the cut.
    pub approval_mode: ApprovalMode,
    /// The currently pending inbound items, in mailbox sequence order.
    pub inbound_pending: Vec<InboundItem>,
    /// The authoritative background execution records at the cut.
    ///
    /// Provably empty: a `ConversationRuntime` is constructed only over a
    /// pristine tool-runtime background plane — the ownership transfer
    /// requires no prepared dispatch and no committed record and then
    /// refuses `commit_dispatch` while the mailbox is bound inactive — so
    /// no record can exist when the bridge is installed. The seed is
    /// captured anyway, in the same registry section that installs the
    /// observer, so the handshake is one coherent cut for whatever state
    /// exists.
    pub background: Vec<BackgroundExecutionSnapshot>,
    /// Durable Agent owner state and its exact latest committed activation,
    /// captured together under the registry lock. Recovered activation history
    /// never determines the latest Agent by iteration order.
    pub agents: Vec<(
        crate::runtime::subagent::AgentSnapshot,
        crate::runtime::subagent::SubagentSnapshot,
    )>,
    /// The active authoritative capability snapshot.
    pub capabilities: Arc<crate::capabilities::CapabilitySnapshot>,
    /// The authoritative capability-source availability at the cut
    /// (Issue #81).
    pub capability_availability: crate::capabilities::CapabilityAvailability,
    /// The immutable runtime resource generation current at the cut.
    pub resources: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
    /// Live process-owned native interactions at the bootstrap cut.
    pub pending_interactions: Vec<crate::runtime::interaction::RoutedInteraction>,
    /// The conversation's committed task list at the cut, when this runtime
    /// composes the Todo extension (Issue #259).
    ///
    /// The tool runtime rebuilt it from the whole canonical history at
    /// construction, so seeding it here is what lets a client that holds
    /// only the newest transcript page still show the current list. Every
    /// later change arrives as an ordinary committed `todo` result on the
    /// live observation stream.
    ///
    /// `None` means this runtime composes no Todo extension. It is
    /// deliberately distinct from `Some(empty)`: a client must present no
    /// current Todo surface at all, however many historical `todo` results
    /// the conversation's transcript still carries.
    pub todos: Option<crate::tools::todo::TodoSnapshot>,
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
    /// The conversation's parent-authored guidance admission is sealed
    /// (Issue #193): its terminal linearization point already committed, so
    /// no further semantic input can reach an Agent Loop boundary.
    GuidanceSealed,
    /// Runtime drain has begun: no further inbound admission occurs.
    Shutdown,
    /// Inbound content must not be empty.
    EmptyContent,
    /// The runtime's durable authority failed persistently: no new inbound
    /// work may be accepted until the runtime is reconstructed.
    DurabilityFailed {
        /// The human-readable failure diagnostic.
        message: String,
    },
    /// The authoritative mailbox rejected the message.
    Mailbox(MailboxError),
}

impl core::fmt::Display for InboundAdmissionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Inactive => f.write_str("the conversation runtime is not activated"),
            Self::GuidanceSealed => {
                f.write_str("the child conversation already committed its terminal seal")
            }
            Self::Shutdown => f.write_str("the conversation runtime is shutting down"),
            Self::EmptyContent => f.write_str("inbound content must not be empty"),
            Self::DurabilityFailed { message } => write!(
                f,
                "the conversation runtime durability authority failed: {message}"
            ),
            Self::Mailbox(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for InboundAdmissionError {}

/// The semantic class of one trusted in-process inbound submission.
///
/// The class exists only to select the extra admission gates that one class
/// carries; both classes share the same durable acceptance linearization
/// point, the same lifecycle and durability gates, and the same ordinary
/// Agent Loop consumption. There is deliberately no second inbound path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SemanticInboundClass {
    /// Every ordinary producer, including the child's own delegation.
    Ordinary,
    /// Parent-authored in-flight guidance for a running child (Issue #193),
    /// which additionally linearizes against the child conversation's
    /// terminal seal and committed cancellation intent.
    ParentGuidance,
}

impl SemanticInboundClass {
    const fn is_parent_guidance(self) -> bool {
        matches!(self, Self::ParentGuidance)
    }
}

/// The result of one child-conversation terminal seal evaluation (Issue
/// #193).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParentGuidanceSeal {
    /// The seal committed: no accepted guidance remains unobserved and none
    /// can be accepted afterwards. The child may report its terminal.
    Sealed,
    /// Semantic work is still owed — an unobserved admitted attempt, a live
    /// attempt, or accepted guidance still pending adoption. The child must
    /// observe another ordinary attempt terminal before it may seal.
    Open,
    /// The seal could not be *proven*: the durable Pending Inbound Inbox
    /// read failed, so the runtime cannot rule out an accepted, unadopted
    /// guidance. The runtime has committed its absorbing durability-failure
    /// fact; the child must fail closed and must not publish a successful
    /// terminal.
    DurabilityFailed {
        /// The bounded diagnostic of the failed durable read.
        diagnostic: String,
    },
}

/// A cancellation request failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelAttemptError {
    /// No attempt with the given identity is currently cancellable.
    NoCurrentAttempt,
    /// Storage or lifecycle refusal prevented an autonomous Goal attempt's
    /// pause operation. A superseded exact reference is an intentional no-op
    /// and successful cancellation, never this error.
    ///
    /// The attempt was still cancelled. Nothing pretends the Goal is paused:
    /// the durable phase is whatever storage actually holds, and a storage
    /// fault has additionally fenced further runtime admission.
    GoalPauseFailed {
        /// The durable or lifecycle diagnostic that refused the pause.
        diagnostic: String,
    },
}

/// Metadata returned after one manual compaction committed successfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualCompactionOutcome {
    /// The compaction generation derived from Surface history.
    pub generation: u64,
    /// The committed canonical summary identity.
    pub summary_message_id: MessageId,
    /// The Surface revision established by the replacement.
    pub surface_revision: SurfaceRevision,
    /// The request-context measurement before compaction.
    pub tokens_before: crate::runtime::types::TokenMeasurement,
    /// The deterministic request-context estimate after compaction.
    pub estimated_tokens_after: u64,
}

/// A manual context-compaction failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualCompactionError {
    /// The runtime has not been activated.
    Inactive,
    /// Runtime drain has begun.
    Shutdown,
    /// An attempt or another manual compaction currently owns the mutable
    /// conversation state.
    Busy,
    /// The runtime's durable authority is already failed.
    DurabilityFailed { message: String },
    /// Planning, summary generation, cancellation, or fit validation failed
    /// before a durable transition committed.
    Context(ContextError),
    /// The atomic durable compaction transition failed. This also moves the
    /// runtime into its absorbing durability-failed state.
    Durable { message: String },
}

impl core::fmt::Display for ManualCompactionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Inactive => formatter.write_str("the conversation runtime is not activated"),
            Self::Shutdown => formatter.write_str("the conversation runtime is shutting down"),
            Self::Busy => formatter
                .write_str("manual context compaction requires an idle conversation runtime"),
            Self::DurabilityFailed { message } => write!(
                formatter,
                "the conversation runtime durability authority failed: {message}"
            ),
            Self::Context(error) => formatter.write_str(&error.message),
            Self::Durable { message } => write!(
                formatter,
                "the compaction transition cannot be committed durably: {message}"
            ),
        }
    }
}

impl std::error::Error for ManualCompactionError {}

/// A session model update failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelUpdateError {
    /// Explicit model selection uses the same idle and baseline fences as adoption.
    ConfigurationAdoption {
        rejection: crate::local_runtime::configuration::application::AdoptionError,
    },
    /// The runtime has not been activated: a live semantic mutation of an
    /// inert conversation is refused and consumes nothing.
    Inactive,
    /// The configuration cannot be resolved against the catalog or cannot
    /// run under the session context policy.
    InvalidConfiguration(String),
    /// The runtime's durable authority failed persistently: no new durable
    /// mutation may begin until the runtime is reconstructed.
    DurabilityFailed {
        /// The human-readable failure diagnostic.
        message: String,
    },
    /// Product persistence rejected a valid live candidate before mutation.
    PersistenceFailed {
        /// The human-readable persistence diagnostic.
        message: String,
    },
    /// Product persistence crossed its catalog visibility commit point but
    /// could not prove the final durability barrier. The live candidate was
    /// not installed; the attachment must be replaced and rebuilt from the
    /// catalog authority.
    SessionRestartRequired {
        /// The bounded replacement diagnostic.
        message: String,
    },
}

/// A runtime `ApprovalMode` control update failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalModeUpdateError {
    /// The runtime has not been activated.
    Inactive,
    /// The runtime's durable authority failed persistently.
    DurabilityFailed {
        /// The human-readable failure diagnostic.
        message: String,
    },
}

/// Renders the collected settlement failures as one bounded deterministic
/// diagnostic.
///
/// The set is already in deterministic identity order, so the same
/// interleaving always yields the same diagnostic. This is a diagnostic
/// aggregation, not an error framework: shutdown has exactly one failure
/// variant and it carries exactly one bounded string.
fn aggregate_settlement_failures(failures: &std::collections::BTreeSet<String>) -> String {
    if failures.len() == 1 {
        return failures
            .iter()
            .next()
            .expect("a single-element set has one element")
            .clone();
    }
    format!(
        "{} runtime-owned settlement failures: {}",
        failures.len(),
        failures.iter().cloned().collect::<Vec<_>>().join("; ")
    )
}

/// A runtime shutdown failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownError {
    /// The runtime has not been activated: an inert conversation has no
    /// runtime lifecycle to end, so the request is refused and nothing is
    /// published.
    Inactive,
    /// Supervision reached every settleable owner's native terminal boundary,
    /// but at least one required ownership/physical/durable terminal
    /// condition stayed unproven, so the lifecycle remains `Draining` and
    /// successful quiescence is not claimed.
    ///
    /// The detail is a bounded deterministic aggregation of every collected
    /// failure, in identity order. It is never returned while a known
    /// runtime-owned operation is merely in flight and has not been awaited.
    RuntimeOwnedSettlement {
        /// The owner-provided settlement diagnostic.
        detail: String,
    },
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
pub(crate) fn validate_context_policy(
    policy: &SessionContextPolicy,
    model: &AttemptModelSnapshot,
) -> Result<(), crate::context::ContextError> {
    policy.validate_budgets(
        (
            model.primary().context_window(),
            model.primary().max_output_tokens(),
        ),
        (
            model.summary_invocation().context_window(),
            model.summary_invocation().max_output_tokens(),
        ),
    )
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

impl crate::goal::GoalObserver for RuntimeObserver {
    fn changed(&self, view: crate::goal::GoalView) {
        self.push(ConversationObservation::GoalChanged(view));
    }
}

impl AgentExecutionObserver for RuntimeObserver {
    fn observe_event(&self, attempt_id: &AttemptId, event: &RuntimeEvent, journal_sequence: u64) {
        self.push(ConversationObservation::Published {
            journal_sequence,
            observation: Box::new(ConversationObservation::Event {
                attempt_id: attempt_id.clone(),
                event: event.clone(),
            }),
        });
    }

    fn observe_committed(
        &self,
        attempt_id: &AttemptId,
        block: &MessageBlock,
        transcript_cursor: Option<crate::durable::TranscriptCursor>,
    ) {
        self.push(ConversationObservation::Committed {
            attempt_id: Some(attempt_id.clone()),
            block: block.clone(),
            transcript_cursor,
        });
    }

    fn observe_status(&self, observation: &AgentStatusObservation) {
        self.push(ConversationObservation::Status(observation.clone()));
    }

    fn observe_publication_opened(&self, attempt_id: &AttemptId, start: &PublicationStreamStart) {
        self.push(ConversationObservation::PublicationOpened {
            attempt_id: attempt_id.clone(),
            start: start.clone(),
        });
    }

    fn observe_publication(&self, attempt_id: &AttemptId, frame: &PublicationFrame) {
        self.push(ConversationObservation::Publication {
            attempt_id: attempt_id.clone(),
            frame: frame.clone(),
        });
    }

    fn observe_publication_settled(
        &self,
        attempt_id: &AttemptId,
        audit: &PublicationAudit,
        transcript_cursor: crate::durable::TranscriptCursor,
    ) {
        self.push(ConversationObservation::PublicationSettled {
            attempt_id: attempt_id.clone(),
            audit: Box::new(audit.clone()),
            transcript_cursor,
        });
    }

    // The loop fires this while the tool still executes; a leaf push into
    // the queue's disposable live-progress lane, exactly like the other
    // callbacks — no coordinator or projection lock is ever taken here.
    fn observe_tool_progress(
        &self,
        attempt_id: &AttemptId,
        tool_call_id: &crate::runtime::identity::ToolCallId,
        tool_id: &crate::runtime::identity::ToolId,
        progress: &crate::tools::types::ToolProgress,
    ) {
        self.push(ConversationObservation::ToolProgress {
            attempt_id: attempt_id.clone(),
            tool_call_id: tool_call_id.clone(),
            tool_id: tool_id.clone(),
            progress: progress.clone(),
        });
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

    fn on_pending_changed(&self, items: Vec<InboundItem>) {
        self.push(ConversationObservation::PendingInboundChanged(items));
    }

    fn on_drained(&self, batch: &InboundBatch) {
        self.push(ConversationObservation::InboundDrained(batch.clone()));
    }
}

// The registry fires `on_snapshot` while the registry lock is held. Push
// only, so no `coordinator -> registry` ordering discipline is ever
// required of a caller.
impl BackgroundObserver for RuntimeObserver {
    fn on_committed(&self, snapshot: &BackgroundExecutionSnapshot, sequence: u64) {
        self.push(ConversationObservation::Published {
            journal_sequence: sequence,
            observation: Box::new(ConversationObservation::Background(snapshot.clone())),
        });
    }
    fn on_snapshot(&self, snapshot: &BackgroundExecutionSnapshot) {
        self.push(ConversationObservation::Background(snapshot.clone()));
    }
}

// Same leaf contract as the background observer: the registry fires under
// its lock, so this only pushes into the queue. Lifecycle/identity
// publications are reliable; live-activity publications are disposable and
// land in the coalescing latest-value lane.
impl crate::runtime::subagent::SubagentObserver for RuntimeObserver {
    fn observe_agent_committed(
        &self,
        snapshot: &crate::runtime::subagent::AgentSnapshot,
        sequence: u64,
    ) {
        self.push(ConversationObservation::Published {
            journal_sequence: sequence,
            observation: Box::new(ConversationObservation::Agent {
                snapshot: Box::new(snapshot.clone()),
            }),
        });
    }
    fn observe_agent(&self, snapshot: &crate::runtime::subagent::AgentSnapshot) {
        self.push(ConversationObservation::Agent {
            snapshot: Box::new(snapshot.clone()),
        });
    }

    fn on_committed(&self, snapshot: &crate::runtime::subagent::SubagentSnapshot, sequence: u64) {
        self.push(ConversationObservation::Published {
            journal_sequence: sequence,
            observation: Box::new(ConversationObservation::SubagentLifecycle(snapshot.clone())),
        });
    }
    fn on_snapshot(&self, snapshot: &crate::runtime::subagent::SubagentSnapshot) {
        self.push(ConversationObservation::SubagentLifecycle(snapshot.clone()));
    }

    fn on_workspace(&self, snapshot: &crate::runtime::subagent::SubagentSnapshot) {
        self.push(ConversationObservation::SubagentWorkspace(snapshot.clone()));
    }

    fn on_activity(&self, snapshot: &crate::runtime::subagent::SubagentSnapshot) {
        self.push(ConversationObservation::SubagentActivity(snapshot.clone()));
    }

    fn on_interaction_pending(&self, interaction: &RoutedInteraction) {
        self.push(ConversationObservation::InteractionPending {
            interaction: interaction.clone(),
            audit: None,
        });
    }

    fn on_interaction_settled(&self, interaction: &InteractionRef, outcome: &InteractionOutcome) {
        self.push(ConversationObservation::InteractionSettled {
            interaction: interaction.clone(),
            outcome: outcome.clone(),
            audit: None,
        });
    }

    fn on_interaction_removed(&self, interaction: &InteractionRef) {
        self.push(ConversationObservation::InteractionRemoved {
            interaction: interaction.clone(),
        });
    }
}

// The coordinator fires `on_snapshot` while the capability state lock is
// held, with an attempt commit blocked behind it. Push only, so an
// authoritative capability commit never waits on the coordinator lock or
// the Runtime Client projection lock. The observation carries the
// authoritative `CapabilitySnapshot`; the Runtime Client projection owns
// the translation into its capability view.
impl CapabilityObserver for RuntimeObserver {
    fn on_snapshot(
        &self,
        snapshot: &CapabilitySnapshot,
        availability: &crate::capabilities::CapabilityAvailability,
    ) {
        self.push(ConversationObservation::Capability {
            snapshot: Arc::new(snapshot.clone()),
            availability: availability.clone(),
        });
    }
}

// Interaction pending publication fires while the coordinator's pending-state
// lock is held; terminal publication fires only after the waiter releases its
// callback authority and while the coordinator's counted settlement guard is
// held. Both callbacks are leaves: push only into the queue, never into the
// Runtime Client projection lock or conversation coordinator lock.
impl InteractionObserver for RuntimeObserver {
    fn on_pending(
        &self,
        request: &InteractionRequest,
        audit: &crate::events::types::RuntimeEventEnvelope,
        transcript_cursor: crate::durable::TranscriptCursor,
    ) {
        self.push(ConversationObservation::InteractionPending {
            interaction: RoutedInteraction::primary(request.clone()),
            audit: Some((audit.clone(), transcript_cursor)),
        });
    }

    fn on_settled(
        &self,
        interaction_id: &crate::runtime::identity::InteractionId,
        outcome: &InteractionOutcome,
        audit: Option<&(
            crate::events::types::RuntimeEventEnvelope,
            crate::durable::TranscriptCursor,
        )>,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        self.push(ConversationObservation::InteractionSettled {
            interaction: InteractionRef::new(inner.conversation_id.clone(), interaction_id.clone()),
            outcome: outcome.clone(),
            audit: audit.cloned(),
        });
    }
}

#[cfg(test)]
impl ConversationRuntime {
    /// Runs the ordinary coordinator admission boundary synchronously in a test.
    pub(crate) fn admit_now_for_test(&self) {
        self.inner.admit_next_attempt();
    }

    /// Commits a synthetic durable-authority failure through the exact
    /// production `record_durability_failure` path (Issue #60 regression
    /// seam).
    ///
    /// The given operation is committed through the one authority: a
    /// non-transient operation enters `DurabilityFailed` immediately and
    /// marks the shared durability frontier (`DurabilityGate`) failed under
    /// the same commit — exactly what a real exhaustion (subagent or
    /// background terminal publication) performs. Tests use it to force the
    /// health-failure side of the ownership-vs-failure total order
    /// deterministically.
    pub(crate) fn force_durability_failure_for_test(
        &self,
        operation: DurableOperation,
        diagnostic: &str,
    ) {
        let mut state = self.inner.lock_state();
        self.inner
            .record_durability_failure(&mut state, operation, diagnostic.to_owned());
    }

    /// The settled canonical ledger of this conversation, or `None` while
    /// an attempt owns the in-memory conversation state.
    pub(crate) fn settled_ledger(&self) -> Option<Vec<MessageBlock>> {
        self.inner
            .state
            .lock()
            .expect("runtime lock")
            .conversation
            .as_ref()
            .map(|conversation| conversation.ledger().audit_records().to_vec())
    }

    /// The runtime-owned Message Ledger records, or `None` while an attempt
    /// owns the conversation state.
    pub(crate) fn coordinator_ledger(&self) -> Option<Vec<MessageBlock>> {
        self.settled_ledger()
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

    /// Whether idle maintenance currently owns the conversation state.
    #[cfg(test)]
    pub(crate) fn has_manual_compaction(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("runtime lock")
            .manual_compaction
            .is_some()
    }

    /// A non-owning handle to the shared runtime state, for lifetime tests.
    pub(crate) fn weak_inner(&self) -> Weak<RuntimeInner> {
        Arc::downgrade(&self.inner)
    }

    /// Installs one test-only pre-tool policy for the next runtime-created
    /// attempt. Production derives the policy from the admitted effective
    /// `ApprovalMode`; this hook exists solely to exercise the real
    /// `ConversationRuntime` ownership and drain path without exposing a
    /// public policy factory.
    #[cfg(test)]
    pub(crate) fn install_test_pre_tool_policy(
        &self,
        policy: Arc<dyn crate::agent::PreToolPolicy>,
    ) {
        *self
            .inner
            .test_pre_tool_policy
            .lock()
            .expect("test pre-tool policy lock") = Some(policy);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    fn fixture_activation(
        runtime: &crate::tools::runtime::ConversationToolRuntime,
    ) -> crate::capabilities::AgentActivation {
        let mut activation = crate::capabilities::AgentActivation::default();
        activation.profile.extensions =
            crate::scripted_suites::common::plugin_document(runtime.extensions());
        activation.profile.skills = Some(crate::runtime::agent_profile::AgentSkillSelection::All);
        activation
    }

    /// A Builtin-only frozen named-agent specification for registry-level
    /// tests: resolution is the resolver's concern, so a registry test
    /// supplies its already-frozen result.
    fn test_resolved_subagent(agent: &str) -> crate::runtime::subagent::ResolvedSubagentSpec {
        crate::runtime::subagent::ResolvedSubagentSpec {
            environment: Vec::new(),
            generation: crate::runtime::identity::RuntimeResourceRevision::new(1),
            skill_roots: Vec::new(),

            selection: crate::runtime::agent_profile::FrozenAgentSelection::default(),
            agent: crate::runtime::subagent::SubagentName::parse(agent).expect("canonical name"),
            definition_digest: serde_json::from_value(serde_json::json!(
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
            ))
            .expect("digest"),
            execution_deadline: None,
            workspace_policy: crate::runtime::workspace::WorkspacePolicy::SharedWorkspace,
            instructions: "instructions".to_owned(),
            model: crate::model::frozen::test_frozen_model_spec(
                serde_json::from_value(serde_json::json!("local/model")).expect("model reference"),
            ),
            tools: Vec::new(),
            skills: Vec::new(),
            project_instructions: Vec::new(),
            materialization:
                crate::runtime::subagent::resolver::ResolvedSubagentMaterialization::default(),
            extensions: crate::extensions::NativeAgentExtensionsDocument::default().resolve(),
        }
    }

    use super::{
        CancelAttemptError, ConversationContextConfig, ConversationRuntime,
        ConversationRuntimeError, CoordinatorProbe, Gate, InboundAdmissionError,
        ManualCompactionError, ModelUpdateError, PendingObservations, RuntimeConversationConfig,
    };
    use crate::agent::{LifecycleError, PreToolDecision, PreToolPolicy, PreToolView};
    use crate::context::{
        AgentStatusEngine, ClosureTokenEstimator, ContextError, ContextErrorKind,
        DefaultTokenEstimator, TokenEstimator,
    };
    use crate::conversation::SurfaceSpan;
    use crate::durable::inbox::{CompactionCommitInput, ConversationStore};
    use crate::events::types::RuntimeEvent;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        CompactionSummaryMetadata, InboundKind, MessageBlock, UserContentBlock, UserSource,
    };
    use crate::model::adapter::ModelAdapter;
    use crate::runtime::ApprovalMode;
    #[cfg(unix)]
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::identity::{
        AgentId, AttemptId, ConversationId, SubagentId, ToolCallId, ToolId, TurnId,
    };
    use crate::runtime::interaction::{
        ApprovalDecision, InteractionOutcome, InteractionResponse, InteractionSettleGate,
        InteractionWaitCancellationGate,
    };
    use crate::runtime::observation::ConversationObservation;
    use crate::runtime::request_history::RequestHistory;
    use crate::runtime::types::{
        CancellationReason, ConversationLifecycleState, DurableOperation, TokenMeasurement,
        TokenMeasurementSource,
    };
    use crate::runtime_client::event::RuntimeClientEvent;
    use crate::runtime_client::host::{EventDelivery, RuntimeClientHost, RuntimeClientHostConfig};
    use crate::runtime_client::types::{
        RUNTIME_CLIENT_PROTOCOL_VERSION, RequestId, RuntimeClientError, RuntimeClientProtocolEvent,
        RuntimeClientRequest, RuntimeClientResult,
    };
    use crate::scripted_suites::support::fake::{
        FakeModel, FakeStep, FakeTool, model_release, success_result,
    };
    use crate::scripted_suites::support::model::scripted_session_model;
    use crate::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
    use crate::tools::types::{
        ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionStatus,
        ToolInvocation, ToolOrigin, ToolReplayPolicy,
    };
    use futures_util::future::BoxFuture;

    /// Adopts one accepted inbound item with the durable answer obligation the
    /// adoption transaction requires.
    fn adopt_accepted(
        store: &dyn ConversationStore,
        accepted: &crate::durable::inbox::AcceptedInbound,
    ) {
        store
            .adopt_pending_batch(accepted.sequence, None)
            .expect("adopt");
    }

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

    struct MutableResourceLoader {
        project_files: std::sync::Mutex<
            Result<
                Vec<crate::runtime::ProjectContextFile>,
                crate::runtime::RuntimeResourceLoadError,
            >,
        >,
        capability_inputs: std::sync::Mutex<Option<crate::capabilities::CapabilityResourceInputs>>,
        context_assembly: std::sync::Mutex<crate::context::ContextAssembly>,
        workflow_catalog: std::sync::Mutex<crate::runtime::WorkflowCatalog>,
        candidate_close_probe:
            std::sync::Mutex<Option<Arc<crate::tools::mcp::test_sync::CloseProbe>>>,
        prepare_count: std::sync::atomic::AtomicU64,
    }

    impl MutableResourceLoader {
        fn new(project_files: Vec<crate::runtime::ProjectContextFile>) -> Self {
            Self {
                project_files: std::sync::Mutex::new(Ok(project_files)),
                capability_inputs: std::sync::Mutex::new(None),
                context_assembly: std::sync::Mutex::new(crate::context::ContextAssembly::new()),
                workflow_catalog: std::sync::Mutex::new(crate::runtime::WorkflowCatalog::empty()),
                candidate_close_probe: std::sync::Mutex::new(None),
                prepare_count: std::sync::atomic::AtomicU64::new(0),
            }
        }

        fn set_capability_inputs(&self, inputs: crate::capabilities::CapabilityResourceInputs) {
            *self.capability_inputs.lock().expect("capability inputs") = Some(inputs);
        }

        fn prepare_count(&self) -> u64 {
            self.prepare_count
                .load(std::sync::atomic::Ordering::Acquire)
        }
    }

    impl crate::runtime::RuntimeResourceLoader for MutableResourceLoader {
        fn prepare<'a>(
            &'a self,
            capability: &'a crate::capabilities::CapabilityCoordinator,
            _capture: Option<crate::local_runtime::configuration::ProspectiveSessionConfig>,
            _preparation_cancellation: &'a crate::runtime::cancellation::CancellationSignal,
        ) -> BoxFuture<
            'a,
            Result<
                crate::runtime::PreparedRuntimeResources,
                crate::runtime::RuntimeResourceLoadError,
            >,
        > {
            Box::pin(async move {
                self.prepare_count
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                let inputs = self
                    .capability_inputs
                    .lock()
                    .expect("capability inputs")
                    .clone();
                let context_assembly = self
                    .context_assembly
                    .lock()
                    .expect("context assembly")
                    .clone();
                let candidate = match inputs {
                    Some(inputs) => capability.prepare_candidate_with_inputs(inputs).await,
                    None => capability.prepare_candidate().await,
                }
                .map_err(|error| {
                    crate::runtime::RuntimeResourceLoadError::new(error.to_string())
                })?;
                if let Some(probe) = self
                    .candidate_close_probe
                    .lock()
                    .expect("candidate close probe")
                    .clone()
                {
                    candidate.install_mcp_close_probe(&probe);
                }
                // Candidate preparation deliberately precedes project-context
                // loading so a later project-file failure exercises the same
                // candidate-retirement path as the production loader.
                let project_files = self.project_files.lock().expect("project files").clone()?;
                let workflow_catalog = self
                    .workflow_catalog
                    .lock()
                    .expect("workflow catalog")
                    .clone();
                Ok(crate::runtime::PreparedRuntimeResources::new(
                    project_files,
                    None,
                    context_assembly,
                    candidate,
                )
                .with_workflow_catalog(workflow_catalog))
            })
        }
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

    /// A headless runtime fixture: the conversation runtime with zero
    /// Runtime Client attachments, over a scripted model adapter and an
    /// optional runtime-owned observation bridge the test folds itself.
    struct HeadlessFixture {
        _dir: tempfile::TempDir,
        runtime: ConversationRuntime,
        model: Arc<FakeModel>,
        pending: Option<Arc<PendingObservations>>,
    }

    /// The outer liveness guard of a deterministic ordering proof: the
    /// synchronization below is exact, so this only turns a defect that
    /// breaks the ordering into a failure instead of a hang.
    async fn within_liveness_guard<F: std::future::Future>(label: &str, future: F) -> F::Output {
        tokio::time::timeout(std::time::Duration::from_mins(1), future)
            .await
            .unwrap_or_else(|_| panic!("liveness guard exceeded while waiting for {label}"))
    }

    fn text_content(text: &str) -> Vec<UserContentBlock> {
        vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })]
    }

    /// Test-only policy used by the real `ConversationRuntime` interaction
    /// shutdown regression. It is deliberately a concrete one-shot policy:
    /// production runtime construction derives approval from its effective
    /// `ApprovalMode` and has no policy factory in its public configuration.
    struct RuntimeAskPolicy;

    impl PreToolPolicy for RuntimeAskPolicy {
        fn evaluate<'a>(
            &'a self,
            _view: &'a PreToolView<'a>,
        ) -> BoxFuture<'a, Result<PreToolDecision, LifecycleError>> {
            Box::pin(async {
                Ok(PreToolDecision::Ask {
                    reason: "runtime shutdown regression".to_owned(),
                })
            })
        }
    }

    fn one_turn_script() -> Vec<FakeStep> {
        text_turn_script("done")
    }

    fn text_turn_script(text: &str) -> Vec<FakeStep> {
        use crate::message::types::ContentBlockIndex;
        use crate::model::event::ModelEvent;
        use crate::model::finish::ModelFinishReason;
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: text.to_owned(),
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
        headless_runtime_with_options(
            dir,
            scripts,
            base_tool_registry,
            probe,
            HeadlessRuntimeOptions::default(),
        )
        .await
    }

    struct HeadlessRuntimeOptions {
        skill_discovery: crate::skills::SkillDiscoveryConfig,
        estimator: Arc<dyn TokenEstimator>,
        policy: crate::context::SessionContextPolicy,
        status_engine: AgentStatusEngine,
        initial_messages: Vec<MessageBlock>,
        project_context_files: Vec<crate::runtime::ProjectContextFile>,
        agent_profile: Option<String>,
        resource_loader: Option<Arc<dyn crate::runtime::RuntimeResourceLoader>>,
        workflow_catalog: crate::runtime::WorkflowCatalog,
        mcp_servers: std::collections::BTreeMap<
            crate::runtime::identity::McpServerId,
            crate::tools::mcp::McpServerBinding,
        >,
        pre_activation_mcp_close_probe: Option<Arc<crate::tools::mcp::test_sync::CloseProbe>>,
    }

    impl Default for HeadlessRuntimeOptions {
        fn default() -> Self {
            Self {
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
                estimator: Arc::new(DefaultTokenEstimator),
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                status_engine: AgentStatusEngine::default(),
                initial_messages: Vec::new(),
                project_context_files: Vec::new(),
                agent_profile: None,
                resource_loader: None,
                workflow_catalog: crate::runtime::WorkflowCatalog::empty(),
                mcp_servers: std::collections::BTreeMap::new(),
                pre_activation_mcp_close_probe: None,
            }
        }
    }

    /// Skill context fixtures explicitly admit the native lazy-load dependency.
    fn with_native_read(mut registry: ToolRegistry) -> ToolRegistry {
        let read = crate::tools::native::subagent_child_definition(
            "read",
            crate::tools::NativeToolPolicies::default().read,
        )
        .unwrap();
        crate::tools::native::register_subagent_child_tools(&mut registry, &[read]).unwrap();
        registry
    }

    /// The configurable headless fixture variant used by exact context-input
    /// regressions without changing the defaults of the broad runtime suite.
    #[allow(
        clippy::too_many_lines,
        reason = "complete headless runtime fixture composition"
    )]
    async fn headless_runtime_with_options(
        dir: &tempfile::TempDir,
        scripts: Vec<Vec<FakeStep>>,
        base_tool_registry: Option<crate::tools::executor::ToolRegistry>,
        probe: Option<CoordinatorProbe>,
        options: HeadlessRuntimeOptions,
    ) -> (ConversationRuntime, Arc<FakeModel>) {
        let conversation_id = ConversationId::new("conv_d9e0ddde-2c51-755b-8a4e-1932c402870e");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
                crate::context::AgentStatusConfig::default(),
            ))
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_todo()
                    .and_agent_status(options.status_engine.config().clone()),
            ),
        )
        .expect("tool runtime");
        let base_tool_registry = base_tool_registry.unwrap_or_default();
        let selected_builtin = base_tool_registry
            .definitions()
            .into_iter()
            .filter(|tool| tool.origin.source().is_none())
            .map(|tool| tool.name.clone())
            .collect();
        let source_selection: std::collections::BTreeMap<_, _> = base_tool_registry
            .definitions()
            .into_iter()
            .filter_map(|definition| definition.origin.source())
            .chain(
                options
                    .mcp_servers
                    .keys()
                    .cloned()
                    .map(crate::capabilities::ToolSourceId::Mcp),
            )
            .map(|source| {
                (
                    source,
                    crate::capabilities::selection::SourceToolSelection::All,
                )
            })
            .collect();
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::new(
                    options
                        .mcp_servers
                        .keys()
                        .cloned()
                        .map(crate::capabilities::ToolSourceId::Mcp),
                    crate::runtime::resources::ManagedPythonCatalog::default(),
                ),
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(base_tool_registry),
                extension_tools: tool_runtime.extension_tool_plane(),
                agent_activation: crate::capabilities::AgentActivation {
                    profile: crate::local_runtime::config::AgentProfileDocument {
                        tools: crate::capabilities::selection::ToolSelectionDocument {
                            builtin: selected_builtin,
                            sources: source_selection,
                        },
                        ..fixture_activation(&tool_runtime).profile
                    },
                    ..Default::default()
                },
                skill_discovery: options.skill_discovery,
                mcp_servers: options.mcp_servers.clone(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        if let Some(probe) = options.pre_activation_mcp_close_probe.clone() {
            for server_id in options.mcp_servers.keys() {
                if let Some(runtime) = coordinator.current_mcp_runtime(server_id) {
                    runtime.install_close_probe(probe.clone());
                }
            }
            coordinator.retire_current_mcp_runtimes();
            assert!(
                coordinator.settle_ready_mcp_runtimes().await.is_err(),
                "the pre-activation close probe must publish an authoritative failure"
            );
        }
        let model = Arc::new(FakeModel::new(scripts));
        let adapter: Arc<dyn ModelAdapter> = model.clone();
        let resources = Arc::new(
            crate::runtime::RuntimeResourceSnapshot::new(
                crate::runtime::RuntimeResourceRevision::new(1),
                options.project_context_files,
                options.agent_profile,
                crate::context::ContextAssembly::new(),
                coordinator.current_snapshot(),
            )
            .with_workflow_catalog(options.workflow_catalog),
        );
        let resource_loader = options
            .resource_loader
            .unwrap_or_else(|| test_resource_loader(&coordinator));
        let config = RuntimeConversationConfig {
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter),
            approval_mode: ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: options.policy,
                estimator: options.estimator,
                status_engine: Some(options.status_engine),
            },
            tool_runtime,
            resources,
            resource_loader,
            capability: coordinator,
            clock: None,
            initial_messages: options.initial_messages,
            subagents: None,
            workflow_output: None,
        };
        let runtime = match probe {
            Some(probe) => ConversationRuntime::with_probe(config, probe).expect("runtime"),
            None => ConversationRuntime::new(config).expect("runtime"),
        };
        (runtime, model)
    }

    /// Builds the conversation runtime of one headless fixture over a supplied
    /// durable authority. The runtime derives its mailbox from this same
    /// authority binding.
    async fn headless_runtime_over_store(
        dir: &tempfile::TempDir,
        conversation_id: &str,
        store: Arc<dyn ConversationStore>,
    ) -> (ConversationRuntime, Arc<FakeModel>) {
        headless_runtime_over_store_with(dir, conversation_id, store, vec![one_turn_script()], None)
            .await
    }

    /// The same durable-authority fixture with explicit model scripts and an
    /// optional coordinator probe, for the M9c supervision regressions.
    async fn headless_runtime_over_store_with(
        dir: &tempfile::TempDir,
        conversation_id: &str,
        store: Arc<dyn ConversationStore>,
        scripts: Vec<Vec<FakeStep>>,
        probe: Option<CoordinatorProbe>,
    ) -> (ConversationRuntime, Arc<FakeModel>) {
        headless_runtime_over_store_with_policy(
            dir,
            conversation_id,
            store,
            scripts,
            probe,
            crate::model::ModelTimeoutPolicy::default(),
        )
        .await
    }

    /// The same durable-authority fixture with an explicit model deadline.
    /// Long-lived parked-provider tests use this to keep their deterministic
    /// channel/barrier ordering independent of full-suite wall-clock load.
    async fn headless_runtime_over_store_with_policy(
        dir: &tempfile::TempDir,
        conversation_id: &str,
        store: Arc<dyn ConversationStore>,
        scripts: Vec<Vec<FakeStep>>,
        probe: Option<CoordinatorProbe>,
        model_timeout_policy: crate::model::ModelTimeoutPolicy,
    ) -> (ConversationRuntime, Arc<FakeModel>) {
        let conversation_id = ConversationId::new(conversation_id);
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig {
                durable_binding: Some(crate::durable::ConversationStoreBinding::new(store.clone())),
                ..crate::tools::runtime::ConversationRuntimeConfig::new(
                    &workspace,
                    dir.path().join("artifacts"),
                )
                .with_extensions(
                    crate::extensions::NativeAgentExtensions::with_agent_status(
                        crate::context::AgentStatusConfig::default(),
                    ),
                )
            },
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
                extension_tools: tool_runtime.extension_tool_plane(),
                agent_activation: fixture_activation(&tool_runtime),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
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
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter),
            approval_mode: ApprovalMode::Policy,
            model_timeout_policy,
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_engine: Some(AgentStatusEngine::default()),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator,
            clock: None,
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        };
        let runtime = match probe {
            Some(probe) => ConversationRuntime::with_probe(config, probe).expect("runtime"),
            None => ConversationRuntime::new(config).expect("runtime"),
        };
        (runtime, model)
    }

    /// Builds the same headless runtime fixture with a conversation-owned
    /// subagent registry, so the runtime-level durability sink is exercised
    /// instead of only the registry's isolated publication policy.
    #[allow(
        clippy::too_many_lines,
        reason = "one complete deterministic fixture boundary"
    )]
    async fn headless_runtime_over_store_with_subagents(
        dir: &tempfile::TempDir,
        conversation_id: &str,
        store: Arc<dyn ConversationStore>,
        admission_gate: Option<Arc<super::Gate>>,
    ) -> (
        ConversationRuntime,
        Arc<FakeModel>,
        crate::runtime::subagent::SubagentRegistry,
    ) {
        let conversation_id = ConversationId::new(conversation_id);
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig {
                durable_binding: Some(crate::durable::ConversationStoreBinding::new(store)),
                ..crate::tools::runtime::ConversationRuntimeConfig::new(
                    &workspace,
                    dir.path().join("artifacts"),
                )
                .with_extensions(
                    crate::extensions::NativeAgentExtensions::with_agent_status(
                        crate::context::AgentStatusConfig::default(),
                    ),
                )
            },
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
                extension_tools: tool_runtime.extension_tool_plane(),
                agent_activation: fixture_activation(&tool_runtime),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let subagents = crate::runtime::subagent::SubagentRegistry::new(
            crate::runtime::subagent::SubagentRegistryConfig {
                conversation_id: conversation_id.clone(),
                agent_id: AgentId::new("agent-a"),
                mailbox: tool_runtime.mailbox(),
                clock: Arc::new(crate::runtime::types::SystemClock),
                monotonic_clock: Arc::new(crate::runtime::ManualMonotonicClock::new()),
                spawn: crate::runtime::subagent::SubagentSpawnPlan {
                    session_id: crate::runtime::identity::SessionId::new(
                        "ses_01900000-0000-7000-8000-000000000001",
                    ),

                    program: std::path::PathBuf::from("/nonexistent/rustx"),
                    product_root: crate::runtime::local_storage::ProductRoot::create(
                        &dir.path().join("subagents"),
                    )
                    .expect("product root"),
                },
                workspace: crate::runtime::workspace::WorkspaceManager::new(
                    &workspace,
                    dir.path().join("subagents"),
                ),
                max_active: 4,
            },
        );
        let model = Arc::new(FakeModel::new(Vec::new()));
        let adapter: Arc<dyn ModelAdapter> = model.clone();
        let config = RuntimeConversationConfig {
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter),
            approval_mode: ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator: Arc::new(DefaultTokenEstimator),
                status_engine: Some(AgentStatusEngine::default()),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator,
            clock: None,
            initial_messages: Vec::new(),
            subagents: Some(subagents.clone()),
            workflow_output: None,
        };
        let runtime = match admission_gate {
            Some(admission_gate) => ConversationRuntime::with_probe(
                config,
                CoordinatorProbe {
                    admission_gate: Some(admission_gate),
                    ..CoordinatorProbe::default()
                },
            )
            .expect("runtime"),
            None => ConversationRuntime::new(config).expect("runtime"),
        };
        (runtime, model, subagents)
    }

    /// A runtime construction config over one store, with a subagent
    /// registry built over the same tool runtime and a deliberately
    /// configurable registry identity (ownership-domain validation tests).
    /// Returns the registry alongside the config so a test can commit
    /// children into it before attempting construction.
    #[allow(clippy::too_many_lines)]
    async fn subagent_runtime_config_with_registry(
        dir: &tempfile::TempDir,
        conversation_id: &str,
        store: Arc<dyn ConversationStore>,
        registry_conversation: &ConversationId,
        registry_agent: &AgentId,
        runtime_agent: &AgentId,
    ) -> (
        crate::runtime::subagent::SubagentRegistry,
        RuntimeConversationConfig,
    ) {
        let conversation_id = ConversationId::new(conversation_id);
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig {
                durable_binding: Some(crate::durable::ConversationStoreBinding::new(store)),
                ..crate::tools::runtime::ConversationRuntimeConfig::new(
                    &workspace,
                    dir.path().join("artifacts"),
                )
                .with_extensions(
                    crate::extensions::NativeAgentExtensions::with_agent_status(
                        crate::context::AgentStatusConfig::default(),
                    ),
                )
            },
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
                extension_tools: tool_runtime.extension_tool_plane(),
                agent_activation: fixture_activation(&tool_runtime),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let subagents = crate::runtime::subagent::SubagentRegistry::new(
            crate::runtime::subagent::SubagentRegistryConfig {
                conversation_id: registry_conversation.clone(),
                agent_id: registry_agent.clone(),
                mailbox: tool_runtime.mailbox(),
                clock: Arc::new(crate::runtime::types::SystemClock),
                monotonic_clock: Arc::new(crate::runtime::ManualMonotonicClock::new()),
                spawn: crate::runtime::subagent::SubagentSpawnPlan {
                    session_id: crate::runtime::identity::SessionId::new(
                        "ses_01900000-0000-7000-8000-000000000001",
                    ),

                    program: std::path::PathBuf::from("/nonexistent/rustx"),
                    product_root: crate::runtime::local_storage::ProductRoot::create(
                        &dir.path().join("subagents"),
                    )
                    .expect("product root"),
                },
                workspace: crate::runtime::workspace::WorkspaceManager::new(
                    &workspace,
                    dir.path().join("subagents"),
                ),
                max_active: 4,
            },
        );
        let model = Arc::new(FakeModel::new(Vec::new()));
        let adapter: Arc<dyn ModelAdapter> = model.clone();
        let config = RuntimeConversationConfig {
            explicit_model: true,
            agent_id: runtime_agent.clone(),
            model: scripted_session_model(adapter),
            approval_mode: ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator: Arc::new(DefaultTokenEstimator),
                status_engine: Some(AgentStatusEngine::default()),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator,
            clock: None,
            initial_messages: Vec::new(),
            subagents: Some(subagents.clone()),
            workflow_output: None,
        };
        (subagents, config)
    }

    /// The runtime rejects a `SubagentRegistry` that belongs to another
    /// conversation's ownership domain before anything is claimed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn construction_rejects_a_registry_for_another_conversation() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_efeaf863-95c1-73d9-8623-5a766b1967be",
            ))
            .expect("in-memory store"),
        );
        let (_subagents, config) = subagent_runtime_config_with_registry(
            &dir,
            "conv_efeaf863-95c1-73d9-8623-5a766b1967be",
            store,
            &ConversationId::new("conv_a2120f51-0e8e-7082-8caf-04e5d3e2a71d"),
            &AgentId::new("agent-a"),
            &AgentId::new("agent-a"),
        )
        .await;
        let error = ConversationRuntime::new(config).expect_err("mismatched registry domain");
        assert!(
            matches!(
                &error,
                ConversationRuntimeError::SubagentOwnershipMismatch {
                    registry_conversation,
                    runtime_conversation,
                } if registry_conversation == &ConversationId::new("conv_a2120f51-0e8e-7082-8caf-04e5d3e2a71d")
                    && runtime_conversation == &ConversationId::new("conv_efeaf863-95c1-73d9-8623-5a766b1967be")
            ),
            "the constructor names both ownership domains: {error}"
        );
    }

    /// The runtime rejects a `SubagentRegistry` whose parent agent identity
    /// disagrees with the runtime's own agent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn construction_rejects_a_registry_for_another_parent_agent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_efeaf863-95c1-73d9-8623-5a766b1967be",
            ))
            .expect("in-memory store"),
        );
        let (_subagents, config) = subagent_runtime_config_with_registry(
            &dir,
            "conv_efeaf863-95c1-73d9-8623-5a766b1967be",
            store,
            &ConversationId::new("conv_efeaf863-95c1-73d9-8623-5a766b1967be"),
            &AgentId::new("agent-other"),
            &AgentId::new("agent-a"),
        )
        .await;
        let error = ConversationRuntime::new(config).expect_err("mismatched parent agent");
        assert!(
            matches!(
                error,
                ConversationRuntimeError::SubagentOwnershipMismatch { .. }
            ),
            "a registry for another parent agent is rejected: {error}"
        );
    }

    /// A registry that already owns a committed child record can be
    /// independently committed before runtime construction; the constructor
    /// must not silently adopt it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn construction_rejects_a_non_pristine_registry_with_committed_children() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_3e98d554-da03-7c85-899c-22164a246018",
            ))
            .expect("in-memory store"),
        );
        let (subagents, config) = subagent_runtime_config_with_registry(
            &dir,
            "conv_3e98d554-da03-7c85-899c-22164a246018",
            store.clone(),
            &ConversationId::new("conv_3e98d554-da03-7c85-899c-22164a246018"),
            &AgentId::new("agent-a"),
            &AgentId::new("agent-a"),
        )
        .await;
        // A standalone registry over the unbound mailbox can commit a child
        // before any runtime exists.
        let (staged, _peer) = stage_runtime_test_child(&dir.path().join("pre-constructed-child"));
        subagents.push_staged_override(staged);
        let accepted = match subagents
            .commit(
                subagents
                    .prepare(
                        &crate::runtime::subagent::SubagentStartSpec {
                            authority: crate::runtime::subagent::DurableAgentAuthority {
                                execution_policy:
                                    crate::runtime::subagent::InheritedExecutionPolicy::default(),
                                resolved: test_resolved_subagent("explore"),
                                approval_mode: crate::runtime::ApprovalMode::Policy,
                            },
                            admission: crate::runtime::subagent::ActivationAdmission {
                                task: "pre-constructed".to_owned(),
                                context: None,
                                origin:
                                    crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                        tool_call_id: ToolCallId::new("call-pre-constructed"),
                                    },
                                terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                            },
                        },
                        &crate::runtime::cancellation::CancellationSignal::new(),
                    )
                    .await
                    .expect("prepare"),
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect("standalone commit")
        {
            crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
            crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
                panic!("no cancellation was requested")
            }
        };

        let error = ConversationRuntime::new(config).expect_err("non-pristine registry");
        assert!(
            matches!(
                error,
                ConversationRuntimeError::SubagentRegistryNotPristine { .. }
            ),
            "the constructor rejects a registry that already owns children: {error}"
        );

        // Settle the committed child (escalate and reap) so the fixture
        // leaks no process.
        let _ = subagents.cancel(
            &accepted.subagent_id,
            crate::runtime::types::CancellationReason::UserRequested,
        );
        let settled = subagents
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(
            settled.state,
            crate::runtime::subagent::SubagentState::Cancelled
        );
    }

    /// The matching pristine registry is accepted: construction keeps its
    /// normal composition behavior.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn construction_accepts_a_matching_pristine_registry() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_77a234cd-423d-7899-862c-728e8960bb43",
            ))
            .expect("in-memory store"),
        );
        let (_subagents, config) = subagent_runtime_config_with_registry(
            &dir,
            "conv_77a234cd-423d-7899-862c-728e8960bb43",
            store,
            &ConversationId::new("conv_77a234cd-423d-7899-862c-728e8960bb43"),
            &AgentId::new("agent-a"),
            &AgentId::new("agent-a"),
        )
        .await;
        let runtime = ConversationRuntime::new(config).expect("matching pristine registry");
        assert!(!runtime.is_activated());
    }

    /// The runtime/subagent ownership transfer has one deterministic total
    /// order. A standalone `SubagentRegistry::commit` that entered its
    /// mailbox standalone decision before the constructor bound the mailbox
    /// Inactive publishes its durable ownership and record first; the
    /// constructor's post-claim pristine arbitration then observes the
    /// non-pristine plane, rolls back every claim it acquired, and rejects
    /// typed. The runtime never silently adopts a child started outside its
    /// ownership transfer.
    ///
    /// Production synchronization: the standalone commit holds the registry
    /// mutex across its durable ownership write and record publication
    /// (`with_running_commit` + record creation under one lock), so the
    /// constructor's post-claim `is_pristine()` blocks until the commit
    /// finishes and then sees the record.
    ///
    /// Test hook: the registry's `CommitBoundaryHook` parks the commit
    /// inside that ownership critical section (mailbox standalone decision
    /// already crossed, registry mutex held, mailbox still unbound). The
    /// constructor starts only after the hook is entered, so the forced
    /// interleaving is: standalone commit in flight -> constructor binds
    /// mailbox -> commit publishes -> post-claim check rejects.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn a_standalone_subagent_commit_winning_the_transfer_race_rejects_construction() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_ef4deea7-17ce-7f7c-8a2f-8b1cdf561c4b",
            ))
            .expect("in-memory store"),
        );
        let (subagents, config) = subagent_runtime_config_with_registry(
            &dir,
            "conv_ef4deea7-17ce-7f7c-8a2f-8b1cdf561c4b",
            store.clone(),
            &ConversationId::new("conv_ef4deea7-17ce-7f7c-8a2f-8b1cdf561c4b"),
            &AgentId::new("agent-a"),
            &AgentId::new("agent-a"),
        )
        .await;
        let tool_runtime = config.tool_runtime.clone();
        let hook = Arc::new(crate::runtime::subagent::CommitBoundaryHook::default());
        subagents.install_commit_boundary_hook(hook.clone());
        let (staged, _peer) = stage_runtime_test_child(&dir.path().join("race-child"));
        subagents.push_staged_override(staged);

        // The standalone commit task: prepares privately and parks inside
        // the ownership-commit critical section (the CommitBoundaryHook),
        // proving it crossed the mailbox standalone decision with the
        // mailbox still unbound.
        let commit_registry = subagents.clone();
        let committer = tokio::spawn(async move {
            let prepared = commit_registry
                .prepare(
                    &crate::runtime::subagent::SubagentStartSpec {
                        authority: crate::runtime::subagent::DurableAgentAuthority {
                            execution_policy:
                                crate::runtime::subagent::InheritedExecutionPolicy::default(),
                            resolved: test_resolved_subagent("explore"),
                            approval_mode: crate::runtime::ApprovalMode::Policy,
                        },
                        admission: crate::runtime::subagent::ActivationAdmission {
                            task: "transfer race".to_owned(),
                            context: None,
                            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                tool_call_id: ToolCallId::new("call-transfer-race"),
                            },
                            terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                        },
                    },
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .await
                .expect("prepare");
            commit_registry
                .commit(
                    prepared,
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::task::spawn_blocking({
                let hook = hook.clone();
                move || hook.wait_until_entered()
            }),
        )
        .await
        .expect("commit-boundary liveness")
        .expect("commit-boundary entered");

        // Start the constructor only now: static domain validation passes,
        // the mailbox is bound to the runtime's Inactive lifecycle, and the
        // post-claim pristine arbitration blocks on the registry mutex held
        // by the parked standalone commit.
        let constructor = tokio::spawn(async move { ConversationRuntime::new(config) });
        hook.release();

        let commit_outcome = tokio::time::timeout(std::time::Duration::from_secs(10), committer)
            .await
            .expect("commit liveness")
            .expect("committer")
            .expect("standalone commit succeeds on the unbound mailbox");
        let accepted = match commit_outcome {
            crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
            crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
                panic!("no cancellation was requested")
            }
        };
        let construction = tokio::time::timeout(std::time::Duration::from_secs(10), constructor)
            .await
            .expect("constructor liveness")
            .expect("constructor task");
        assert!(
            matches!(
                construction,
                Err(ConversationRuntimeError::SubagentRegistryNotPristine { .. })
            ),
            "a standalone child commit that won the transfer must make the constructor reject: {construction:?}"
        );

        // The committed child remains owned by the standalone registry; the
        // constructor never adopted it and never spawned anything.
        let snapshots = subagents.all_snapshots();
        assert_eq!(snapshots.len(), 1, "exactly the standalone child is owned");
        assert_eq!(
            snapshots[0].subagent_id, accepted.subagent_id,
            "the same identity the standalone commit published"
        );
        assert!(
            !tool_runtime.mailbox().is_bound_inactive(),
            "the failed constructor rolled back the mailbox binding"
        );
        // Exactly one durable ownership fact exists — no duplicate child
        // ownership event from the constructor.
        let ownership_facts = store
            .read_events(None, 64)
            .expect("events")
            .events
            .iter()
            .filter(|envelope| {
                matches!(
                    &envelope.event,
                    crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                        subagent_id,
                        ..
                    } if *subagent_id == accepted.subagent_id
                )
            })
            .count();
        assert_eq!(ownership_facts, 1, "one durable ownership fact");

        // Settle the committed child (escalate and reap) so the fixture
        // leaks no process.
        let _ = subagents.cancel(
            &accepted.subagent_id,
            crate::runtime::types::CancellationReason::UserRequested,
        );
        subagents
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
    }

    /// The opposite side of the same total order: a runtime ownership claim
    /// that wins first binds the canonical mailbox to the runtime's
    /// `Inactive` lifecycle, so a later standalone `SubagentRegistry::commit`
    /// is refused by the lifecycle before it can publish anything. The
    /// constructor observes a pristine plane and succeeds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_runtime_claim_winning_first_refuses_the_later_standalone_commit() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_21809a09-ae58-7275-8747-47667d792339",
            ))
            .expect("in-memory store"),
        );
        let (subagents, config) = subagent_runtime_config_with_registry(
            &dir,
            "conv_21809a09-ae58-7275-8747-47667d792339",
            store.clone(),
            &ConversationId::new("conv_21809a09-ae58-7275-8747-47667d792339"),
            &AgentId::new("agent-a"),
            &AgentId::new("agent-a"),
        )
        .await;

        // The runtime ownership claim wins first: construction succeeds on
        // the pristine plane and binds the mailbox Inactive.
        let _runtime = ConversationRuntime::new(config).expect("construction succeeds");
        assert!(subagents.is_pristine());

        // A later standalone subagent start is refused by the runtime-owned
        // Inactive lifecycle at `prepare` — the same lifecycle gate that
        // guards the ownership commit, and the earliest point of the start
        // path — so nothing is ever spawned or published.
        let error = subagents
            .prepare(
                &crate::runtime::subagent::SubagentStartSpec {
                    authority: crate::runtime::subagent::DurableAgentAuthority {
                        execution_policy:
                            crate::runtime::subagent::InheritedExecutionPolicy::default(),
                        resolved: test_resolved_subagent("explore"),
                        approval_mode: crate::runtime::ApprovalMode::Policy,
                    },
                    admission: crate::runtime::subagent::ActivationAdmission {
                        task: "refused after claim".to_owned(),
                        context: None,
                        origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                            tool_call_id: ToolCallId::new("call-refused"),
                        },
                        terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                    },
                },
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect_err("standalone start refused by the runtime-owned lifecycle");
        assert!(matches!(
            error,
            crate::runtime::subagent::SubagentStartError::ConversationInactive
        ));
        assert!(
            subagents.is_pristine(),
            "the refused start published no record"
        );
        assert!(subagents.all_snapshots().is_empty());
        assert!(
            store
                .read_events(None, 64)
                .expect("events")
                .events
                .iter()
                .all(|envelope| !matches!(
                    envelope.event,
                    crate::events::types::RuntimeEvent::SubagentOwnershipCommitted { .. }
                )),
            "no durable ownership fact entered the journal"
        );
    }

    /// Stages a stubborn real child for a registry test; the driver must
    /// escalate to SIGKILL after the scripted terminal frame and then reap it.
    fn stage_runtime_test_child(
        runtime_root: &std::path::Path,
    ) -> (
        crate::runtime::subagent::process::StagedChild,
        tokio::net::UnixStream,
    ) {
        std::fs::create_dir_all(runtime_root).expect("runtime root");
        let (driver_end, test_end) = tokio::net::UnixStream::pair().expect("control pair");
        let (observation_end, _observation_peer) =
            tokio::net::UnixStream::pair().expect("observation pair");
        let child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg("trap '' TERM; exec sleep 60")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .expect("staged child");
        (
            crate::runtime::subagent::process::StagedChild::for_test(
                child,
                driver_end,
                observation_end,
                runtime_root.to_path_buf(),
            ),
            test_end,
        )
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
        assert_eq!(
            admission.message_id.as_str(),
            "conv_d9e0ddde-2c51-755b-8a4e-1932c402870e-inbound-1"
        );
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
            matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id == admission.message_id)
        }));
        // The attempt was admitted exactly once and the runtime is idle
        // again.
        assert!(!fixture.runtime.has_current_attempt());
        assert_eq!(
            request_snapshots(&fixture.runtime.request_history()).len(),
            1
        );
    }

    /// A quarantine belongs to one attempt, not to the conversation's
    /// launch-scoped status template. The first real runtime attempt loses
    /// its Time contribution to a deterministic capture failure; the next
    /// attempt is constructed through the coordinator's normal path and
    /// retries Time with a fresh engine.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn runtime_constructs_a_fresh_status_engine_for_each_attempt() {
        let seam = crate::context::AgentStatusTestSeam::new();
        seam.fail_capture_once(crate::context::AgentStatusModuleId::Time);
        let status_engine = AgentStatusEngine::default().with_test_seam(seam.clone());
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, model) = headless_runtime_with_options(
            &dir,
            vec![one_turn_script(), one_turn_script()],
            None,
            None,
            HeadlessRuntimeOptions {
                status_engine,
                ..HeadlessRuntimeOptions::default()
            },
        )
        .await;
        runtime.activate();

        runtime
            .submit_inbound(text_content("first attempt"))
            .expect("first inbound accepted");
        let first_ledger = await_settled_ledger(&runtime).await;
        assert_eq!(
            seam.capture_count(crate::context::AgentStatusModuleId::Time),
            1,
            "the first attempt captures Time once before quarantining it"
        );
        assert!(
            first_ledger.iter().all(|message| !matches!(
                message,
                MessageBlock::User(user)
                    if matches!(
                        &user.kind,
                        InboundKind::Context(crate::message::types::ContextKind::AgentStatus(_))
                    )
            )),
            "the failed module contributes no status message"
        );

        runtime
            .submit_inbound(text_content("second attempt"))
            .expect("second inbound accepted");
        let second_ledger = await_settled_ledger(&runtime).await;
        assert_eq!(
            seam.capture_count(crate::context::AgentStatusModuleId::Time),
            2,
            "a new runtime attempt retries the previously quarantined module"
        );
        assert!(
            second_ledger.iter().any(|message| matches!(
                message,
                MessageBlock::User(user)
                    if matches!(
                        &user.kind,
                        InboundKind::Context(crate::message::types::ContextKind::AgentStatus(_))
                    )
            )),
            "the retried Time module contributes through the normal runtime path"
        );
        assert_eq!(
            model.requests().len(),
            2,
            "one provider request per attempt"
        );
    }

    /// Manual compaction is a first-class idle maintenance operation: it
    /// uses the configured summary invocation, commits one canonical summary
    /// without an attempt identity, and returns the conversation to the
    /// coordinator before reporting success.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn manual_compaction_commits_and_restores_the_idle_conversation() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, model) = headless_runtime(
            &dir,
            vec![
                one_turn_script(),
                text_turn_script("compact factual summary"),
            ],
            None,
            None,
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();
        runtime
            .submit_inbound(text_content(&"important history ".repeat(512)))
            .expect("accepted");
        let _ = await_observation(&pending, |observation| {
            matches!(
                observation,
                ConversationObservation::Event { event, .. } if is_terminal_event(event)
            )
        })
        .await;
        let _ = await_settled_ledger(&runtime).await;

        let outcome = runtime.compact_context().await.expect("manual compaction");
        assert_eq!(outcome.generation, 1);
        assert!(outcome.estimated_tokens_after < outcome.tokens_before.input_tokens);
        assert!(!runtime.has_current_attempt());
        let active = runtime
            .coordinator_active_ids()
            .expect("conversation restored after success");
        assert_eq!(active, vec![outcome.summary_message_id.clone()]);
        let ledger = runtime.coordinator_ledger().expect("restored ledger");
        assert!(ledger.iter().any(|message| {
            matches!(
                message,
                MessageBlock::User(user)
                    if user.id == outcome.summary_message_id
                        && user.kind.is_compaction_summary()
            )
        }));
        assert_eq!(model.requests().len(), 2, "turn plus summary request");

        let observations = pending.drain();
        assert!(matches!(
            observations.first(),
            Some(ConversationObservation::ManualCompactionEvent {
                event: RuntimeEvent::CompactionStarted
            })
        ));
        assert!(observations.iter().any(|observation| {
            matches!(
                observation,
                ConversationObservation::Committed {
                    attempt_id: None,
                    block: MessageBlock::User(user),
                    ..
                } if user.id == outcome.summary_message_id
            )
        }));
        assert!(matches!(
            observations.last(),
            Some(ConversationObservation::ManualCompactionEvent {
                event: RuntimeEvent::CompactionCompleted { generation: 1, .. }
            })
        ));
    }

    /// Manual admission freezes capability-derived Skill guidance as
    /// non-retirable request input. The same history can retire one message
    /// without that guidance, but must retire two when the frozen catalog
    /// makes the one-message candidate exceed the exact soft limit.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn manual_compaction_exact_fit_includes_frozen_skill_guidance() {
        fn estimator() -> Arc<dyn TokenEstimator> {
            Arc::new(ClosureTokenEstimator::new(
                |messages, effective_system_prompt, _tools| {
                    let conversation = messages
                        .iter()
                        .map(|message| {
                            if matches!(
                                message,
                                crate::model::ModelInputMessage::Canonical(MessageBlock::User(user))
                                    if user.kind.is_compaction_summary()
                            ) {
                                10_000
                            } else {
                                350_000
                            }
                        })
                        .sum::<u64>();
                    conversation
                        + if effective_system_prompt.is_empty() {
                            0
                        } else {
                            400_000
                        }
                },
            ))
        }

        fn history() -> Vec<MessageBlock> {
            vec![
                seed_user("oldest", "oldest fact"),
                seed_user("middle", "middle fact"),
                seed_user("recent", "recent fact"),
            ]
        }

        let without_skill = tempfile::tempdir().expect("temp dir");
        let (runtime_without_skill, _) = headless_runtime_with_options(
            &without_skill,
            vec![text_turn_script("S")],
            None,
            None,
            HeadlessRuntimeOptions {
                estimator: estimator(),
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 700_000,
                    summary_output_cap: None,
                },
                initial_messages: history(),
                ..HeadlessRuntimeOptions::default()
            },
        )
        .await;
        runtime_without_skill.activate();
        let without_outcome = runtime_without_skill
            .compact_context()
            .await
            .expect("one-message retirement fits without Skill guidance");
        assert_eq!(without_outcome.tokens_before.input_tokens, 1_050_000);
        assert_eq!(without_outcome.estimated_tokens_after, 710_000);
        assert_eq!(
            runtime_without_skill
                .coordinator_active_ids()
                .expect("restored without Skill guidance"),
            vec![
                without_outcome.summary_message_id,
                crate::runtime::identity::MessageId::new("middle"),
                crate::runtime::identity::MessageId::new("recent"),
            ],
            "the retention target keeps the one-message candidate when it truly fits"
        );

        let with_skill = tempfile::tempdir().expect("temp dir");
        let skill = with_skill
            .path()
            .join("workspace/.agents/skills/exact-fit-skill");
        std::fs::create_dir_all(&skill).expect("skill package");
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: exact-fit-skill\ndescription: Frozen fit guidance.\n---\n\nInstructions.\n",
        )
        .expect("SKILL.md");
        let (runtime_with_skill, _) = headless_runtime_with_options(
            &with_skill,
            vec![text_turn_script("S")],
            Some(with_native_read(ToolRegistry::new())),
            None,
            HeadlessRuntimeOptions {
                skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                    skill.parent().unwrap(),
                ),
                estimator: estimator(),
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 700_000,
                    summary_output_cap: None,
                },
                initial_messages: history(),
                ..HeadlessRuntimeOptions::default()
            },
        )
        .await;
        runtime_with_skill.activate();
        let with_outcome = runtime_with_skill
            .compact_context()
            .await
            .expect("a larger retirement span fits with frozen Skill guidance");
        assert_eq!(with_outcome.tokens_before.input_tokens, 1_450_000);
        assert_eq!(with_outcome.estimated_tokens_after, 760_000);
        assert_eq!(
            runtime_with_skill
                .coordinator_active_ids()
                .expect("restored with Skill guidance"),
            vec![
                with_outcome.summary_message_id,
                crate::runtime::identity::MessageId::new("recent"),
            ],
            "the frozen catalog makes the one-message candidate fail, so planning retires two"
        );
    }

    /// A rejected manual compaction never strands the checked-out state. An
    /// empty conversation has no retirable span, and ordinary inbound still
    /// admits immediately after that typed failure.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn manual_compaction_no_progress_restores_state_for_future_admission() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, _model) = headless_runtime(&dir, vec![one_turn_script()], None, None).await;
        runtime.activate();
        let error = runtime
            .compact_context()
            .await
            .expect_err("empty context cannot compact");
        assert!(matches!(
            error,
            ManualCompactionError::Context(ContextError {
                kind: ContextErrorKind::NoProgress,
                ..
            })
        ));
        assert!(runtime.coordinator_ledger().is_some());
        runtime
            .submit_inbound(text_content("works after rejected compaction"))
            .expect("accepted after failure");
        let _ = await_settled_ledger(&runtime).await;
        assert!(!runtime.has_current_attempt());
    }

    /// Manual compaction never aborts or races an active Agent attempt. The
    /// caller receives a typed busy error and the provider turn keeps its
    /// original cancellation/settlement authority.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn manual_compaction_rejects_a_running_attempt_without_cancelling_it() {
        use crate::model::event::ModelEvent;
        use crate::model::finish::ModelFinishReason;

        let (release_sender, release_receiver) = model_release();
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, model) = headless_runtime(
            &dir,
            vec![vec![
                FakeStep::ParkUntilReleased(release_receiver),
                FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]],
            None,
            None,
        )
        .await;
        runtime.activate();
        runtime
            .submit_inbound(text_content("keep this attempt running"))
            .expect("accepted");
        let mut parked = model.parked();
        within_liveness_guard("manual compaction busy model park", async {
            parked.wait_for(|is_parked| *is_parked).await
        })
        .await
        .expect("model park watch remains open");

        assert_eq!(
            runtime.compact_context().await,
            Err(ManualCompactionError::Busy)
        );
        assert!(runtime.has_current_attempt());
        release_sender.send(true).expect("release provider");
        let _ = await_settled_ledger(&runtime).await;
        assert!(!runtime.has_current_attempt());
    }

    /// The mailbox and every full-store operation are derived from one
    /// `ConversationStoreBinding`. Two independent stores may happen to use
    /// the same `ConversationId`, but there is no production constructor that
    /// can pair one store's mailbox with the other's canonical/event store.
    /// The runtime also performs a normal turn without enumerating its
    /// historical Request Snapshots.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn runtime_uses_one_durable_binding_without_snapshot_enumeration() {
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_e80836f4-9cfc-7703-834b-1d9831f9f8e5");
        let store_a = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(conversation_id.clone())
                .expect("store A"),
        );
        let store_b = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(conversation_id).expect("store B"),
        );
        let (runtime, model) = headless_runtime_over_store(
            &dir,
            "conv_e80836f4-9cfc-7703-834b-1d9831f9f8e5",
            store_a.clone(),
        )
        .await;
        runtime.activate();

        let accepted = runtime
            .submit_inbound(text_content("one durable authority"))
            .expect("accepted");
        let ledger = await_settled_ledger(&runtime).await;

        assert!(ledger.iter().any(|message| {
            matches!(message, MessageBlock::User(user) if user.id == accepted.message_id)
        }));
        assert_eq!(model.requests().len(), 1, "one normal admitted turn");
        assert_eq!(store_a.request_snapshot_page_reads(), 0);
        assert!(
            store_b
                .load_canonical()
                .expect("store B canonical")
                .is_empty(),
            "an independent same-id store never becomes a hidden second authority"
        );
        assert!(
            store_b
                .read_events(None, 32)
                .expect("store B events")
                .events
                .is_empty(),
            "the Event Journal also remains on the bound store"
        );
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
        use crate::message::types::ContentBlockIndex;
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
            approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
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
                MessageBlock::User(user) if user.kind.is_compaction_summary() => "summary",
                MessageBlock::User(_) => "user",
                MessageBlock::Assistant(_) => "assistant",
                MessageBlock::Tool(_) => "tool",
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
            matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id == admission.message_id)
        }));
        assert!(
            requests[1]
                .messages
                .iter()
                .any(|message| matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::Tool(tool)) if tool.tool_call_id.as_str() == call_id)),
            "the second provider request observes the canonical ToolResult"
        );

        // Terminal settlement happened exactly once and the runtime owns
        // the authoritative state again, with the request facts retained.
        assert!(!runtime.has_current_attempt());
        let history = runtime.request_history();
        let snapshots = request_snapshots(&history);
        assert_eq!(snapshots.len(), 2, "both requests retained");
        assert_eq!(
            snapshots[0].runtime_resource_revision, snapshots[1].runtime_resource_revision,
            "tool continuation retains the attempt-admitted resource generation"
        );
        assert_eq!(
            snapshots[0].capability_revision, snapshots[1].capability_revision,
            "tool continuation retains the attempt capability revision"
        );
        assert_eq!(snapshots[0].tool_definitions, snapshots[1].tool_definitions);
        assert_eq!(
            snapshots[0].effective_system_prompt,
            snapshots[1].effective_system_prompt
        );
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
            .enqueue(runtime_inbound_message(
                "conv_2d631b33-df46-75b2-8fc6-0b2b4f2a77d4",
            ))
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
                MessageBlock::User(user) if user.id.as_str() == "conv_2d631b33-df46-75b2-8fc6-0b2b4f2a77d4"
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
            activation_gate: None,
            manual_compaction_settlement_gate: None,
            submit_gate: None,
            submit_arrival: None,
            shutdown_arrival: Some(Arc::new(tokio::sync::Notify::new())),
            mcp_failure_drain_gate: None,
            drain_linearization: None,
            start_boundary_pause: None,
            model_arbitration_pause: None,
            tool_start_pause: None,
            drain_supervision: None,
            attempt_exit_gate: None,
            parent_guidance_seal_gate: None,
            background_failure_gate: None,
            subagent_failure_published_gate: None,
        }))
        .await;
        gate.arm();

        // The async producer enqueues first; the wake gate starts an
        // admission that parks before the coordinator lock.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(runtime_inbound_message(
                "conv_2d631b33-df46-75b2-8fc6-0b2b4f2a77d4",
            ))
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
        assert_eq!(inbound[0], "conv_2d631b33-df46-75b2-8fc6-0b2b4f2a77d4");
        // The client submit is the second item of the one durable sequence
        // domain, so its deterministic identity derives from sequence 2.
        assert!(inbound.contains(&"conv_d9e0ddde-2c51-755b-8a4e-1932c402870e-inbound-2"));
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

    /// Durable pending inbound accepted before the runtime exists (for
    /// example by a crashed process) drives admission on recovery without a
    /// new client request (Issue #63 recovery seam).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn recovered_durable_pending_drives_admission_without_a_client_request() {
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let artifacts = dir.path().join("artifacts");

        // First process: accept one inbound item durably, then die before
        // any adoption.
        {
            let _tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
                ConversationId::new("conv_d9e0ddde-2c51-755b-8a4e-1932c402870e"),
                crate::tools::runtime::ConversationRuntimeConfig::new(&workspace, &artifacts)
                    .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
                        crate::context::AgentStatusConfig::default(),
                    )),
            )
            .expect("tool runtime");
            let store = crate::durable::SqliteConversationStore::open(
                ConversationId::new("conv_d9e0ddde-2c51-755b-8a4e-1932c402870e"),
                &artifacts.join("conversation.sqlite"),
            )
            .map(Arc::new)
            .expect("durable store");
            store
                .accept_inbound(crate::durable::inbox::InboundDraft {
                    message_id: None,
                    source: UserSource::Human,
                    kind: InboundKind::Message,
                    content: text_content("recovered"),
                    timestamp: chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                        .expect("parse")
                        .with_timezone(&chrono::Utc),
                    correlation: None,
                })
                .expect("durable acceptance");
        }

        // Second process: reconstruct the runtime over the same durable
        // store and activate. The recovered pending item drives admission by
        // itself, without any new submit.
        let (runtime, model) = headless_runtime(&dir, vec![one_turn_script()], None, None).await;
        runtime.activate();
        let ledger = await_settled_ledger(&runtime).await;
        let inbound: Vec<&str> = ledger
            .iter()
            .filter_map(|message| match message {
                MessageBlock::User(user) if user.kind == InboundKind::Message => {
                    Some(user.id.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            inbound,
            vec!["conv_d9e0ddde-2c51-755b-8a4e-1932c402870e-inbound-1"],
            "the recovered pending item is adopted exactly once"
        );
        assert_eq!(model.requests().len(), 1, "one admitted attempt");
        assert!(
            runtime
                .tool_runtime()
                .mailbox()
                .select_pending_batch()
                .expect("select")
                .is_none(),
            "no pending record remains after recovery adoption"
        );
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

    fn project_file(path: &std::path::Path, content: &str) -> crate::runtime::ProjectContextFile {
        crate::runtime::ProjectContextFile {
            path: path.to_path_buf(),
            content: content.to_owned(),
        }
    }

    /// A cold reopen publishes current resource authority for a new attempt,
    /// while the old compaction summary and old `RequestSnapshot` remain exact
    /// historical values. Reopen does not synthesize a resource-change fact.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cold_reopen_uses_current_resources_without_rewriting_history() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("workspace/AGENTS.md");
        std::fs::create_dir_all(path.parent().expect("workspace")).expect("workspace");
        std::fs::write(&path, "old project authority").expect("old project file");

        let (runtime_a, model_a) = headless_runtime_with_options(
            &dir,
            vec![one_turn_script(), text_turn_script("summary-A")],
            None,
            None,
            HeadlessRuntimeOptions {
                project_context_files: vec![project_file(&path, "old project authority")],
                ..HeadlessRuntimeOptions::default()
            },
        )
        .await;
        runtime_a.activate();
        runtime_a
            .submit_inbound(text_content("historical request A"))
            .expect("request A");
        let _ = await_settled_ledger(&runtime_a).await;
        let snapshots_a = request_snapshots(&runtime_a.request_history());
        assert_eq!(snapshots_a.len(), 1);
        assert!(
            snapshots_a[0]
                .effective_system_prompt
                .contains("old project authority")
        );
        let snapshot_a_bytes = serde_json::to_vec(&snapshots_a[0]).expect("snapshot A bytes");

        let outcome = runtime_a
            .compact_context()
            .await
            .expect("compaction under resource generation A");
        let ledger_a = runtime_a.coordinator_ledger().expect("generation A ledger");
        let summary_a = ledger_a
            .iter()
            .find(|message| {
                matches!(
                    message,
                    MessageBlock::User(user)
                        if user.id == outcome.summary_message_id
                            && user.kind.is_compaction_summary()
                )
            })
            .cloned()
            .expect("summary A");
        let summary_a_bytes = serde_json::to_vec(&summary_a).expect("summary A bytes");
        assert!(
            ledger_a.iter().all(|message| {
                !serde_json::to_string(message)
                    .expect("ledger message JSON")
                    .contains("old project authority")
            }),
            "project instructions remain request-time authority, not history"
        );
        runtime_a.shutdown().await.expect("stop generation A");
        drop(runtime_a);
        drop(model_a);

        std::fs::write(&path, "new project authority").expect("new project file");
        let (runtime_b, model_b) = headless_runtime_with_options(
            &dir,
            vec![one_turn_script()],
            None,
            None,
            HeadlessRuntimeOptions {
                project_context_files: vec![project_file(&path, "new project authority")],
                ..HeadlessRuntimeOptions::default()
            },
        )
        .await;
        runtime_b.activate();
        runtime_b
            .submit_inbound(text_content("new request B"))
            .expect("request B");
        let ledger_b = await_settled_ledger(&runtime_b).await;
        let snapshots_b = request_snapshots(&runtime_b.request_history());
        assert_eq!(snapshots_b.len(), 2);
        assert_eq!(
            serde_json::to_vec(&snapshots_b[0]).expect("reopened snapshot bytes"),
            snapshot_a_bytes,
            "cold reopen does not rewrite the old RequestSnapshot"
        );
        assert_eq!(
            serde_json::to_vec(
                ledger_b
                    .iter()
                    .find(|message| {
                        matches!(
                            message,
                            MessageBlock::User(user)
                                if user.id == outcome.summary_message_id
                                    && user.kind.is_compaction_summary()
                        )
                    })
                    .expect("reopened summary A")
            )
            .expect("reopened summary bytes"),
            summary_a_bytes,
            "cold reopen does not rewrite the old CompactionSummary"
        );
        assert!(
            model_b.requests()[0]
                .effective_system_prompt
                .contains("new project authority")
        );
        assert!(
            !model_b.requests()[0]
                .effective_system_prompt
                .contains("old project authority")
        );
        assert!(
            snapshots_b[1]
                .effective_system_prompt
                .contains("new project authority")
        );
        assert!(ledger_b.iter().all(|message| {
            !serde_json::to_string(message)
                .expect("ledger message JSON")
                .contains("resources changed")
        }));
        runtime_b.shutdown().await.expect("stop generation B");
    }

    /// External resource edits made while an admitted provider call is
    /// parked cannot splice a new generation into that attempt's automatic
    /// overflow-compaction retry. Project instructions, Skill metadata, and
    /// Tool definitions all remain byte-identical across the retry.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn automatic_compaction_keeps_attempt_pinned_resources_after_external_edits() {
        use crate::model::error::{ModelError, ModelErrorKind};
        use crate::model::event::ModelEvent;

        fn tool_registry(id: &str, name: &str, description: &str) -> ToolRegistry {
            let definition = ToolDefinition {
                id: ToolId::new(id),
                name: name.to_owned(),
                description: description.to_owned(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
                execution_policy: ToolExecutionPolicy::ForegroundOnly,
                concurrency_policy: ToolConcurrencyPolicy::default(),
                approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
                replay_policy: ToolReplayPolicy::Never,
                origin: ToolOrigin::Builtin,
            };
            let tool = FakeTool::new(definition, success_result("unused"));
            let mut registry = ToolRegistry::new();
            tool.register(&mut registry);
            with_native_read(registry)
        }

        let dir = tempfile::tempdir().expect("temp dir");
        let workspace = dir.path().join("workspace");
        let project_path = workspace.join("AGENTS.md");
        let skill = workspace.join(".agents/skills/pinned-skill");
        let skill_markdown = skill.join("SKILL.md");
        std::fs::create_dir_all(&skill).expect("Skill package");
        std::fs::write(&project_path, "project authority generation A")
            .expect("project generation A");
        std::fs::write(
            &skill_markdown,
            "---\nname: pinned-skill\ndescription: Skill generation A.\n---\n\nBody A.\n",
        )
        .expect("Skill generation A");

        let loader = Arc::new(MutableResourceLoader::new(vec![project_file(
            &project_path,
            "project authority generation B",
        )]));
        let loader_trait: Arc<dyn crate::runtime::RuntimeResourceLoader> = loader.clone();
        let (release, release_rx) = model_release();
        let overflow_script = vec![
            FakeStep::ParkUntilReleased(release_rx),
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::Failed {
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
            }),
        ];
        let (runtime, model) = headless_runtime_with_options(
            &dir,
            vec![overflow_script, text_turn_script("s"), one_turn_script()],
            Some(tool_registry(
                "tool-generation-a",
                "tool_generation_a",
                "Tool generation A",
            )),
            None,
            HeadlessRuntimeOptions {
                skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                    skill.parent().unwrap(),
                ),
                initial_messages: vec![seed_user(
                    "old",
                    "old history that the overflow compaction retires: this seed carries enough \
                     retired content that replacing it with the compact summary is measurable \
                     progress even with the typed metadata the canonical summary now carries",
                )],
                project_context_files: vec![project_file(
                    &project_path,
                    "project authority generation A",
                )],
                resource_loader: Some(loader_trait),
                ..HeadlessRuntimeOptions::default()
            },
        )
        .await;
        runtime.activate();
        runtime
            .submit_inbound(text_content("fresh request"))
            .expect("fresh request");
        let mut parked = model.parked();
        parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("the first provider request is parked");

        std::fs::write(&project_path, "project authority generation B")
            .expect("edit project authority");
        std::fs::write(
            &skill_markdown,
            "---\nname: pinned-skill\ndescription: Skill generation B.\n---\n\nBody B.\n",
        )
        .expect("edit Skill metadata");
        loader.set_capability_inputs(crate::capabilities::CapabilityResourceInputs {
            source_demand: crate::capabilities::source::ToolSourceDemand::default(),
            base_tool_registry: Arc::new(tool_registry(
                "tool-generation-b",
                "tool_generation_b",
                "Tool generation B",
            )),
            agent_activation: {
                let mut activation = fixture_activation(runtime.tool_runtime());
                activation.profile.tools.builtin = vec!["tool_generation_b".into()];
                activation
            },
            skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                skill.parent().unwrap(),
            ),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: runtime.tool_runtime().environment().clone(),
        });
        release
            .send(true)
            .expect("release the first provider request");
        let _ = await_settled_ledger(&runtime).await;

        let requests = model.requests();
        assert_eq!(requests.len(), 3, "overflow, isolated summary, and retry");
        assert!(
            requests[0]
                .effective_system_prompt
                .contains("project authority generation A")
        );
        assert!(
            requests[0]
                .effective_system_prompt
                .contains("Skill generation A.")
        );
        assert!(!requests[0].effective_system_prompt.contains("generation B"));
        assert!(
            requests[0]
                .tools
                .iter()
                .any(|tool| tool.name == "tool_generation_a")
        );
        assert!(
            requests[0]
                .tools
                .iter()
                .all(|tool| tool.name != "tool_generation_b")
        );

        assert!(requests[1].tools.is_empty());
        assert!(requests[1].effective_system_prompt.is_empty());
        assert!(requests[1].continuation.is_none());

        assert_eq!(
            requests[2].effective_system_prompt, requests[0].effective_system_prompt,
            "the retry keeps the admitted System bytes"
        );
        assert_eq!(
            requests[2].tools, requests[0].tools,
            "the retry keeps the admitted Tool definitions"
        );
        assert!(
            requests[2]
                .effective_system_prompt
                .contains("project authority generation A")
        );
        assert!(
            requests[2]
                .effective_system_prompt
                .contains("Skill generation A.")
        );
        assert!(!requests[2].effective_system_prompt.contains("generation B"));
        assert!(
            requests[2]
                .tools
                .iter()
                .all(|tool| tool.name != "tool_generation_b")
        );
        assert_eq!(
            loader.prepare_count(),
            0,
            "no configuration application occurred during compaction"
        );
    }

    #[cfg(feature = "mcp-fixture")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn preactivation_mcp_settlement_failure_is_replayed_and_fences_activation() {
        use crate::tools::mcp::fixture::{
            FIXTURE_MODE_ENV, FixtureServer, fixture_spawn_args, serve_if_fixture_mode,
        };
        use crate::tools::mcp::test_sync::CloseProbe;

        if serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }

        let test_name = "runtime::conversation_runtime::tests::preactivation_mcp_settlement_failure_is_replayed_and_fences_activation";
        let binding = crate::tools::mcp::McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            transport: crate::tools::mcp::McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: fixture_spawn_args(test_name),
                cwd: None,
                environment: std::collections::BTreeMap::from([(
                    FIXTURE_MODE_ENV.to_owned(),
                    "1".to_owned(),
                )]),
            },
            policy: crate::tools::types::ToolInvocationPolicy::default(),
        };
        let server_id = crate::runtime::identity::McpServerId::new("preactivation-failure");
        let close = Arc::new(CloseProbe::failing(
            "preactivation MCP terminal state is unproven",
        ));
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, model) = headless_runtime_with_options(
            &dir,
            vec![one_turn_script()],
            None,
            None,
            HeadlessRuntimeOptions {
                mcp_servers: std::collections::BTreeMap::from([(server_id, binding)]),
                // The helper retires and settles this generation before it
                // constructs ConversationRuntime, so the callback installed
                // by the runtime must replay an already-authoritative failure.
                pre_activation_mcp_close_probe: Some(close),
                ..HeadlessRuntimeOptions::default()
            },
        )
        .await;

        assert_eq!(
            runtime.inner.lifecycle.state(),
            ConversationLifecycleState::Draining,
            "a replayed pre-activation settlement failure enters the explicit failure drain"
        );
        assert!(!runtime.inner.lifecycle.is_running());
        runtime.activate();
        assert_eq!(
            runtime.inner.lifecycle.state(),
            ConversationLifecycleState::Draining,
            "activation cannot reopen healthy admission after callback replay"
        );
        assert_eq!(
            runtime.submit_inbound(text_content("must never start")),
            Err(InboundAdmissionError::Shutdown)
        );
        assert!(model.requests().is_empty(), "no attempt crossed activation");

        let shutdown = runtime.shutdown().await;
        let Err(super::ShutdownError::RuntimeOwnedSettlement { detail }) = shutdown else {
            panic!("shutdown must retain the pre-activation settlement failure: {shutdown:?}");
        };
        assert!(detail.contains("preactivation MCP terminal state is unproven"));
        assert_eq!(
            runtime.inner.capability.pending_mcp_retirements(),
            1,
            "the failed generation remains authoritative through final reporting"
        );
    }

    #[cfg(feature = "mcp-fixture")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mcp_settlement_failure_wins_deterministic_activation_race() {
        use crate::tools::mcp::fixture::{
            FIXTURE_MODE_ENV, FixtureServer, fixture_spawn_args, serve_if_fixture_mode,
        };
        use crate::tools::mcp::test_sync::CloseProbe;

        if serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }

        let test_name = "runtime::conversation_runtime::tests::mcp_settlement_failure_wins_deterministic_activation_race";
        let binding = crate::tools::mcp::McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            transport: crate::tools::mcp::McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: fixture_spawn_args(test_name),
                cwd: None,
                environment: std::collections::BTreeMap::from([(
                    FIXTURE_MODE_ENV.to_owned(),
                    "1".to_owned(),
                )]),
            },
            policy: crate::tools::types::ToolInvocationPolicy::default(),
        };
        let server_id = crate::runtime::identity::McpServerId::new("activation-race");
        let activation_gate = Arc::new(Gate::default());
        activation_gate.arm();
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, model) = headless_runtime_with_options(
            &dir,
            vec![one_turn_script()],
            None,
            Some(CoordinatorProbe {
                activation_gate: Some(activation_gate.clone()),
                ..CoordinatorProbe::default()
            }),
            HeadlessRuntimeOptions {
                mcp_servers: std::collections::BTreeMap::from([(server_id.clone(), binding)]),
                ..HeadlessRuntimeOptions::default()
            },
        )
        .await;
        let old_runtime = runtime
            .inner
            .capability
            .current_mcp_runtime(&server_id)
            .expect("generation A");
        let close = Arc::new(CloseProbe::failing(
            "activation race MCP terminal state is unproven",
        ));
        old_runtime.install_close_probe(close);

        let activation_runtime = runtime.clone();
        let activation = tokio::spawn(async move { activation_runtime.activate() });
        // The gate proves activation has not acquired the coordinator lock or
        // crossed Inactive -> Running yet.
        activation_gate.wait_entered();

        runtime.inner.capability.retire_current_mcp_runtimes();
        let settlement = runtime.inner.capability.settle_ready_mcp_runtimes().await;
        assert!(
            settlement.is_err(),
            "the retired generation must fail settlement"
        );
        assert_eq!(
            runtime.inner.lifecycle.state(),
            ConversationLifecycleState::Draining,
            "failure publication wins while activation is still gated"
        );

        activation_gate.release();
        activation.await.expect("activation task");
        assert_eq!(
            runtime.inner.lifecycle.state(),
            ConversationLifecycleState::Draining
        );
        assert_eq!(
            runtime.submit_inbound(text_content("must be fenced")),
            Err(InboundAdmissionError::Shutdown)
        );
        assert!(model.requests().is_empty());

        let shutdown = runtime.shutdown().await;
        let Err(super::ShutdownError::RuntimeOwnedSettlement { detail }) = shutdown else {
            panic!("shutdown must retain the activation-race failure: {shutdown:?}");
        };
        assert!(detail.contains("activation race MCP terminal state is unproven"));
    }

    #[cfg(feature = "mcp-fixture")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mcp_settlement_failure_fences_after_activation_wins() {
        use crate::tools::mcp::fixture::{
            FIXTURE_MODE_ENV, FixtureServer, fixture_spawn_args, serve_if_fixture_mode,
        };
        use crate::tools::mcp::test_sync::CloseProbe;

        if serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }

        let test_name = "runtime::conversation_runtime::tests::mcp_settlement_failure_fences_after_activation_wins";
        let binding = crate::tools::mcp::McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            transport: crate::tools::mcp::McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: fixture_spawn_args(test_name),
                cwd: None,
                environment: std::collections::BTreeMap::from([(
                    FIXTURE_MODE_ENV.to_owned(),
                    "1".to_owned(),
                )]),
            },
            policy: crate::tools::types::ToolInvocationPolicy::default(),
        };
        let server_id = crate::runtime::identity::McpServerId::new("activation-first");
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, model) = headless_runtime_with_options(
            &dir,
            vec![one_turn_script()],
            None,
            None,
            HeadlessRuntimeOptions {
                mcp_servers: std::collections::BTreeMap::from([(server_id.clone(), binding)]),
                ..HeadlessRuntimeOptions::default()
            },
        )
        .await;
        runtime.activate();
        assert_eq!(
            runtime.inner.lifecycle.state(),
            ConversationLifecycleState::Running
        );
        let old_runtime = runtime
            .inner
            .capability
            .current_mcp_runtime(&server_id)
            .expect("generation A");
        old_runtime.install_close_probe(Arc::new(CloseProbe::failing(
            "activation-first MCP terminal state is unproven",
        )));

        runtime.inner.capability.retire_current_mcp_runtimes();
        assert!(
            runtime
                .inner
                .capability
                .settle_ready_mcp_runtimes()
                .await
                .is_err()
        );
        assert_eq!(
            runtime.inner.lifecycle.state(),
            ConversationLifecycleState::Draining,
            "a later failure immediately fences a runtime that activated first"
        );
        assert_eq!(
            runtime.submit_inbound(text_content("must be fenced")),
            Err(InboundAdmissionError::Shutdown)
        );
        assert!(model.requests().is_empty());

        let shutdown = runtime.shutdown().await;
        let Err(super::ShutdownError::RuntimeOwnedSettlement { detail }) = shutdown else {
            panic!("shutdown must retain the activation-first failure: {shutdown:?}");
        };
        assert!(detail.contains("activation-first MCP terminal state is unproven"));
    }

    #[cfg(feature = "mcp-fixture")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn sibling_mcp_settlement_failures_aggregate_through_one_runtime_drain() {
        use crate::tools::mcp::fixture::{
            FIXTURE_MODE_ENV, FixtureServer, TOOL_PREFIX_ENV, fixture_spawn_args,
            serve_if_fixture_mode,
        };
        use crate::tools::mcp::test_sync::CloseProbe;

        if serve_if_fixture_mode(FixtureServer::from_env()).await {
            return;
        }

        let test_name = "runtime::conversation_runtime::tests::sibling_mcp_settlement_failures_aggregate_through_one_runtime_drain";
        let fixture_binding = |tool_prefix: Option<&str>| {
            let mut environment =
                std::collections::BTreeMap::from([(FIXTURE_MODE_ENV.to_owned(), "1".to_owned())]);
            if let Some(tool_prefix) = tool_prefix {
                environment.insert(TOOL_PREFIX_ENV.to_owned(), tool_prefix.to_owned());
            }
            crate::tools::mcp::McpServerBinding {
                credentials: crate::credentials::SourceCredentials::default(),
                transport: crate::tools::mcp::McpTransportConfig::Stdio {
                    program: std::env::current_exe()
                        .expect("test executable")
                        .display()
                        .to_string(),
                    args: fixture_spawn_args(test_name),
                    cwd: None,
                    environment,
                },
                policy: crate::tools::types::ToolInvocationPolicy::default(),
            }
        };
        let alpha = crate::runtime::identity::McpServerId::new("sibling-alpha");
        let beta = crate::runtime::identity::McpServerId::new("sibling-beta");
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, _) = headless_runtime_with_options(
            &dir,
            Vec::new(),
            None,
            None,
            HeadlessRuntimeOptions {
                mcp_servers: std::collections::BTreeMap::from([
                    (alpha.clone(), fixture_binding(None)),
                    (beta.clone(), fixture_binding(Some("beta-"))),
                ]),
                ..HeadlessRuntimeOptions::default()
            },
        )
        .await;
        runtime.activate();

        let alpha_runtime = runtime
            .inner
            .capability
            .current_mcp_runtime(&alpha)
            .expect("alpha generation A");
        let beta_runtime = runtime
            .inner
            .capability
            .current_mcp_runtime(&beta)
            .expect("beta generation A");
        alpha_runtime.install_close_probe(Arc::new(CloseProbe::failing(
            "sibling alpha physical settlement is unproven",
        )));
        beta_runtime.install_close_probe(Arc::new(CloseProbe::failing(
            "sibling beta physical settlement is unproven",
        )));

        runtime.inner.capability.retire_current_mcp_runtimes();
        let settlement = runtime.inner.capability.settle_ready_mcp_runtimes().await;
        let Err(failures) = settlement else {
            panic!("sibling failures must be reported by ready retirement settlement");
        };
        let message = failures.join("; ");
        assert!(message.contains("sibling alpha physical settlement is unproven"));
        assert!(message.contains("sibling beta physical settlement is unproven"));
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Draining,
            "the second diagnostic does not start a second lifecycle state machine"
        );
        assert_eq!(runtime.inner.capability.pending_mcp_retirements(), 2);

        let shutdown = runtime.shutdown().await;
        let Err(super::ShutdownError::RuntimeOwnedSettlement { detail }) = shutdown else {
            panic!("both sibling failures must remain final shutdown evidence: {shutdown:?}");
        };
        assert!(detail.contains("sibling alpha physical settlement is unproven"));
        assert!(detail.contains("sibling beta physical settlement is unproven"));
        assert_eq!(runtime.inner.capability.pending_mcp_retirements(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn agent_profile_is_snapshot_system_authority_not_bootstrap_history() {
        let dir = tempfile::tempdir().expect("temp dir");
        let persona = "immutable child exploration persona";
        let (runtime, model) = headless_runtime_with_options(
            &dir,
            vec![one_turn_script()],
            None,
            None,
            HeadlessRuntimeOptions {
                agent_profile: Some(persona.to_owned()),
                ..HeadlessRuntimeOptions::default()
            },
        )
        .await;
        assert!(
            runtime
                .coordinator_ledger()
                .expect("bootstrap history")
                .is_empty(),
            "agent profile creates no fake bootstrap conversation message"
        );
        runtime.activate();
        runtime
            .submit_inbound(text_content("delegated task"))
            .expect("delegated inbound");
        let _ = await_settled_ledger(&runtime).await;
        assert!(
            model.requests()[0]
                .effective_system_prompt
                .contains(persona)
        );
        let snapshots = request_snapshots(&runtime.request_history());
        assert_eq!(snapshots.len(), 1);
        assert!(snapshots[0].effective_system_prompt.contains(persona));
        assert!(snapshots[0].system_sections.iter().any(|section| {
            section.lane == crate::context::SystemSectionLane::AgentProfile
                && section.content == persona
        }));
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
        let conversation_id = ConversationId::new("conv_bf9033a7-86e2-71aa-8314-b791ebfdbfec");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("tool runtime");
        let other = ConversationId::new("conv_449370cc-308b-7409-84bd-ff539142e4fb");
        let other_workspace = dir.path().join("workspace-other");
        std::fs::create_dir_all(&other_workspace).expect("other workspace");
        let other_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            other.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &other_workspace,
                dir.path().join("artifacts-other"),
            )
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("other tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: other_runtime.conversation_id().clone(),
                workspace: other_runtime.workspace().clone(),
                base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
                extension_tools: other_runtime.extension_tool_plane(),
                agent_activation: fixture_activation(&tool_runtime),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: other_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let error = ConversationRuntime::new(RuntimeConversationConfig {
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(Arc::new(FakeModel::new(Vec::new()))),
            approval_mode: ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator: Arc::new(DefaultTokenEstimator),
                status_engine: Some(AgentStatusEngine::default()),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator,
            clock: None,
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
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
        let conversation_id = ConversationId::new("conv_6d7e77c3-41c2-710b-8817-1134b3ee72c2");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        // This fixture's subject is the missing execution runtime, and it runs
        // on a plain thread — so it can never `await` a prepared candidate,
        // and its coordinator necessarily stays at revision zero with an
        // empty executable registry. It therefore composes Agent Status and
        // no Tool-providing extension, which is a coherent composition whose
        // required extension Tool authority is *empty*: the Issue #259
        // active-capability check passes on it, and construction reaches the
        // `NoExecutionRuntime` decision this test is about. A Todo-composed
        // fixture here would be genuinely incoherent — configured to publish
        // `todo`, with nothing executable carrying it — and would be refused
        // earlier, for a reason unrelated to Tokio.
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
                crate::context::AgentStatusConfig::default(),
            ))
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
                extension_tools: tool_runtime.extension_tool_plane(),
                agent_activation: fixture_activation(&tool_runtime),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let error = ConversationRuntime::new(RuntimeConversationConfig {
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(Arc::new(FakeModel::new(Vec::new()))),
            approval_mode: ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator: Arc::new(DefaultTokenEstimator),
                status_engine: Some(AgentStatusEngine::default()),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator,
            clock: None,
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        })
        .expect_err("construction outside Tokio is rejected");
        assert!(matches!(
            error,
            ConversationRuntimeError::NoExecutionRuntime
        ));
    }

    /// Shutdown closes further semantic admission.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_gates_further_admission() {
        let fixture = headless_fixture().await;
        fixture
            .runtime
            .shutdown()
            .await
            .expect("accepted after activation");
        assert!(matches!(
            fixture.runtime.submit_inbound(text_content("late")),
            Err(InboundAdmissionError::Shutdown)
        ));
    }

    /// A user cancellation that wins the owning attempt must remain the cause
    /// of every runtime-driven interaction settlement. The waiter is parked
    /// after observing cancellation but before it can call the coordinator;
    /// shutdown therefore has to settle the pending map and propagate the
    /// `AgentCancellation` winner itself.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_propagates_existing_user_cancellation_to_pending_interaction() {
        use crate::message::types::ContentBlockIndex;
        use crate::model::event::ModelEvent;
        use crate::model::finish::ModelFinishReason;
        use crate::tools::types::{ToolCall, ToolCallStart};

        let definition = ToolDefinition {
            id: ToolId::new("tool-approval-user-first"),
            name: "approval".to_owned(),
            description: "a deterministic approval test tool".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"]
            }),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::default(),
            approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        };
        let tool = FakeTool::new(definition.clone(), success_result("must not run"));
        let tool_calls = tool.calls();
        let tool_started = tool.started();
        let mut registry = ToolRegistry::new();
        tool.register(&mut registry);

        let call_id = ToolCallId::new("call-runtime-user-first");
        let scripts = vec![vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call_id.clone(),
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                },
            }),
            FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: call_id.clone(),
                arguments_delta: "{\"text\":\"hi\"}".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCall {
                    id: call_id,
                    tool_id: definition.id,
                    name: definition.name,
                    arguments: serde_json::json!({"text": "hi"}),
                },
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ]];

        let dir = tempfile::tempdir().expect("temp dir");
        let drain_linearization = Arc::new(tokio::sync::Notify::new());
        let probe = CoordinatorProbe {
            drain_linearization: Some(drain_linearization.clone()),
            ..CoordinatorProbe::default()
        };
        let (runtime, _) = headless_runtime(&dir, scripts, Some(registry), Some(probe)).await;
        runtime.install_test_pre_tool_policy(Arc::new(RuntimeAskPolicy));

        let waiter_gate = Arc::new(InteractionWaitCancellationGate::default());
        waiter_gate.arm();
        runtime
            .inner
            .interaction
            .install_wait_cancellation_gate(waiter_gate.clone());
        let settle_gate = Arc::new(InteractionSettleGate::default());
        settle_gate.arm();
        runtime
            .inner
            .interaction
            .install_settle_gate(settle_gate.clone());

        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: runtime.clone(),
            replay_limit: None,
        })
        .expect("Runtime Client host");
        let (attachment, initialized) = host
            .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("Runtime Client attachment");
        let cursor = match initialized {
            RuntimeClientResult::Initialized { cursor, .. } => cursor,
            other => panic!("unexpected initialization result: {other:?}"),
        };
        let subscription = attachment
            .subscribe_events(cursor)
            .expect("Runtime Client subscription");

        runtime.activate();
        let accepted = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: RequestId::new(1),
            content: text_content("request approval"),
        });
        assert!(matches!(
            accepted.result,
            Some(RuntimeClientResult::InboundAccepted { .. })
        ));

        let pending = loop {
            match subscription.next().await {
                EventDelivery::Event(RuntimeClientProtocolEvent { event, .. }) => {
                    if let RuntimeClientEvent::InteractionPending { interaction } = event {
                        break interaction;
                    }
                }
                EventDelivery::Pending => unreachable!("next never returns Pending"),
                delivery => panic!("pending interaction stream ended: {delivery:?}"),
            }
        };
        let (attempt_id, cancellation) = {
            let state = runtime.inner.state.lock().expect("runtime state lock");
            let current = state
                .current_attempt
                .as_ref()
                .expect("pending interaction has an active attempt");
            (current.attempt_id.clone(), current.cancellation.clone())
        };
        assert_eq!(pending.request.attempt_id, attempt_id);

        // This is the first-winner commit. The waiter observes the signal but
        // is then parked before it can call coordinator.cancel.
        assert!(cancellation.request_cancel(CancellationReason::UserRequested));
        assert_eq!(cancellation.reason(), CancellationReason::UserRequested);
        let waiter_gate_for_wait = waiter_gate.clone();
        tokio::task::spawn_blocking(move || waiter_gate_for_wait.wait_entered())
            .await
            .expect("waiter cancellation gate task");

        // Drain now contends with the already-cancelled attempt. Its own
        // RuntimeShutdown request must lose, and the drain path must use the
        // UserRequested reason when it removes the pending interaction.
        let runtime_for_shutdown = runtime.clone();
        let (shutdown_sender, mut shutdown_receiver) = tokio::sync::oneshot::channel();
        let drain_wait = drain_linearization.notified();
        tokio::spawn(async move {
            let _ = shutdown_sender.send(runtime_for_shutdown.shutdown().await);
        });
        drain_wait.await;
        assert_eq!(cancellation.reason(), CancellationReason::UserRequested);
        assert!(!cancellation.request_cancel(CancellationReason::RuntimeShutdown));
        assert_eq!(cancellation.reason(), CancellationReason::UserRequested);

        let settle_gate_for_wait = settle_gate.clone();
        tokio::task::spawn_blocking(move || settle_gate_for_wait.wait_entered())
            .await
            .expect("drain interaction settle gate task");
        assert_eq!(
            runtime.inner.lifecycle.state(),
            ConversationLifecycleState::Draining
        );
        assert_eq!(runtime.inner.interaction.pending_count(), 0);
        assert_eq!(
            shutdown_receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        );

        // Releasing the drain transition only hands the UserRequested result
        // to the still-parked waiter. Quiescence remains impossible until the
        // waiter releases its own cancellation gate and drops the payload.
        settle_gate.release();
        assert_eq!(
            shutdown_receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        );
        waiter_gate.release();
        shutdown_receiver
            .await
            .expect("shutdown completion sender")
            .expect("runtime reaches quiescence");
        assert_eq!(
            runtime.inner.lifecycle.state(),
            ConversationLifecycleState::Quiescent
        );
        assert_eq!(runtime.inner.interaction.pending_count(), 0);

        let settled_outcome = loop {
            match subscription.next().await {
                EventDelivery::Event(RuntimeClientProtocolEvent { event, .. }) => {
                    if let RuntimeClientEvent::InteractionSettled {
                        interaction,
                        outcome,
                    } = event
                        && interaction == pending.interaction
                    {
                        break outcome;
                    }
                }
                EventDelivery::Pending => unreachable!("next never returns Pending"),
                delivery => panic!("settled interaction stream ended: {delivery:?}"),
            }
        };
        assert_eq!(
            settled_outcome,
            InteractionOutcome::Cancelled {
                reason: CancellationReason::UserRequested
            }
        );
        let (snapshot, _) = host.snapshot().expect("post-shutdown snapshot");
        assert!(snapshot.pending_interactions.is_empty());

        let stale = attachment
            .handle_request_async(RuntimeClientRequest::InteractionRespond {
                id: RequestId::new(2),
                interaction: pending.interaction.clone(),
                response: InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            })
            .await;
        assert_eq!(
            stale.error,
            Some(RuntimeClientError::InteractionNotPending {
                interaction: pending.interaction.clone()
            })
        );

        let canonical = runtime
            .inner
            .store
            .load_canonical()
            .expect("canonical tool settlement");
        let tool_messages = canonical
            .iter()
            .filter_map(|message| match message {
                MessageBlock::Tool(tool) => Some(tool),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(tool_messages.len(), 1, "one structural tool result slot");
        assert!(matches!(
            &tool_messages[0].result.status,
            ToolExecutionStatus::Cancelled {
                reason: CancellationReason::UserRequested,
                phase: crate::tools::types::ToolCancellationPhase::BeforeStart,
            }
        ));
        assert!(!matches!(
            &tool_messages[0].result.status,
            ToolExecutionStatus::Cancelled {
                reason: CancellationReason::RuntimeShutdown,
                phase: crate::tools::types::ToolCancellationPhase::BeforeStart,
            }
        ));
        assert!(tool_calls.borrow().is_empty(), "executor was never invoked");
        assert!(!*tool_started.borrow(), "executor never started");

        let journal = runtime
            .inner
            .store
            .read_events(None, 256)
            .expect("runtime event journal")
            .events;
        assert_eq!(
            journal
                .iter()
                .filter(|envelope| matches!(
                    envelope.event,
                    crate::events::types::RuntimeEvent::ToolExecutionStarted { .. }
                ))
                .count(),
            0,
            "cancelled-before-start emits no ToolExecutionStarted"
        );
        let attempt_cancel_reasons = journal
            .iter()
            .filter_map(|envelope| match &envelope.event {
                crate::events::types::RuntimeEvent::AttemptCancelled { reason, .. } => {
                    Some(*reason)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            attempt_cancel_reasons,
            vec![CancellationReason::UserRequested]
        );
        assert_eq!(
            settled_outcome,
            InteractionOutcome::Cancelled {
                reason: CancellationReason::UserRequested
            },
            "no RuntimeShutdown interaction terminal exists for this attempt"
        );
    }

    /// Repeated concurrent shutdown calls join one drain completion and
    /// publish one lifecycle transition.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_shutdown_is_one_idempotent_drain() {
        let fixture = headless_fixture().await;
        let first = fixture.runtime.clone();
        let second = fixture.runtime.clone();
        let (left, right) = tokio::join!(first.shutdown(), second.shutdown());
        assert_eq!(left, Ok(()));
        assert_eq!(right, Ok(()));
        assert_eq!(
            fixture.runtime.lifecycle_state(),
            ConversationLifecycleState::Quiescent
        );
        let third = fixture.runtime.shutdown().await;
        assert_eq!(third, Ok(()));
    }

    /// Issue #63 (Finding 1): a successful acceptance and shutdown have one
    /// total ordering. When acceptance linearizes first — proven by parking
    /// `submit_inbound` inside its coordinator-lock critical section while
    /// `shutdown` blocks on that same lock — the acceptance succeeds and
    /// shutdown follows.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn submit_linearizes_before_shutdown_when_acceptance_wins() {
        let gate = Arc::new(super::Gate::default());
        let shutdown_arrival = Arc::new(tokio::sync::Notify::new());
        let fixture = headless_fixture_with(Some(CoordinatorProbe {
            admission_gate: None,
            settlement_gate: None,
            activation_gate: None,
            manual_compaction_settlement_gate: None,
            submit_gate: Some(gate.clone()),
            submit_arrival: None,
            shutdown_arrival: Some(shutdown_arrival.clone()),
            mcp_failure_drain_gate: None,
            drain_linearization: None,
            start_boundary_pause: None,
            model_arbitration_pause: None,
            tool_start_pause: None,
            drain_supervision: None,
            attempt_exit_gate: None,
            parent_guidance_seal_gate: None,
            background_failure_gate: None,
            subagent_failure_published_gate: None,
        }))
        .await;
        gate.arm();

        // `submit_inbound` parks inside its critical section: the coordinator
        // lock is held, the shutdown decision was read, but the durable
        // acceptance has not yet committed.
        let submit_runtime = fixture.runtime.clone();
        let submit_task = tokio::task::spawn_blocking(move || {
            submit_runtime
                .submit_inbound(text_content("raced"))
                .expect("acceptance linearized before shutdown")
        });
        let submit_entered = {
            let gate = gate.clone();
            tokio::task::spawn_blocking(move || gate.wait_entered())
        };
        submit_entered
            .await
            .expect("submit parked inside its critical section holding the lock");

        // Shutdown signals that it reached the point immediately before
        // attempting the coordinator lock, then blocks on that lock. The
        // ordering proof is mutex exclusion (submit holds the lock), not a
        // timeout: shutdown cannot complete while submit holds the critical
        // section.
        let shutdown_runtime = fixture.runtime.clone();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
        let shutdown_task = tokio::spawn(async move {
            shutdown_runtime
                .shutdown()
                .await
                .expect("shutdown reaches quiescence");
            let _ = shutdown_tx.send(());
        });
        shutdown_arrival.notified().await;

        // Release submit: the acceptance commits, then shutdown acquires the
        // lock and linearizes after it.
        let release = {
            let gate = gate.clone();
            tokio::task::spawn_blocking(move || gate.release())
        };
        release.await.expect("release submit");
        let admission = submit_task.await.expect("submit completed");
        assert_eq!(
            admission.inbound_sequence.get(),
            1,
            "the acceptance was the first sequence of the conversation"
        );
        shutdown_task.await.expect("shutdown completed");
        shutdown_rx
            .recv()
            .expect("shutdown completed after submit released the lock");
        // Shutdown is now authoritative: no later acceptance succeeds.
        assert!(matches!(
            fixture.runtime.submit_inbound(text_content("late")),
            Err(InboundAdmissionError::Shutdown)
        ));
    }

    /// Issue #63 (Finding 1): when shutdown linearizes first, a later submit
    /// returns `Shutdown` and commits no pending item and consumes no
    /// sequence. Admission is frozen deterministically so the pre-shutdown
    /// acceptance stays pending for the exact-count assertion.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_linearizes_before_acceptance_commits_nothing() {
        let admission_gate = Arc::new(super::Gate::default());
        let shutdown_arrival = Arc::new(tokio::sync::Notify::new());
        let fixture = headless_fixture_with(Some(CoordinatorProbe {
            admission_gate: Some(admission_gate.clone()),
            settlement_gate: None,
            activation_gate: None,
            manual_compaction_settlement_gate: None,
            submit_gate: None,
            submit_arrival: None,
            shutdown_arrival: Some(shutdown_arrival.clone()),
            mcp_failure_drain_gate: None,
            drain_linearization: None,
            start_boundary_pause: None,
            model_arbitration_pause: None,
            tool_start_pause: None,
            drain_supervision: None,
            attempt_exit_gate: None,
            parent_guidance_seal_gate: None,
            background_failure_gate: None,
            subagent_failure_published_gate: None,
        }))
        .await;
        // Freeze admission so the worker cannot adopt the pre-shutdown item.
        admission_gate.arm();
        let first = fixture
            .runtime
            .submit_inbound(text_content("before"))
            .expect("pre-shutdown acceptance");
        assert_eq!(first.inbound_sequence.get(), 1);
        admission_gate.wait_entered();
        // Shutdown linearizes first. The worker is still parked at its
        // pre-admission test boundary, so release that boundary only after
        // the explicit shutdown-arrival signal proves drain owns admission.
        let shutdown_runtime = fixture.runtime.clone();
        let shutdown_task = tokio::spawn(async move {
            shutdown_runtime
                .shutdown()
                .await
                .expect("shutdown reaches quiescence");
        });
        shutdown_arrival.notified().await;
        let release = {
            let admission_gate = admission_gate.clone();
            tokio::task::spawn_blocking(move || admission_gate.release())
        };
        release.await.expect("release parked admission");
        shutdown_task.await.expect("shutdown completed");
        assert!(matches!(
            fixture.runtime.submit_inbound(text_content("late")),
            Err(InboundAdmissionError::Shutdown)
        ));

        // The refused acceptance committed no pending item and consumed no
        // sequence: exactly the one pre-shutdown acceptance remains durable.
        let batch = fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .select_pending_batch()
            .expect("select")
            .expect("exactly one pre-shutdown pending item");
        assert_eq!(
            batch.items().len(),
            1,
            "the refused acceptance committed no pending item"
        );
        assert_eq!(
            batch.items()[0].sequence().get(),
            1,
            "the refused acceptance consumed no sequence"
        );
        // Release the frozen admission worker so the test's tokio runtime can
        // shut down: the worker observes the closed wake gate and returns without
        // adopting the pending item.
        admission_gate.release();
    }

    /// The MCP failure transition is one coordinator critical section. The
    /// failure path is parked after both the latch and `Draining` are
    /// published; a competing submit reaches the boundary but cannot cross
    /// it until the failure path releases the coordinator lock.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mcp_failure_and_lifecycle_closure_are_atomic_before_inbound() {
        let failure_gate = Arc::new(super::Gate::default());
        failure_gate.arm();
        let submit_arrival = Arc::new(tokio::sync::Notify::new());
        let fixture = headless_fixture_with(Some(CoordinatorProbe {
            submit_arrival: Some(submit_arrival.clone()),
            mcp_failure_drain_gate: Some(failure_gate.clone()),
            ..CoordinatorProbe::default()
        }))
        .await;

        let failure_inner = fixture.runtime.inner.clone();
        let failure = tokio::spawn(async move {
            failure_inner.fence_mcp_settlement_failure("atomic MCP failure".to_owned());
        });
        failure_gate.wait_entered();
        assert_eq!(
            fixture.runtime.lifecycle_state(),
            ConversationLifecycleState::Draining,
            "the lifecycle transition is already published while the failure critical section is parked"
        );

        let submit_runtime = fixture.runtime.clone();
        let (submit_tx, mut submit_rx) = tokio::sync::oneshot::channel();
        let submit = tokio::spawn(async move {
            let _ = submit_tx.send(submit_runtime.submit_inbound(text_content("late inbound")));
        });
        submit_arrival.notified().await;
        assert!(
            matches!(
                submit_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "the competing submit reached the boundary but cannot cross the held coordinator lock"
        );

        failure_gate.release();
        failure.await.expect("failure callback task");
        assert_eq!(
            fixture.runtime.lifecycle_state(),
            ConversationLifecycleState::Draining
        );
        assert_eq!(
            submit_rx.await.expect("submit result"),
            Err(InboundAdmissionError::Shutdown)
        );
        submit.await.expect("submit task");
        assert_eq!(
            fixture
                .runtime
                .tool_runtime()
                .durable_store()
                .load_pending()
                .expect("pending inbox")
                .len(),
            0,
            "the post-failure inbound was never durably accepted"
        );

        let shutdown = fixture.runtime.shutdown().await;
        let Err(super::ShutdownError::RuntimeOwnedSettlement { detail }) = shutdown else {
            panic!("the latched failure must remain shutdown evidence: {shutdown:?}");
        };
        assert!(detail.contains("atomic MCP failure"));
    }

    /// A worker that is already poised to admit pending work cannot publish a
    /// new current attempt after the unified MCP failure transition wins.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mcp_failure_closes_attempt_admission_before_worker_boundary() {
        let admission_gate = Arc::new(super::Gate::default());
        let failure_gate = Arc::new(super::Gate::default());
        let fixture = headless_fixture_with(Some(CoordinatorProbe {
            admission_gate: Some(admission_gate.clone()),
            mcp_failure_drain_gate: Some(failure_gate.clone()),
            ..CoordinatorProbe::default()
        }))
        .await;

        admission_gate.arm();
        fixture
            .runtime
            .submit_inbound(text_content("pending before failure"))
            .expect("inbound is accepted before the failure wins");
        admission_gate.wait_entered();

        failure_gate.arm();
        let failure_inner = fixture.runtime.inner.clone();
        let failure = tokio::spawn(async move {
            failure_inner.fence_mcp_settlement_failure("attempt admission MCP failure".to_owned());
        });
        failure_gate.wait_entered();
        assert_eq!(
            fixture.runtime.lifecycle_state(),
            ConversationLifecycleState::Draining
        );
        assert!(
            fixture.model.requests().is_empty(),
            "the provider request count remains zero while admission is closed"
        );

        failure_gate.release();
        failure.await.expect("failure callback task");
        assert!(!fixture.runtime.has_current_attempt());
        admission_gate.release();
        let shutdown = fixture.runtime.shutdown().await;
        let Err(super::ShutdownError::RuntimeOwnedSettlement { detail }) = shutdown else {
            panic!("the latched failure must remain shutdown evidence: {shutdown:?}");
        };
        assert!(detail.contains("attempt admission MCP failure"));
        assert!(!fixture.runtime.has_current_attempt());
        assert!(fixture.model.requests().is_empty());
    }

    /// Lifecycle Draining remains the generic admission authority for manual
    /// compaction after MCP failure publication; no compaction owner is
    /// created and no second MCP-specific gate is consulted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mcp_failure_blocks_manual_compaction_through_lifecycle() {
        let fixture = headless_fixture().await;
        fixture
            .runtime
            .inner
            .fence_mcp_settlement_failure("compaction MCP failure".to_owned());
        assert_eq!(
            fixture.runtime.compact_context().await,
            Err(super::ManualCompactionError::Shutdown)
        );
        assert!(!fixture.runtime.has_manual_compaction());

        let shutdown = fixture.runtime.shutdown().await;
        let Err(super::ShutdownError::RuntimeOwnedSettlement { detail }) = shutdown else {
            panic!("the latched failure must remain shutdown evidence: {shutdown:?}");
        };
        assert!(detail.contains("compaction MCP failure"));
    }

    /// Issue #63 (Finding 5 / Important 5): a transient select storage
    /// failure follows the bounded retry and completes a fresh admission
    /// cycle; the pending item is admitted exactly once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn transient_select_failure_recovers_and_admits_once() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_b49ab8ad-7952-716b-8f30-a0e5f0375fd7",
            ))
            .expect("in-memory store"),
        );
        let (runtime, _model) = headless_runtime_over_store(
            &dir,
            "conv_b49ab8ad-7952-716b-8f30-a0e5f0375fd7",
            store.clone(),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // One select fault: the first admission fails, the bounded retry
        // succeeds.
        store.arm_fail_select_times(1);
        let admission = runtime
            .submit_inbound(text_content("item"))
            .expect("accepted");
        assert_eq!(admission.inbound_sequence.get(), 1);

        // The transient failure is surfaced exactly once as a DurableFailure.
        let observations = await_observation(pending.as_ref(), |o| {
            matches!(o, ConversationObservation::DurableFailure { .. })
        })
        .await;
        assert_eq!(
            observations
                .iter()
                .filter(|o| matches!(o, ConversationObservation::DurableFailure { .. }))
                .count(),
            1,
            "exactly one transient DurableFailure observation"
        );

        // The retry adopts the pending item exactly once and completes the
        // admission cycle (settlement completes).
        runtime.settlement_signal().notified().await;
        let ledger = runtime.coordinator_ledger().expect("settled");
        assert!(
            ledger.iter().any(|m| matches!(
                m,
                MessageBlock::User(user) if user.id == admission.message_id
            )),
            "the pending item was adopted exactly once"
        );
        assert!(
            store.load_pending().expect("load pending").is_empty(),
            "no pending item remains after the successful retry"
        );
    }

    /// Issue #63 (Important 5): a persistent select storage failure moves the
    /// runtime into the explicit `DurabilityFailed` state after the bounded
    /// retry; pending work remains intact, no attempt is admitted, and a
    /// later durable mutation fails typed (no false healthy state).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn persistent_select_failure_enters_durability_failed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_2765079b-5cc3-7da8-81c2-537c52f240cc",
            ))
            .expect("in-memory store"),
        );
        let (runtime, _model) = headless_runtime_over_store(
            &dir,
            "conv_2765079b-5cc3-7da8-81c2-537c52f240cc",
            store.clone(),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // Arm two select faults: the first fails and re-kicks exactly once;
        // the retry fails again → explicit DurabilityFailed, no hot loop.
        store.arm_fail_select_times(2);
        let admission = runtime
            .submit_inbound(text_content("item"))
            .expect("accepted");
        assert_eq!(admission.inbound_sequence.get(), 1);

        let observations = await_observation(pending.as_ref(), |o| {
            matches!(o, ConversationObservation::DurabilityFailed { operation, .. }
                if operation == "select_pending_batch")
        })
        .await;
        assert!(
            observations
                .iter()
                .any(|o| matches!(o, ConversationObservation::DurabilityFailed { .. })),
            "the persistent failure is surfaced as an explicit degraded state"
        );

        // Pending remains durably intact, no attempt was admitted, and a
        // later mutation requiring durability fails typed.
        assert_eq!(
            store.load_pending().expect("load pending").len(),
            1,
            "the persistent failure left the accepted pending item intact"
        );
        assert!(
            !runtime.has_current_attempt(),
            "no attempt was admitted through a failed selection"
        );
        assert!(matches!(
            runtime.submit_inbound(text_content("late")),
            Err(InboundAdmissionError::DurabilityFailed { .. })
        ));
    }

    /// Issue #63 (retry domain): two independent first failures of
    /// *different* durable stages never combine into a false
    /// `DurabilityFailed` — the finite admission-cycle budget gives select
    /// and adopt independent allowances, so the adopt failure gets its own
    /// bounded retry after the select retry already succeeded; the item is
    /// then admitted exactly once and the cycle completes normally.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn select_then_adopt_failures_get_independent_bounded_retries() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_b4d6ae75-9048-7504-8dad-c0ed48da2f01",
            ))
            .expect("in-memory store"),
        );
        let (runtime, _model) = headless_runtime_over_store(
            &dir,
            "conv_b4d6ae75-9048-7504-8dad-c0ed48da2f01",
            store.clone(),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // One select fault and one adopt fault: pass 1 consumes the select
        // retry allowance and re-kicks; pass 2 selects successfully (the
        // select allowance remains consumed) but fails the adopt, which
        // consumes adopt's independent allowance; pass 3 adopts successfully
        // and completes the cycle before admitting the item.
        store.arm_fail_select_times(1);
        store.arm_fail_adopt_times(1);
        let admission = runtime
            .submit_inbound(text_content("item"))
            .expect("accepted");
        assert_eq!(admission.inbound_sequence.get(), 1);

        runtime.settlement_signal().notified().await;
        let ledger = runtime.coordinator_ledger().expect("settled");
        assert!(
            ledger.iter().any(|m| matches!(
                m,
                MessageBlock::User(user) if user.id == admission.message_id
            )),
            "the pending item was adopted exactly once after the independent retries"
        );
        assert!(
            store.load_pending().expect("load pending").is_empty(),
            "no pending item remains after the successful retries"
        );
        // Every durability observation is published during admission,
        // strictly before the settlement handoff observed above.
        let observations = pending.drain();
        let transient = observations
            .iter()
            .filter(|o| matches!(o, ConversationObservation::DurableFailure { .. }))
            .count();
        assert_eq!(
            transient, 2,
            "exactly two transient failures (one per operation), never a combined failure"
        );
        assert!(
            !observations
                .iter()
                .any(|o| matches!(o, ConversationObservation::DurabilityFailed { .. })),
            "two independent first failures must not become DurabilityFailed"
        );
    }

    /// Issue #63 (retry-cycle budget): alternating transient failures cannot
    /// keep replacing one another's retry debt. The deterministic ordered
    /// fault script forces exactly this admission sequence:
    ///
    /// ```text
    /// select fails -> select succeeds -> adopt fails -> select fails again
    /// ```
    ///
    /// The second select failure exhausts the select allowance retained from
    /// the first operation, so the runtime fails closed before any attempt is
    /// admitted. The test uses the real Runtime Client projection to prove
    /// the explicit degraded state is observable; the timeout only guards
    /// against a deadlocked or non-progressing test.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn alternating_select_adopt_failures_exhaust_one_admission_cycle() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_97aa4ab9-a684-7a4a-8846-b45621bf8052",
            ))
            .expect("in-memory store"),
        );
        let (runtime, model) = headless_runtime_over_store(
            &dir,
            "conv_97aa4ab9-a684-7a4a-8846-b45621bf8052",
            store.clone(),
        )
        .await;
        let host = crate::runtime_client::RuntimeClientHost::new(
            crate::runtime_client::RuntimeClientHostConfig {
                runtime: runtime.clone(),
                replay_limit: None,
            },
        )
        .expect("runtime client host");
        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach runtime client");
        let subscription = attachment
            .subscribe_events(crate::runtime_client::RuntimeClientCursor::new(0))
            .expect("subscribe runtime client");
        runtime.activate();

        // Activation's empty admission cycle is complete before this script
        // is armed. The pending item therefore drives exactly the four
        // operations named above.
        store.arm_admission_fault_script([
            crate::durable::sqlite::AdmissionFaultOperation::SelectPendingBatch,
            crate::durable::sqlite::AdmissionFaultOperation::AdoptPendingBatch,
            crate::durable::sqlite::AdmissionFaultOperation::SelectPendingBatch,
        ]);
        runtime
            .submit_inbound(text_content("item"))
            .expect("accepted");

        let failure = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match subscription.next().await {
                    crate::runtime_client::EventDelivery::Event(event)
                        if matches!(
                            &event.event,
                            crate::runtime_client::RuntimeClientEvent::RuntimeDurabilityFailed {
                                operation,
                                ..
                            } if operation == "select_pending_batch"
                        ) =>
                    {
                        break event;
                    }
                    crate::runtime_client::EventDelivery::Event(_) => {}
                    delivery => panic!("unexpected terminal client delivery: {delivery:?}"),
                }
            }
        })
        .await
        .expect("the alternating fault sequence reaches explicit failure");
        assert!(matches!(
            failure.event,
            crate::runtime_client::RuntimeClientEvent::RuntimeDurabilityFailed {
                operation,
                ..
            } if operation == "select_pending_batch"
        ));

        // The ordered script was fully consumed, proving the intended
        // deterministic operation sequence rather than a persistent
        // same-operation shortcut.
        assert!(
            store
                .admission_fault_script
                .lock()
                .expect("admission fault script lock")
                .is_empty(),
            "select fail -> select success -> adopt fail -> select fail"
        );

        let (snapshot, _) = host.snapshot().expect("client snapshot");
        assert_eq!(
            snapshot
                .durability_failure
                .as_ref()
                .map(|failure| failure.operation.as_str()),
            Some("select_pending_batch"),
            "the Runtime Client projection exposes the explicit degraded state"
        );
        assert_eq!(
            store.load_pending().expect("load pending").len(),
            1,
            "the accepted pending item remains durable and intact"
        );
        assert!(
            !runtime.has_current_attempt(),
            "no attempt is admitted through the failed adoption cycle"
        );
        assert!(
            model.requests().is_empty(),
            "the failed admission cycle never reaches the model"
        );
        assert!(matches!(
            runtime.submit_inbound(text_content("late")),
            Err(InboundAdmissionError::DurabilityFailed { .. })
        ));
        assert!(matches!(
            subscription.try_next(),
            crate::runtime_client::EventDelivery::Pending
        ));
    }

    /// Issue #63 (retry domain): a persistent adopt storage failure moves
    /// the runtime into the explicit `DurabilityFailed(adopt_pending_batch)`
    /// state after its own bounded retry; the pending work remains intact,
    /// no attempt is admitted, and a later durable mutation fails typed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn persistent_adopt_failure_enters_durability_failed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_51dc7c9c-4b76-7f38-861b-49d80370e1b8",
            ))
            .expect("in-memory store"),
        );
        let (runtime, _model) = headless_runtime_over_store(
            &dir,
            "conv_51dc7c9c-4b76-7f38-861b-49d80370e1b8",
            store.clone(),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // Two adopt faults: pass 1 selects successfully but fails the adopt
        // (the adopt allowance is consumed and a re-kick is armed); the retry
        // fails the same stage again -> explicit DurabilityFailed, no hot
        // loop.
        store.arm_fail_adopt_times(2);
        let admission = runtime
            .submit_inbound(text_content("item"))
            .expect("accepted");
        assert_eq!(admission.inbound_sequence.get(), 1);

        let observations = await_observation(pending.as_ref(), |o| {
            matches!(o, ConversationObservation::DurabilityFailed { operation, .. }
                if operation == "adopt_pending_batch")
        })
        .await;
        let transient = observations
            .iter()
            .filter(|o| matches!(o, ConversationObservation::DurableFailure { .. }))
            .count();
        assert_eq!(
            transient, 1,
            "exactly one transient failure before the second adopt-stage failure"
        );

        // Pending remains durably intact, no attempt was admitted, and a
        // later durable mutation fails typed.
        assert_eq!(
            store.load_pending().expect("load pending").len(),
            1,
            "the persistent failure left the accepted pending item intact"
        );
        assert!(
            !runtime.has_current_attempt(),
            "no attempt was admitted through a failed adoption"
        );
        assert!(matches!(
            runtime.submit_inbound(text_content("late")),
            Err(InboundAdmissionError::DurabilityFailed { .. })
        ));
    }

    /// Issue #63 (active-attempt durability audit) + Issue #12 (M9b): when
    /// the active attempt hits a durable canonical-write failure, the
    /// attempt settles failed with the typed durable-store failure AND the
    /// coordinator records the durable-authority failure — the runtime
    /// never returns to a false `Healthy` state that admits further work as
    /// though storage were fine.
    ///
    /// Under M9b the first canonical write of an attempt is the model-turn
    /// start transaction (request-scoped context + Request Snapshot +
    /// `ModelRequestStarted`); its failure leaves no start fact and no
    /// provider invocation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn active_attempt_durable_failure_degrades_the_runtime() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_503e0440-6f7e-731a-8571-e29416cdf3d5",
            ))
            .expect("in-memory store"),
        );
        let (runtime, model) = headless_runtime_over_store(
            &dir,
            "conv_503e0440-6f7e-731a-8571-e29416cdf3d5",
            store.clone(),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // The model-turn start transaction of the first turn fails durably.
        store.arm_request_start_fault_script([
            crate::durable::sqlite::RequestStartFaultOperation::BeforeContextAppend,
        ]);
        let admission = runtime
            .submit_inbound(text_content("item"))
            .expect("accepted");
        assert_eq!(admission.inbound_sequence.get(), 1);

        // The coordinator enters the explicit DurabilityFailed state for
        // the active attempt's durable commit failure.
        let observations = await_observation(pending.as_ref(), |o| {
            matches!(o, ConversationObservation::DurabilityFailed { operation, .. }
                if operation == "request_start")
        })
        .await;
        assert!(
            observations.iter().any(|o| matches!(
                o,
                ConversationObservation::Event { event, .. }
                    if matches!(
                        event,
                        crate::events::types::RuntimeEvent::AttemptFailed {
                            error: crate::events::types::AttemptFailure::Runtime {
                                error: crate::runtime::types::RuntimeError::DurableStore { .. },
                            },
                            ..
                        }
                    )
            )),
            "the attempt settled failed with the typed durable-store failure"
        );

        // No false healthy progress: new durable work is refused typed and
        // no further attempt is admitted.
        assert!(matches!(
            runtime.submit_inbound(text_content("late")),
            Err(InboundAdmissionError::DurabilityFailed { .. })
        ));
        assert!(
            model.requests().is_empty(),
            "the durable fault struck the model-turn start transaction, so the \
             attempt failed before its first model request and no provider \
             invocation escaped the failed start"
        );

        // The durable Ledger contains the adopted inbound but never the
        // failed start's request-scoped context: memory and durability
        // stayed consistent (a failed durable commit installed nothing).
        let canonical = store.load_canonical().expect("load canonical");
        assert_eq!(
            canonical.len(),
            1,
            "only the adopted inbound is durable; the failed start committed nothing"
        );
        assert!(
            matches!(&canonical[0], MessageBlock::User(user) if user.id == admission.message_id),
            "the adopted inbound survived intact"
        );
    }

    /// A terminal Event Journal failure must degrade the owning runtime while
    /// leaving the execution settlement candidate and the durable Journal
    /// truthful: no observer or local read model may see a terminal event that
    /// `SQLite` rejected.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn terminal_event_durable_failure_degrades_without_fabricated_terminal() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_8a56d655-7b4e-7b7a-870c-20e586d7d16f",
            ))
            .expect("in-memory store"),
        );
        let (runtime, model) = headless_runtime_over_store(
            &dir,
            "conv_8a56d655-7b4e-7b7a-870c-20e586d7d16f",
            store.clone(),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        store.arm_fail_next_terminal_event();
        let admission = runtime
            .submit_inbound(text_content("terminal fault"))
            .expect("accepted");
        let observations = await_observation(pending.as_ref(), |observation| {
            matches!(
                observation,
                ConversationObservation::DurabilityFailed { operation, .. }
                    if operation == "event_journal"
            )
        })
        .await;

        assert_eq!(
            model.requests().len(),
            1,
            "the model turn completed normally"
        );
        assert!(
            !observations.iter().any(|observation| matches!(
                observation,
                ConversationObservation::Event { event, .. } if is_terminal_event(event)
            )),
            "the rejected terminal candidate never reaches the runtime observer"
        );
        let journal = store.read_events(None, 128).expect("event journal").events;
        assert!(
            !journal
                .iter()
                .any(|envelope| is_terminal_event(&envelope.event)),
            "the failed terminal transaction leaves no durable terminal event"
        );
        assert!(
            store
                .load_canonical()
                .expect("canonical ledger")
                .iter()
                .any(|message| {
                    matches!(message, MessageBlock::User(user) if user.id == admission.message_id)
                })
        );
        assert_eq!(store.terminal_event_attempts(), 1);
        assert!(!runtime.has_current_attempt());
        assert!(matches!(
            runtime.submit_inbound(text_content("late")),
            Err(InboundAdmissionError::DurabilityFailed { .. })
        ));
    }

    /// A gated background executor for the owning-runtime degradation
    /// regression: signals its start, parks until released, then returns a
    /// fixed success result.
    struct GatedBackgroundExecutor {
        started: tokio::sync::watch::Sender<bool>,
        release: tokio::sync::watch::Sender<bool>,
    }

    impl GatedBackgroundExecutor {
        fn new() -> (
            Self,
            tokio::sync::watch::Receiver<bool>,
            tokio::sync::watch::Sender<bool>,
        ) {
            let (started, started_rx) = tokio::sync::watch::channel(false);
            let (release, _release_rx) = tokio::sync::watch::channel(false);
            (
                Self {
                    started,
                    release: release.clone(),
                },
                started_rx,
                release,
            )
        }
    }

    impl crate::tools::executor::ToolExecutor for GatedBackgroundExecutor {
        fn start<'a>(
            &'a self,
            _invocation: crate::tools::types::ToolInvocation,
            context: crate::tools::executor::ToolExecutionContext<'a>,
        ) -> crate::tools::executor::ToolExecutionHandle<'a> {
            let started = self.started.clone();
            let mut release = self.release.subscribe();
            crate::tools::executor::ToolExecutionHandle::settled_by_operation(
                Box::pin(async move {
                    started.send_replace(true);
                    release
                        .wait_for(|released| *released)
                        .await
                        .expect("release channel stays open");
                    crate::tools::types::ToolExecutionResult {
                        status: crate::tools::types::ToolExecutionStatus::Success,
                        content: Vec::new(),
                        duration_ms: 0,
                        exit_code: None,
                        artifacts: Vec::new(),
                        truncation: None,
                        workflow: None,
                        managed_output: None,
                    }
                }),
                context.cancellation.clone(),
            )
        }

        fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
            crate::tools::deadline::ToolProgressCapability::None
        }
    }

    /// Commits one conversation-owned background execution through the
    /// authoritative registry of the runtime's tool runtime.
    fn commit_background(
        runtime: &ConversationRuntime,
        executor: &Arc<dyn crate::tools::executor::ToolExecutor>,
        call_id: &str,
    ) -> crate::runtime::identity::ToolExecutionId {
        let invocation = crate::tools::types::ToolInvocation {
            id: crate::tools::types::ToolInvocationId::Agent {
                call_id: ToolCallId::new(call_id),
            },
            tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
            tool_name: "bash".to_owned(),
            mode: crate::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let prepared = runtime
            .tool_runtime()
            .background()
            .prepare_dispatch(
                &invocation,
                executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
            runtime
                .tool_runtime()
                .background()
                .commit_dispatch(
                    prepared,
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .expect("commit")
        else {
            panic!("accepted");
        };
        execution_id
    }

    /// Issue #60: after the subagent registry exhausts its bounded terminal
    /// publication budget, the owning runtime (not the registry) becomes the
    /// explicit durability-health owner. A first transient publication fault
    /// is retried without degrading the runtime; exhaustion retains the
    /// candidate as unresolved and rejects later durable mutations.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn exhausted_subagent_publication_degrades_the_owning_runtime() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_fb18687a-681e-748a-85a6-401be78c689e",
            ))
            .expect("in-memory store"),
        );
        let admission_gate = Arc::new(super::Gate::default());
        let (runtime, _model, subagents) = headless_runtime_over_store_with_subagents(
            &dir,
            "conv_fb18687a-681e-748a-85a6-401be78c689e",
            store.clone(),
            Some(admission_gate.clone()),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();
        let _admission_release = admission_gate.arm_scoped();
        runtime
            .submit_inbound(text_content("hold admission worker"))
            .expect("hold inbound");
        within_liveness_guard(
            "subagent runtime admission worker gate",
            tokio::task::spawn_blocking({
                let admission_gate = admission_gate.clone();
                move || admission_gate.wait_entered()
            }),
        )
        .await
        .expect("admission gate task");

        // One transient terminal-publication fault is retried successfully;
        // the owning runtime remains healthy.
        let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("child-one"));
        subagents.push_staged_override(staged);
        let accepted = match subagents
            .commit(
                subagents
                    .prepare(
                        &crate::runtime::subagent::SubagentStartSpec {
                            authority: crate::runtime::subagent::DurableAgentAuthority {
                                execution_policy:
                                    crate::runtime::subagent::InheritedExecutionPolicy::default(),
                                resolved: test_resolved_subagent("explore"),
                                approval_mode: crate::runtime::ApprovalMode::Policy,
                            },
                            admission: crate::runtime::subagent::ActivationAdmission {
                                task: "first terminal".to_owned(),
                                context: None,
                                origin:
                                    crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                        tool_call_id: ToolCallId::new("call-subagent-one"),
                                    },
                                terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                            },
                        },
                        &crate::runtime::cancellation::CancellationSignal::new(),
                    )
                    .await
                    .expect("prepare first"),
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect("commit first")
        {
            crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
            crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
                panic!("first subagent unexpectedly rolled back")
            }
        };
        assert!(matches!(
            crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
                .await
                .expect("delegate frame"),
            Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
        ));
        store.arm_fail_accept_times(1);
        crate::runtime::subagent::ipc::write_child_frame(
            &mut peer,
            &crate::runtime::subagent::ipc::ChildFrame::Result(
                crate::runtime::subagent::ipc::ResultFrame {
                    status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                    content: Some("first".to_owned()),
                    diagnostic: None,
                },
            ),
        )
        .await
        .expect("first result");
        let first = subagents
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("first settles");
        assert_eq!(
            first.state,
            crate::runtime::subagent::SubagentState::Succeeded
        );
        assert!(
            runtime.inner.durability_failure_diagnostic().is_none(),
            "one transient publication fault does not degrade the runtime"
        );

        // The initial publication plus both bounded retries now fail. The
        // worker is still parked, so no unrelated durable acceptance can
        // consume the scripted storage faults.
        let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("child-two"));
        subagents.push_staged_override(staged);
        let accepted = match subagents
            .commit(
                subagents
                    .prepare(
                        &crate::runtime::subagent::SubagentStartSpec {
                            authority: crate::runtime::subagent::DurableAgentAuthority {
                                execution_policy:
                                    crate::runtime::subagent::InheritedExecutionPolicy::default(),
                                resolved: test_resolved_subagent("explore"),
                                approval_mode: crate::runtime::ApprovalMode::Policy,
                            },
                            admission: crate::runtime::subagent::ActivationAdmission {
                                task: "second terminal".to_owned(),
                                context: None,
                                origin:
                                    crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                        tool_call_id: ToolCallId::new("call-subagent-two"),
                                    },
                                terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                            },
                        },
                        &crate::runtime::cancellation::CancellationSignal::new(),
                    )
                    .await
                    .expect("prepare second"),
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect("commit second")
        {
            crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
            crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
                panic!("second subagent unexpectedly rolled back")
            }
        };
        assert!(matches!(
            crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
                .await
                .expect("delegate frame"),
            Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
        ));
        let publication_gate = Arc::new(super::Gate::default());
        let _publication_release = publication_gate.arm_scoped();
        runtime
            .inner
            .probe
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .subagent_failure_published_gate = Some(publication_gate.clone());
        store.arm_fail_accept_times(3);
        crate::runtime::subagent::ipc::write_child_frame(
            &mut peer,
            &crate::runtime::subagent::ipc::ChildFrame::Result(
                crate::runtime::subagent::ipc::ResultFrame {
                    status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                    content: Some("second".to_owned()),
                    diagnostic: None,
                },
            ),
        )
        .await
        .expect("second result");
        let observations = await_observation(&pending, |observation| {
            matches!(
                observation,
                ConversationObservation::DurabilityFailed { operation, .. }
                    if operation == "subagent_terminal_publication"
            )
        })
        .await;
        assert_eq!(
            observations
                .iter()
                .filter(|observation| matches!(
                    observation,
                    ConversationObservation::DurabilityFailed { operation, .. }
                        if operation == "subagent_terminal_publication"
                ))
                .count(),
            1,
            "exhaustion degrades the runtime once"
        );
        tokio::task::spawn_blocking({
            let gate = publication_gate.clone();
            move || gate.wait_entered()
        })
        .await
        .unwrap();
        assert!(
            !subagents
                .snapshot(&accepted.subagent_id)
                .unwrap()
                .publication_abandoned,
            "runtime health publication precedes registry settlement"
        );
        let settled = subagents.wait_until_settled(&accepted.subagent_id);
        tokio::pin!(settled);
        assert!(
            futures_util::poll!(&mut settled).is_pending(),
            "health observation alone cannot complete terminal settlement"
        );
        publication_gate.release();
        let unresolved = settled.await.expect("second settlement");
        assert_eq!(
            unresolved.state,
            crate::runtime::subagent::SubagentState::PublishingTerminal
        );
        assert!(unresolved.publication_abandoned);
        // Issue #178: the successful answer never rides the live read
        // model, not even while its publication is unresolved; the
        // candidate remains observable through the PublishingTerminal
        // lifecycle state itself.
        assert!(unresolved.detail.is_none());
        let pending_items = store
            .select_pending_batch()
            .expect("pending")
            .map(|batch| batch.items)
            .unwrap_or_default();
        assert!(
            !pending_items.iter().any(|item| {
                item.correlation.as_deref()
                    == Some(
                        crate::runtime::subagent::terminal_correlation(&accepted.subagent_id)
                            .as_str(),
                    )
            }),
            "no false terminal inbound was committed"
        );
        assert!(matches!(
            runtime.submit_inbound(text_content("late")),
            Err(InboundAdmissionError::DurabilityFailed { .. })
        ));

        admission_gate.release();
        let shutdown = runtime.shutdown().await;
        assert!(matches!(
            shutdown,
            Err(super::ShutdownError::RuntimeOwnedSettlement { detail })
                if detail.contains("subagent")
        ));
    }

    /// Issue #385: a running subagent is the busy owner of the native
    /// configuration-adoption gate. The gate reports Busy from the real
    /// registry lifecycle, explicit adoption is refused typed, and the exact
    /// child settlement restores Eligible with the pending candidate
    /// untouched — no automatic adoption, no model request, no rebinding.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // scenario bodies are deliberately linear
    async fn subagent_settlement_restores_configuration_adoption_eligibility() {
        use crate::local_runtime::configuration::application::{
            AdoptionEligibility, AdoptionError, PreparedConfiguration,
        };
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_385a385a-3850-7385-8385-385385385385",
            ))
            .expect("in-memory store"),
        );
        let (runtime, model, subagents) = headless_runtime_over_store_with_subagents(
            &dir,
            "conv_385a385a-3850-7385-8385-385385385385",
            store.clone(),
            None,
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // A pending, ready candidate owned by this runtime at the current
        // binding baseline (the same shape model_set commits).
        let mut prepared = {
            let state = runtime.inner.lock_state();
            Some(PreparedConfiguration {
                selection_only: true,
                owner: runtime.inner.configuration_owner.clone(),
                baseline: state.binding_revision,
                preview: state.resources.clone(),
                model: state.model.clone(),
                capability: None,
                resources: None,
                impact: crate::model::request_shape::CacheImpact::Unproven,
            })
        };
        let baseline = prepared.as_ref().expect("candidate").baseline;
        assert_eq!(
            runtime.configuration_adoption_eligibility(),
            AdoptionEligibility::Eligible
        );

        // The busy owner is a real Running child record: committed through
        // the registry and delegated over the staged control channel.
        let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("busy-child"));
        subagents.push_staged_override(staged);
        let accepted = match subagents
            .commit(
                subagents
                    .prepare(
                        &crate::runtime::subagent::SubagentStartSpec {
                            authority: crate::runtime::subagent::DurableAgentAuthority {
                                execution_policy:
                                    crate::runtime::subagent::InheritedExecutionPolicy::default(),
                                resolved: test_resolved_subagent("explore"),
                                approval_mode: crate::runtime::ApprovalMode::Policy,
                            },
                            admission: crate::runtime::subagent::ActivationAdmission {
                                task: "hold the adoption gate".to_owned(),
                                context: None,
                                origin:
                                    crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                        tool_call_id: ToolCallId::new("call-busy-subagent"),
                                    },
                                terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                            },
                        },
                        &crate::runtime::cancellation::CancellationSignal::new(),
                    )
                    .await
                    .expect("prepare"),
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect("commit")
        {
            crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
            crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
                panic!("subagent unexpectedly rolled back")
            }
        };
        assert!(matches!(
            crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
                .await
                .expect("delegate frame"),
            Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
        ));
        // Attribution: no foreground attempt and the background plane owns
        // nothing; the subagent registry alone owns the Busy lifecycle.
        assert!(!runtime.tool_runtime().background().configuration_busy());
        assert!(subagents.configuration_busy());
        assert_eq!(runtime.idle_epoch(), Err(super::IdleBusyReason::Subagent));
        assert_eq!(
            runtime.configuration_adoption_eligibility(),
            AdoptionEligibility::Busy
        );
        assert_eq!(
            runtime.adopt_configuration(&mut prepared, baseline, true, || Ok(())),
            Err(AdoptionError::Busy)
        );
        assert!(prepared.is_some(), "a Busy refusal retains the candidate");

        // The exact child settles through its real terminal publication.
        crate::runtime::subagent::ipc::write_child_frame(
            &mut peer,
            &crate::runtime::subagent::ipc::ChildFrame::Result(
                crate::runtime::subagent::ipc::ResultFrame {
                    status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                    content: Some("done".to_owned()),
                    diagnostic: None,
                },
            ),
        )
        .await
        .expect("child result");
        let settled = within_liveness_guard(
            "busy subagent settlement",
            subagents.wait_until_settled(&accepted.subagent_id),
        )
        .await
        .expect("child settles");
        assert_eq!(
            settled.state,
            crate::runtime::subagent::SubagentState::Succeeded
        );
        assert!(!subagents.configuration_busy());
        // No notification fires for busy -> idle: eligibility is computed at
        // read time, and the settling child releases its lifecycle admission
        // guard after the record is already terminal, so poll the read inside
        // the liveness guard.
        within_liveness_guard("eligibility after subagent settlement", async {
            loop {
                if runtime.configuration_adoption_eligibility() == AdoptionEligibility::Eligible {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(runtime.idle_epoch().is_ok());

        // The pending candidate survived untouched: the binding baseline
        // never advanced (no automatic adoption) and nothing re-prepared it.
        assert_eq!(runtime.inner.lock_state().binding_revision, baseline);
        assert_eq!(
            prepared.as_ref().expect("candidate retained").baseline,
            baseline
        );
        // The only model activity across settlement is the orphan result's
        // own continuation: the runtime consumes the terminal notice with one
        // attempt. The eligibility transition itself invokes no model.
        let requests = model.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert!(
            format!("{:?}", requests[0]).contains("terminal-notice"),
            "the single request is the subagent terminal continuation: {:?}",
            requests[0]
        );
        assert!(runtime.inner.durability_failure_diagnostic().is_none());
        // Fencing still rejects a stale binding after eligibility returns.
        assert_eq!(
            runtime.adopt_configuration(&mut prepared, baseline + 1, true, || Ok(())),
            Err(AdoptionError::Conflict)
        );
        assert!(
            prepared.is_some(),
            "a Conflict refusal retains the candidate"
        );
        assert_eq!(
            runtime.adopt_configuration(&mut prepared, baseline, true, || Ok(())),
            Ok(baseline + 1)
        );
        assert!(
            prepared.is_none(),
            "the successful adoption consumed the candidate"
        );
        assert_eq!(runtime.inner.lock_state().binding_revision, baseline + 1);
        assert_eq!(
            model.requests().len(),
            1,
            "adoption invoked no model request"
        );
    }

    /// DurabilityFailed-first for subagents (Issue #60): once the owning
    /// runtime commits `DurabilityFailed` through the real subagent
    /// terminal-publication exhaustion path, a new subagent start is
    /// refused at the runtime durability frontier — the commit rejects
    /// typed, the staged child is torn down conclusively, no
    /// `SubagentOwnershipCommitted` fact and no Running record exist, no
    /// Delegate is ever sent, and the existing unresolved candidate stays
    /// unchanged.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn a_durability_failed_runtime_rejects_new_subagent_ownership() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_95189f41-6174-741a-8c26-0134828bfe76",
            ))
            .expect("in-memory store"),
        );
        let admission_gate = Arc::new(super::Gate::default());
        let (runtime, _model, subagents) = headless_runtime_over_store_with_subagents(
            &dir,
            "conv_95189f41-6174-741a-8c26-0134828bfe76",
            store.clone(),
            Some(admission_gate.clone()),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();
        let _admission_release = admission_gate.arm_scoped();
        runtime
            .submit_inbound(text_content("hold admission worker"))
            .expect("hold inbound");
        within_liveness_guard(
            "subagent runtime admission worker gate",
            tokio::task::spawn_blocking({
                let admission_gate = admission_gate.clone();
                move || admission_gate.wait_entered()
            }),
        )
        .await
        .expect("admission gate task");

        // One owned child exhausts its bounded terminal-publication budget
        // and degrades the owning runtime through the real sink path.
        let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("owned-child"));
        subagents.push_staged_override(staged);
        let owned = match subagents
            .commit(
                subagents
                    .prepare(
                        &crate::runtime::subagent::SubagentStartSpec {
                            authority: crate::runtime::subagent::DurableAgentAuthority {
                                execution_policy:
                                    crate::runtime::subagent::InheritedExecutionPolicy::default(),
                                resolved: test_resolved_subagent("explore"),
                                approval_mode: crate::runtime::ApprovalMode::Policy,
                            },
                            admission: crate::runtime::subagent::ActivationAdmission {
                                task: "owned child".to_owned(),
                                context: None,
                                origin:
                                    crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                        tool_call_id: ToolCallId::new("call-owned"),
                                    },
                                terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                            },
                        },
                        &crate::runtime::cancellation::CancellationSignal::new(),
                    )
                    .await
                    .expect("prepare owned"),
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect("commit owned")
        {
            crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
            crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
                panic!("unexpected rollback")
            }
        };
        assert!(matches!(
            crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
                .await
                .expect("delegate frame"),
            Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
        ));
        store.arm_fail_accept_times(3);
        crate::runtime::subagent::ipc::write_child_frame(
            &mut peer,
            &crate::runtime::subagent::ipc::ChildFrame::Result(
                crate::runtime::subagent::ipc::ResultFrame {
                    status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                    content: Some("owned answer".to_owned()),
                    diagnostic: None,
                },
            ),
        )
        .await
        .expect("owned result");
        let observations = await_observation(&pending, |observation| {
            matches!(
                observation,
                ConversationObservation::DurabilityFailed { operation, .. }
                    if operation == "subagent_terminal_publication"
            )
        })
        .await;
        assert_eq!(
            observations
                .iter()
                .filter(|observation| matches!(
                    observation,
                    ConversationObservation::DurabilityFailed { .. }
                ))
                .count(),
            1,
            "the exhausted budget degraded the runtime exactly once"
        );
        assert!(
            runtime.inner.durability_failure_diagnostic().is_some(),
            "the runtime committed DurabilityFailed"
        );

        // A new subagent start after the failure is refused at the runtime
        // durability frontier: `prepare` still stages privately, `commit`
        // rejects typed at the gate, and the staged child is rolled back
        // conclusively (the returned error is the typed rejection, never a
        // rollback failure).
        let (staged, _rejected_peer) = stage_runtime_test_child(&dir.path().join("rejected-child"));
        subagents.push_staged_override(staged);
        let prepared = subagents
            .prepare(
                &crate::runtime::subagent::SubagentStartSpec {
                    authority: crate::runtime::subagent::DurableAgentAuthority {
                        execution_policy:
                            crate::runtime::subagent::InheritedExecutionPolicy::default(),
                        resolved: test_resolved_subagent("explore"),
                        approval_mode: crate::runtime::ApprovalMode::Policy,
                    },
                    admission: crate::runtime::subagent::ActivationAdmission {
                        task: "rejected after failure".to_owned(),
                        context: None,
                        origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                            tool_call_id: ToolCallId::new("call-rejected"),
                        },
                        terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                    },
                },
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect("prepare rejected");
        let error = subagents
            .commit(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect_err("ownership refused after DurabilityFailed");
        assert!(
            matches!(
                error,
                crate::runtime::subagent::SubagentStartError::DurabilityFailed { .. }
            ),
            "the typed rejection names the runtime durability failure: {error}"
        );

        // No new durable ownership fact, no new Running record, no
        // Delegate: exactly the one owned child remains, its unresolved
        // candidate unchanged.
        let rejected_id = SubagentId::for_conversation(
            &ConversationId::new("conv_95189f41-6174-741a-8c26-0134828bfe76"),
            2,
        );
        let journal = store.read_events(None, 128).expect("events").events;
        assert!(
            !journal.iter().any(|envelope| matches!(
                &envelope.event,
                crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                    subagent_id,
                    ..
                } if *subagent_id == rejected_id
            )),
            "no ownership fact for the rejected child"
        );
        let snapshots = subagents.all_snapshots();
        assert_eq!(snapshots.len(), 1, "only the owned child remains");
        assert_eq!(snapshots[0].subagent_id, owned.subagent_id);
        let unresolved = subagents
            .wait_until_settled(&owned.subagent_id)
            .await
            .expect("owned settlement");
        assert_eq!(
            unresolved.state,
            crate::runtime::subagent::SubagentState::PublishingTerminal
        );
        assert!(unresolved.publication_abandoned);
        // Issue #178: the successful answer never rides the live read
        // model; the unresolved candidate is observable through its
        // PublishingTerminal lifecycle state, unchanged.
        assert!(unresolved.detail.is_none());

        admission_gate.release();
    }

    // Shared native child invariant for ordinary Subagents and Workflow Agents.
    // Parent admission is held solely to keep the fixture occurrence pending.
    fn assert_pending_mutation_preserves_child(
        runtime: &ConversationRuntime,
        subagents: &crate::runtime::subagent::SubagentRegistry,
        child_id: &crate::runtime::identity::SubagentId,
    ) {
        let _admission = runtime.inner.state.lock().expect("coordinator");
        let before = subagents.snapshot(child_id).unwrap();
        let item = runtime
            .tool_runtime()
            .durable_store()
            .accept_inbound(crate::durable::InboundDraft {
                message_id: None,
                source: UserSource::Human,
                kind: InboundKind::Message,
                content: vec![crate::message::types::UserContentBlock::Text(TextBlock {
                    text: "pending user control".into(),
                })],
                timestamp: chrono::Utc::now(),
                correlation: None,
            })
            .unwrap();
        let expected = crate::durable::inbox::PendingInboundRef {
            sequence: item.sequence,
            message_id: item.message_id,
            revision: 0,
        };
        assert_eq!(
            runtime
                .tool_runtime()
                .mailbox()
                .edit_pending(&expected, "edited pending")
                .unwrap(),
            crate::durable::inbox::PendingMutationOutcome::Applied
        );
        assert_eq!(
            runtime
                .tool_runtime()
                .mailbox()
                .remove_pending(&crate::durable::inbox::PendingInboundRef {
                    revision: 1,
                    ..expected
                })
                .unwrap(),
            crate::durable::inbox::PendingMutationOutcome::Applied
        );
        assert_eq!(subagents.snapshot(child_id).unwrap(), before);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // one ordered ownership transaction
    async fn child_transcript_invalid_limits_precede_unavailable_history_access() {
        use crate::runtime_client::RuntimeClientError;
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_c26a9aad-2921-7350-8d5a-8668bfbc4ee9",
            ))
            .expect("in-memory store"),
        );
        let (runtime, _model, subagents) = headless_runtime_over_store_with_subagents(
            &dir,
            "conv_c26a9aad-2921-7350-8d5a-8668bfbc4ee9",
            store.clone(),
            None,
        )
        .await;
        let host = crate::runtime_client::RuntimeClientHost::new(
            crate::runtime_client::RuntimeClientHostConfig {
                runtime: runtime.clone(),
                replay_limit: None,
            },
        )
        .expect("host");
        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        runtime.activate();

        // Ownership commits first, while the runtime is healthy.
        let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("owned-child"));
        subagents.push_staged_override(staged);
        let accepted = match subagents
            .commit(
                subagents
                    .prepare(
                        &crate::runtime::subagent::SubagentStartSpec {
                            authority: crate::runtime::subagent::DurableAgentAuthority {
                                execution_policy:
                                    crate::runtime::subagent::InheritedExecutionPolicy::default(),
                                resolved: test_resolved_subagent("explore"),
                                approval_mode: crate::runtime::ApprovalMode::Policy,
                            },
                            admission: crate::runtime::subagent::ActivationAdmission {
                                task: "owned".to_owned(),
                                context: None,
                                origin:
                                    crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                        tool_call_id: ToolCallId::new("call-owned"),
                                    },
                                terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                            },
                        },
                        &crate::runtime::cancellation::CancellationSignal::new(),
                    )
                    .await
                    .expect("prepare"),
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect("commit")
        {
            crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
            crate::runtime::subagent::SubagentStartOutcome::RolledBack => panic!("accepted"),
        };
        assert!(matches!(
            crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
                .await
                .expect("delegate frame"),
            Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
        ));

        // The staged child commits real ownership but never creates a child
        // database. Its existing history is therefore deterministically unavailable.
        let resolver_reads = || {
            runtime
                .inner
                .subagent_transcript_store_reads
                .load(std::sync::atomic::Ordering::Relaxed)
        };
        let unknown = crate::runtime::identity::AgentId::new("unknown-child");
        for id in [&accepted.child_agent_id, &unknown] {
            for limit in [0, crate::durable::TRANSCRIPT_PAGE_LIMIT_MAX + 1] {
                assert!(matches!(
                    attachment.agent_transcript_page(id, None, limit),
                    Err(RuntimeClientError::InvalidRequest { .. })
                ));
            }
        }
        assert_eq!(
            resolver_reads(),
            0,
            "invalid limits must not enter ownership/storage resolution"
        );
        for limit in [1, crate::durable::TRANSCRIPT_PAGE_LIMIT_MAX] {
            assert!(matches!(
                attachment.agent_transcript_page(&unknown, None, limit),
                Err(RuntimeClientError::UnknownAgent { agent_id }) if agent_id == unknown
            ));
            assert!(matches!(
                attachment.agent_transcript_page(&accepted.child_agent_id, None, limit),
                Err(RuntimeClientError::RuntimeFailure { message })
                    if message.starts_with("subagent history unavailable:")
            ));
        }
        assert_eq!(
            resolver_reads(),
            2,
            "only owned Agent identities reach history storage resolution"
        );
        let _ = subagents.cancel(&accepted.subagent_id, CancellationReason::UserRequested);
        subagents
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
    }

    /// Ownership-first for subagents (Issue #60): a child whose ownership
    /// committed while the runtime was healthy survives a later
    /// `DurabilityFailed` commit — it is not retroactively reclaimed, and
    /// it keeps its full settlement authority (cancel, escalate, reap,
    /// durable terminal publication).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_owned_subagent_survives_a_later_durability_failure() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_c26a9aad-2921-7350-8d5a-8668bfbc4ee9",
            ))
            .expect("in-memory store"),
        );
        let (runtime, _model, subagents) = headless_runtime_over_store_with_subagents(
            &dir,
            "conv_c26a9aad-2921-7350-8d5a-8668bfbc4ee9",
            store.clone(),
            None,
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // Ownership commits first, while the runtime is healthy.
        let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("owned-child"));
        subagents.push_staged_override(staged);
        let accepted = match subagents
            .commit(
                subagents
                    .prepare(
                        &crate::runtime::subagent::SubagentStartSpec {
                            authority: crate::runtime::subagent::DurableAgentAuthority {
                                execution_policy:
                                    crate::runtime::subagent::InheritedExecutionPolicy::default(),
                                resolved: test_resolved_subagent("explore"),
                                approval_mode: crate::runtime::ApprovalMode::Policy,
                            },
                            admission: crate::runtime::subagent::ActivationAdmission {
                                task: "owned".to_owned(),
                                context: None,
                                origin:
                                    crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                        tool_call_id: ToolCallId::new("call-owned"),
                                    },
                                terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                            },
                        },
                        &crate::runtime::cancellation::CancellationSignal::new(),
                    )
                    .await
                    .expect("prepare"),
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect("commit")
        {
            crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
            crate::runtime::subagent::SubagentStartOutcome::RolledBack => panic!("accepted"),
        };
        assert!(matches!(
            crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
                .await
                .expect("delegate frame"),
            Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
        ));

        assert!(
            runtime.idle_epoch().is_err(),
            "an owned Subagent prevents idle residency even without a parent attempt"
        );

        assert_pending_mutation_preserves_child(&runtime, &subagents, &accepted.subagent_id);

        // The runtime then commits DurabilityFailed; the already-owned
        // child is not retroactively reclaimed.
        runtime.force_durability_failure_for_test(
            DurableOperation::EventJournal,
            "synthetic durability failure",
        );
        await_observation(&pending, |observation| {
            matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )
        })
        .await;
        let snapshot = subagents
            .snapshot(&accepted.subagent_id)
            .expect("owned snapshot");
        assert_eq!(
            snapshot.state,
            crate::runtime::subagent::SubagentState::Running,
            "ownership survives the later failure"
        );

        // The owned child keeps its settlement authority: cancel reaches
        // the driver, the process is escalated and reaped, and the durable
        // terminal publication still succeeds (settlement is not
        // new-mutation authority).
        let _ = subagents.cancel(&accepted.subagent_id, CancellationReason::UserRequested);
        let settled = subagents
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(
            settled.state,
            crate::runtime::subagent::SubagentState::Cancelled
        );
    }

    /// The exact ownership-vs-health race for subagents (Issue #60): the
    /// ownership commit is parked inside its authoritative critical section
    /// (holding the runtime durability frontier across its durable write
    /// and record publication) while the `DurabilityFailed` commit is
    /// invoked concurrently; the failure provably blocks on the frontier,
    /// the ownership completes first, and only then does the failure
    /// linearize. The owned child survives with its settlement authority.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn subagent_ownership_racing_a_durability_failure_linearizes_on_the_runtime_frontier() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_aed9850e-213c-7f34-8a3f-fb60527fc81f",
            ))
            .expect("in-memory store"),
        );
        let (runtime, _model, subagents) = headless_runtime_over_store_with_subagents(
            &dir,
            "conv_aed9850e-213c-7f34-8a3f-fb60527fc81f",
            store.clone(),
            None,
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // The ownership commit is parked inside its authoritative critical
        // section (the CommitBoundaryHook), provably holding the runtime
        // durability frontier across the durable write and record
        // publication.
        let hook = Arc::new(crate::runtime::subagent::CommitBoundaryHook::default());
        subagents.install_commit_boundary_hook(hook.clone());
        let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("racing-child"));
        subagents.push_staged_override(staged);
        let commit_registry = subagents.clone();
        let committer = tokio::spawn(async move {
            let prepared = commit_registry
                .prepare(
                    &crate::runtime::subagent::SubagentStartSpec {
                        authority: crate::runtime::subagent::DurableAgentAuthority {
                            execution_policy:
                                crate::runtime::subagent::InheritedExecutionPolicy::default(),
                            resolved: test_resolved_subagent("explore"),
                            approval_mode: crate::runtime::ApprovalMode::Policy,
                        },
                        admission: crate::runtime::subagent::ActivationAdmission {
                            task: "racing".to_owned(),
                            context: None,
                            origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                tool_call_id: ToolCallId::new("call-racing"),
                            },
                            terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                        },
                    },
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .await
                .expect("prepare");
            commit_registry
                .commit(
                    prepared,
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::task::spawn_blocking({
                let hook = hook.clone();
                move || hook.wait_until_entered()
            }),
        )
        .await
        .expect("commit-boundary liveness")
        .expect("commit-boundary entered");

        // The DurabilityFailed commit is now invoked concurrently: it takes
        // the coordinator lock and blocks on the runtime frontier held by
        // the parked ownership commit.
        let (failed_started_tx, failed_started_rx) = std::sync::mpsc::channel();
        let (failed_done_tx, failed_done_rx) = std::sync::mpsc::channel();
        let failing_runtime = runtime.clone();
        let failure_thread = std::thread::spawn(move || {
            failed_started_tx.send(()).expect("failure-started channel");
            failing_runtime.force_durability_failure_for_test(
                DurableOperation::EventJournal,
                "synthetic durability failure",
            );
            failed_done_tx.send(()).expect("failure-done channel");
        });
        failed_started_rx
            .recv()
            .expect("failure is invoked while the ownership is parked");
        assert!(
            failed_done_rx.try_recv().is_err(),
            "the DurabilityFailed commit is provably blocked on the runtime frontier held by the parked ownership commit"
        );
        assert!(
            !pending.drain().iter().any(|observation| matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )),
            "the health failure has not linearized while the ownership commit holds the frontier"
        );

        // Release the ownership commit: it completes its durable write and
        // record publication first, then the failure commit linearizes.
        hook.release();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), committer)
            .await
            .expect("commit liveness")
            .expect("committer")
            .expect("ownership wins the frontier");
        let accepted = match outcome {
            crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
            crate::runtime::subagent::SubagentStartOutcome::RolledBack => panic!("accepted"),
        };
        await_observation(&pending, |observation| {
            matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )
        })
        .await;
        failure_thread.join().expect("failure thread joins");

        // The owned child exists with a durable ownership fact and keeps
        // its settlement authority.
        let snapshot = subagents
            .snapshot(&accepted.subagent_id)
            .expect("owned snapshot");
        assert_eq!(
            snapshot.state,
            crate::runtime::subagent::SubagentState::Running
        );
        let journal = store.read_events(None, 128).expect("events").events;
        assert!(
            journal.iter().any(|envelope| matches!(
                &envelope.event,
                crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                    subagent_id,
                    ..
                } if *subagent_id == accepted.subagent_id
            )),
            "the ownership durable fact committed before the failure"
        );
        let _ = subagents.cancel(&accepted.subagent_id, CancellationReason::UserRequested);
        let settled = subagents
            .wait_until_settled(&accepted.subagent_id)
            .await
            .expect("settled");
        assert_eq!(
            settled.state,
            crate::runtime::subagent::SubagentState::Cancelled
        );
        // The wire carried Delegate first, then the in-flight Cancel.
        assert!(matches!(
            crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
                .await
                .expect("driver frame"),
            Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
        ));
        assert!(matches!(
            crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
                .await
                .expect("driver frame"),
            Some(crate::runtime::subagent::ipc::ParentFrame::Cancel {
                reason: Some(crate::runtime::types::CancellationReason::UserRequested),
            })
        ));
    }

    /// DurabilityFailed-first for background (Issue #60): once the owning
    /// runtime commits `DurabilityFailed`, a new background ownership
    /// commit is refused at the runtime durability frontier — the prepared
    /// runner is aborted, no record and no durable fact exist.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_durability_failed_runtime_rejects_new_background_ownership() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_a2701eac-eb5d-78a0-8494-e6888fb54e26",
            ))
            .expect("in-memory store"),
        );
        let (runtime, _model) = headless_runtime_over_store(
            &dir,
            "conv_a2701eac-eb5d-78a0-8494-e6888fb54e26",
            store.clone(),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // The runtime commits DurabilityFailed first.
        runtime.force_durability_failure_for_test(
            DurableOperation::EventJournal,
            "synthetic durability failure",
        );
        await_observation(&pending, |observation| {
            matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )
        })
        .await;

        // A new background ownership commit is refused at the runtime
        // frontier: the prepared runner is aborted, no committed record and
        // no durable ownership fact exist.
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> =
            Arc::new(GatedBackgroundExecutor::new().0);
        let invocation = crate::tools::types::ToolInvocation {
            id: crate::tools::types::ToolInvocationId::Agent {
                call_id: ToolCallId::new("call-rejected-bg"),
            },
            tool_id: ToolId::new("tool-bash"),
            tool_name: "bash".to_owned(),
            mode: crate::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let prepared = runtime
            .tool_runtime()
            .background()
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let error = runtime
            .tool_runtime()
            .background()
            .commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .expect_err("background ownership refused after DurabilityFailed");
        assert!(
            matches!(
                error,
                crate::tools::background::BackgroundDispatchError::DurabilityFailed { .. }
            ),
            "the typed rejection names the runtime durability failure: {error}"
        );
        assert!(
            runtime
                .tool_runtime()
                .background()
                .all_snapshots()
                .is_empty(),
            "no committed background record"
        );
        assert!(
            store
                .read_events(None, 128)
                .expect("events")
                .events
                .iter()
                .all(|envelope| !matches!(
                    envelope.event,
                    crate::events::types::RuntimeEvent::BackgroundExecutionCommitted { .. }
                )),
            "no background ownership durable fact"
        );
    }

    /// Ownership-first for background (Issue #60): an execution whose
    /// ownership committed while the runtime was healthy survives a later
    /// `DurabilityFailed` commit and keeps its settlement authority.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_owned_background_execution_survives_a_later_durability_failure() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_e24a334b-c1b2-7efc-8376-f688cf4206bf",
            ))
            .expect("in-memory store"),
        );
        let (runtime, _model) = headless_runtime_over_store(
            &dir,
            "conv_e24a334b-c1b2-7efc-8376-f688cf4206bf",
            store.clone(),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // Ownership commits first while the runtime is healthy.
        let (executor, mut started, release) = GatedBackgroundExecutor::new();
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
        let execution_id = commit_background(&runtime, &executor, "call-owned-bg");
        tokio::time::timeout(
            std::time::Duration::from_mins(2),
            started.wait_for(|is_started| *is_started),
        )
        .await
        .expect("runner start wait exceeded liveness guard")
        .expect("start channel stays open");

        // The runtime then commits DurabilityFailed; the already-owned
        // execution is not reclaimed.
        runtime.force_durability_failure_for_test(
            DurableOperation::EventJournal,
            "synthetic durability failure",
        );
        await_observation(&pending, |observation| {
            matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )
        })
        .await;
        let snapshot = runtime
            .background_status(&execution_id)
            .expect("owned execution");
        assert_eq!(
            snapshot.state,
            crate::tools::background::BackgroundLifecycle::Running,
            "ownership survives the later failure"
        );

        // Settlement still works: the runner completes and its terminal
        // publication is durably accepted (settlement is not new-mutation
        // authority).
        release.send_replace(true);
        runtime
            .tool_runtime()
            .background()
            .wait_until_settled(&execution_id)
            .await;
        let terminal = runtime
            .background_status(&execution_id)
            .expect("settled execution");
        assert_eq!(
            terminal.state,
            crate::tools::background::BackgroundLifecycle::Succeeded
        );
    }

    /// The exact ownership-vs-health race for background (Issue #60): the
    /// background ownership commit is parked inside its authoritative
    /// critical section (holding the runtime durability frontier) while the
    /// `DurabilityFailed` commit is invoked concurrently; the failure
    /// provably blocks on the frontier, the ownership completes first, and
    /// only then does the failure linearize. The owned execution survives
    /// and settles normally.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn background_ownership_racing_a_durability_failure_linearizes_on_the_runtime_frontier() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_80c3ae25-956f-72bf-8e7b-a38546997137",
            ))
            .expect("in-memory store"),
        );
        let (runtime, _model) = headless_runtime_over_store(
            &dir,
            "conv_80c3ae25-956f-72bf-8e7b-a38546997137",
            store.clone(),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // The background ownership commit is parked inside its
        // authoritative critical section (the dispatch CommitBoundaryHook),
        // provably holding the runtime durability frontier across the
        // durable write and record publication.
        let background = runtime.tool_runtime().background().clone();
        let hook = Arc::new(crate::tools::background::test_sync::CommitBoundaryHook::default());
        background.install_commit_boundary_hook(hook.clone());
        let (executor, mut started, release) = GatedBackgroundExecutor::new();
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
        let invocation = crate::tools::types::ToolInvocation {
            id: crate::tools::types::ToolInvocationId::Agent {
                call_id: ToolCallId::new("call-racing-bg"),
            },
            tool_id: ToolId::new("tool-bash"),
            tool_name: "bash".to_owned(),
            mode: crate::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let prepared = background
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let commit_background_registry = background.clone();
        let committer = tokio::task::spawn_blocking(move || {
            commit_background_registry.commit_dispatch(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
        });
        {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || hook.wait_entered())
                .await
                .expect("the background commit entered its ownership boundary");
        }

        // The DurabilityFailed commit is now invoked concurrently: it takes
        // the coordinator lock and blocks on the runtime frontier held by
        // the parked ownership commit.
        let (failed_started_tx, failed_started_rx) = std::sync::mpsc::channel();
        let (failed_done_tx, failed_done_rx) = std::sync::mpsc::channel();
        let failing_runtime = runtime.clone();
        let failure_thread = std::thread::spawn(move || {
            failed_started_tx.send(()).expect("failure-started channel");
            failing_runtime.force_durability_failure_for_test(
                DurableOperation::EventJournal,
                "synthetic durability failure",
            );
            failed_done_tx.send(()).expect("failure-done channel");
        });
        failed_started_rx
            .recv()
            .expect("failure is invoked while the ownership is parked");
        assert!(
            failed_done_rx.try_recv().is_err(),
            "the DurabilityFailed commit is provably blocked on the runtime frontier held by the parked ownership commit"
        );
        assert!(
            !pending.drain().iter().any(|observation| matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )),
            "the health failure has not linearized while the ownership commit holds the frontier"
        );

        // Release the ownership commit: it completes its durable write and
        // record publication first, then the failure commit linearizes.
        {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || hook.proceed())
                .await
                .expect("the commit boundary was released");
        }
        let outcome = committer
            .await
            .expect("commit outcome")
            .expect("ownership wins the frontier");
        let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
            outcome
        else {
            panic!("accepted");
        };
        await_observation(&pending, |observation| {
            matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )
        })
        .await;
        failure_thread.join().expect("failure thread joins");

        // The runner-owned start boundary transitions the durable ownership
        // record from Starting to Running immediately before executor start.
        // Wait on that explicit watch rather than sampling a scheduler race.
        tokio::time::timeout(
            std::time::Duration::from_mins(2),
            started.wait_for(|is_started| *is_started),
        )
        .await
        .expect("runner start wait exceeded liveness guard")
        .expect("start channel stays open");

        // The owned execution exists with a durable ownership fact and
        // keeps its settlement authority.
        let snapshot = runtime
            .background_status(&execution_id)
            .expect("owned execution");
        assert_eq!(
            snapshot.state,
            crate::tools::background::BackgroundLifecycle::Running
        );
        let journal = store.read_events(None, 128).expect("events").events;
        assert!(
            journal.iter().any(|envelope| matches!(
                &envelope.event,
                crate::events::types::RuntimeEvent::BackgroundExecutionCommitted {
                    execution_id: committed_id,
                    ..
                } if *committed_id == execution_id
            )),
            "the ownership durable fact committed before the failure"
        );
        release.send_replace(true);
        runtime
            .tool_runtime()
            .background()
            .wait_until_settled(&execution_id)
            .await;
        let terminal = runtime
            .background_status(&execution_id)
            .expect("settled execution");
        assert_eq!(
            terminal.state,
            crate::tools::background::BackgroundLifecycle::Succeeded
        );
    }

    /// Issue #63 (Blocker 2, owning-runtime level): when the background
    /// settlement owner's bounded terminal-publication budget is exhausted,
    /// the owning runtime is placed into the explicit `DurabilityFailed`
    /// state through the narrow failure seam installed at construction —
    /// the unresolved candidate stays retained and observable in the
    /// registry, and the runtime never claims false healthy progress.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn exhausted_background_publication_degrades_the_owning_runtime() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_829c626d-2692-7096-81f2-912d7383be41",
            ))
            .expect("in-memory store"),
        );
        let (runtime, _model) = headless_runtime_over_store(
            &dir,
            "conv_829c626d-2692-7096-81f2-912d7383be41",
            store.clone(),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // Dispatch one background execution through the authoritative
        // registry of the runtime's tool runtime.
        let (executor, mut started, release) = GatedBackgroundExecutor::new();
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
        let invocation = crate::tools::types::ToolInvocation {
            id: crate::tools::types::ToolInvocationId::Agent {
                call_id: crate::runtime::identity::ToolCallId::new("call-1"),
            },
            tool_id: crate::runtime::identity::ToolId::new("tool-bash"),
            tool_name: "bash".to_owned(),
            mode: crate::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let prepared = runtime
            .tool_runtime()
            .background()
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
            runtime
                .tool_runtime()
                .background()
                .commit_dispatch(
                    prepared,
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .expect("commit")
        else {
            panic!("accepted");
        };
        tokio::time::timeout(
            std::time::Duration::from_mins(2),
            started.wait_for(|is_started| *is_started),
        )
        .await
        .expect("runner start wait exceeded liveness guard")
        .expect("start channel stays open");

        // Arm exactly the bounded publication budget (two acceptance
        // faults), then release the runner: both production publication
        // attempts fail, and the runner reports the exhausted budget
        // through the failure seam.
        store.arm_fail_accept_times(2);
        release.send_replace(true);
        let observations = await_observation(pending.as_ref(), |o| {
            matches!(o, ConversationObservation::DurabilityFailed { operation, .. }
                if operation == "background_terminal_publication")
        })
        .await;
        let failures = observations
            .iter()
            .filter(|o| matches!(o, ConversationObservation::DurabilityFailed { .. }))
            .count();
        assert_eq!(
            failures, 1,
            "the exhausted budget degraded the runtime exactly once — no hot loop"
        );

        // The unresolved terminal candidate stays retained and observable;
        // no false terminal publication exists; the runtime refuses new
        // durable work typed.
        let snapshot = runtime
            .background_status(&execution_id)
            .expect("execution record");
        assert_eq!(
            snapshot.state,
            crate::tools::background::BackgroundLifecycle::PublishingTerminal
        );
        assert!(
            snapshot.result.is_some(),
            "the unresolved terminal candidate remains retained and observable"
        );
        assert!(
            store.load_pending().expect("load pending").is_empty(),
            "no false durable terminal inbound committed"
        );
        assert!(matches!(
            runtime.submit_inbound(text_content("late")),
            Err(InboundAdmissionError::DurabilityFailed { .. })
        ));

        // M9c: shutdown does not wait forever for the intentionally retained
        // PublishingTerminal record. The owner has entered its explicit
        // durability-failure state, so shutdown returns a typed settlement
        // failure and the runtime remains Draining rather than claiming
        // false quiescence.
        let shutdown = runtime.shutdown().await;
        assert!(matches!(
            shutdown,
            Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                detail
            }) if detail.contains("terminal publication")
        ));
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Draining
        );
    }

    /// Every reader of the runtime's durability failure observes the **one
    /// authoritative absorbing fact** (Issue #60 single-source-of-truth):
    /// after the real subagent terminal-publication exhaustion commits
    /// `DurabilityFailed`, the gate's fact, the published observation, the
    /// coordinator's fail-closed rejections (`submit_inbound`, `model_set`),
    /// the subagent and background ownership refusals, and the shutdown
    /// diagnostic all report the same operation and diagnostic. There is no
    /// second failed-state storage anywhere.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn every_reader_of_the_durability_failure_observes_the_single_authoritative_fact() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_042447de-8629-7a98-887b-c9fa571c7cc9",
            ))
            .expect("in-memory store"),
        );
        let admission_gate = Arc::new(super::Gate::default());
        let (runtime, _model, subagents) = headless_runtime_over_store_with_subagents(
            &dir,
            "conv_042447de-8629-7a98-887b-c9fa571c7cc9",
            store.clone(),
            Some(admission_gate.clone()),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();
        admission_gate.arm();
        runtime
            .submit_inbound(text_content("hold admission worker"))
            .expect("hold inbound");
        within_liveness_guard(
            "subagent runtime admission worker gate",
            tokio::task::spawn_blocking({
                let admission_gate = admission_gate.clone();
                move || admission_gate.wait_entered()
            }),
        )
        .await
        .expect("admission gate task");

        // The real production failure path: one owned child exhausts its
        // bounded terminal-publication budget.
        let (staged, mut peer) = stage_runtime_test_child(&dir.path().join("owned-child"));
        subagents.push_staged_override(staged);
        let outcome = subagents
            .commit(
                subagents
                    .prepare(
                        &crate::runtime::subagent::SubagentStartSpec {
                            authority: crate::runtime::subagent::DurableAgentAuthority {
                                execution_policy:
                                    crate::runtime::subagent::InheritedExecutionPolicy::default(),
                                resolved: test_resolved_subagent("explore"),
                                approval_mode: crate::runtime::ApprovalMode::Policy,
                            },
                            admission: crate::runtime::subagent::ActivationAdmission {
                                task: "owned".to_owned(),
                                context: None,
                                origin:
                                    crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                                        tool_call_id: ToolCallId::new("call-owned"),
                                    },
                                terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                            },
                        },
                        &crate::runtime::cancellation::CancellationSignal::new(),
                    )
                    .await
                    .expect("prepare"),
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect("commit");
        assert!(matches!(
            outcome,
            crate::runtime::subagent::SubagentStartOutcome::Accepted(_)
        ));
        assert!(matches!(
            crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
                .await
                .expect("delegate frame"),
            Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
        ));
        store.arm_fail_accept_times(3);
        crate::runtime::subagent::ipc::write_child_frame(
            &mut peer,
            &crate::runtime::subagent::ipc::ChildFrame::Result(
                crate::runtime::subagent::ipc::ResultFrame {
                    status: crate::runtime::subagent::ipc::ChildResultStatus::Succeeded,
                    content: Some("answer".to_owned()),
                    diagnostic: None,
                },
            ),
        )
        .await
        .expect("result");
        let observations = await_observation(&pending, |observation| {
            matches!(
                observation,
                ConversationObservation::DurabilityFailed { operation, .. }
                    if operation == "subagent_terminal_publication"
            )
        })
        .await;

        // The one authoritative fact: stored in the runtime-owned gate, and
        // only there.
        let failure = runtime
            .inner
            .durability_gate
            .failure()
            .expect("the authoritative absorbing fact");
        assert_eq!(
            failure.operation,
            DurableOperation::SubagentTerminalPublication
        );
        assert!(!failure.diagnostic.is_empty());
        let observed = observations
            .iter()
            .find_map(|observation| match observation {
                ConversationObservation::DurabilityFailed {
                    operation,
                    diagnostic,
                } => Some((operation.as_str(), diagnostic.as_str())),
                _ => None,
            })
            .expect("the DurabilityFailed observation");
        assert_eq!(observed.0, failure.operation.as_str());
        assert_eq!(observed.1, failure.diagnostic.as_str());
        assert_eq!(
            runtime.inner.durability_failure_diagnostic().as_deref(),
            Some(failure.diagnostic.as_str())
        );

        // The coordinator's fail-closed rejections read the same fact.
        match runtime.submit_inbound(text_content("late")) {
            Err(InboundAdmissionError::DurabilityFailed { message }) => {
                assert_eq!(message, failure.diagnostic);
            }
            other => {
                panic!("submit_inbound must refuse with the authoritative diagnostic: {other:?}")
            }
        }
        match runtime.model_set(crate::model::session::SessionModelConfig::of(
            serde_json::from_value(serde_json::json!("local/model")).expect("model ref"),
        )) {
            Err(ModelUpdateError::DurabilityFailed { message }) => {
                assert_eq!(message, failure.diagnostic);
            }
            other => panic!("model_set must refuse with the authoritative diagnostic: {other:?}"),
        }

        // The detached ownership planes read the same fact.
        let (staged, _rejected_peer) = stage_runtime_test_child(&dir.path().join("rejected-child"));
        subagents.push_staged_override(staged);
        let prepared = subagents
            .prepare(
                &crate::runtime::subagent::SubagentStartSpec {
                    authority: crate::runtime::subagent::DurableAgentAuthority {
                        execution_policy:
                            crate::runtime::subagent::InheritedExecutionPolicy::default(),
                        resolved: test_resolved_subagent("explore"),
                        approval_mode: crate::runtime::ApprovalMode::Policy,
                    },
                    admission: crate::runtime::subagent::ActivationAdmission {
                        task: "rejected".to_owned(),
                        context: None,
                        origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                            tool_call_id: ToolCallId::new("call-rejected"),
                        },
                        terminal: crate::runtime::subagent::SubagentTerminalMode::Normal,
                    },
                },
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .expect("prepare");
        match subagents
            .commit(
                prepared,
                &crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
        {
            Err(crate::runtime::subagent::SubagentStartError::DurabilityFailed { detail }) => {
                assert_eq!(detail, failure.diagnostic);
            }
            other => panic!(
                "subagent ownership must refuse with the authoritative diagnostic: {other:?}"
            ),
        }
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> =
            Arc::new(GatedBackgroundExecutor::new().0);
        let invocation = crate::tools::types::ToolInvocation {
            id: crate::tools::types::ToolInvocationId::Agent {
                call_id: ToolCallId::new("call-rejected-bg"),
            },
            tool_id: ToolId::new("tool-bash"),
            tool_name: "bash".to_owned(),
            mode: crate::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let prepared = runtime
            .tool_runtime()
            .background()
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        match runtime.tool_runtime().background().commit_dispatch(
            prepared,
            &crate::runtime::cancellation::CancellationSignal::new(),
        ) {
            Err(crate::tools::background::BackgroundDispatchError::DurabilityFailed { detail }) => {
                assert_eq!(detail, failure.diagnostic);
            }
            other => panic!(
                "background ownership must refuse with the authoritative diagnostic: {other:?}"
            ),
        }

        admission_gate.release();
    }

    /// The absorbing winner is never replaced (Issue #60 single
    /// source-of-truth): the first committed durability failure stays the
    /// authoritative fact — a second failure with a *different* operation
    /// and diagnostic neither rewrites the stored operation/diagnostic nor
    /// publishes a second `DurabilityFailed` observation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_second_durability_failure_never_replaces_the_absorbing_first() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_ab84aa64-dcd1-7c9b-8c79-ca11323ae1a6",
            ))
            .expect("in-memory store"),
        );
        let (runtime, _model) = headless_runtime_over_store(
            &dir,
            "conv_ab84aa64-dcd1-7c9b-8c79-ca11323ae1a6",
            store.clone(),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // Failure A commits first through the single production commit seam.
        runtime.force_durability_failure_for_test(
            DurableOperation::EventJournal,
            "first absorbing failure",
        );
        let observations = await_observation(&pending, |observation| {
            matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )
        })
        .await;
        assert_eq!(
            observations
                .iter()
                .filter(|observation| matches!(
                    observation,
                    ConversationObservation::DurabilityFailed { .. }
                ))
                .count(),
            1,
            "the first commit publishes exactly one DurabilityFailed observation"
        );
        let failure = runtime
            .inner
            .durability_gate
            .failure()
            .expect("the authoritative absorbing fact");
        assert_eq!(failure.operation, DurableOperation::EventJournal);
        assert_eq!(failure.diagnostic, "first absorbing failure");

        // Failure B arrives later through the same seam, with a genuinely
        // different operation and diagnostic: the guard observes the
        // absorbing winner and neither rewrites the fact nor publishes a
        // second observation.
        runtime.force_durability_failure_for_test(
            DurableOperation::SubagentTerminalPublication,
            "second failure must not win",
        );
        let failure = runtime
            .inner
            .durability_gate
            .failure()
            .expect("the authoritative absorbing fact");
        assert_eq!(
            failure.operation,
            DurableOperation::EventJournal,
            "the first winner stays the absorbing operation"
        );
        assert_eq!(
            failure.diagnostic, "first absorbing failure",
            "the first winner stays the absorbing diagnostic"
        );
        assert!(
            !pending.drain().iter().any(|observation| matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )),
            "no second DurabilityFailed observation is published"
        );
        assert_eq!(
            runtime.inner.durability_failure_diagnostic().as_deref(),
            Some("first absorbing failure"),
            "the shutdown diagnostic reads the same absorbing fact"
        );
        // The single authoritative fact is still exactly one: there is no
        // second storage to disagree.
        assert_eq!(
            runtime.inner.durability_gate.failure().map(|f| f.operation),
            Some(DurableOperation::EventJournal)
        );
    }

    /// A foreground tool that starts, parks until cancellation is observable,
    /// and then reads the cancellation **cause from its execution context's
    /// authority** — exactly what a real cancellable executor does when it
    /// normalizes its own terminal status.
    struct CauseProbeTool {
        started: tokio::sync::watch::Sender<bool>,
        observed: Arc<std::sync::Mutex<Option<CancellationReason>>>,
    }

    impl CauseProbeTool {
        fn new() -> (
            Self,
            tokio::sync::watch::Receiver<bool>,
            Arc<std::sync::Mutex<Option<CancellationReason>>>,
        ) {
            let (started, started_rx) = tokio::sync::watch::channel(false);
            let observed = Arc::new(std::sync::Mutex::new(None));
            (
                Self {
                    started,
                    observed: observed.clone(),
                },
                started_rx,
                observed,
            )
        }
    }

    impl crate::tools::executor::ToolExecutor for CauseProbeTool {
        fn start<'a>(
            &'a self,
            _invocation: crate::tools::types::ToolInvocation,
            context: crate::tools::executor::ToolExecutionContext<'a>,
        ) -> crate::tools::executor::ToolExecutionHandle<'a> {
            let started = self.started.clone();
            let observed = self.observed.clone();
            let cancellation = context.cancellation.clone();
            crate::tools::executor::ToolExecutionHandle::settled_by_operation(
                Box::pin(async move {
                    // The context is built at tool start, before any cancellation
                    // exists: a start-time copy of the cause could only ever be
                    // the attempt's default.
                    assert!(!context.cancellation.is_cancelled());
                    started.send_replace(true);
                    context.cancellation.cancelled().await;
                    let reason = context.cancellation.reason();
                    *observed.lock().expect("observed cause lock") = Some(reason);
                    crate::tools::types::ToolExecutionResult {
                        status: crate::tools::types::ToolExecutionStatus::Cancelled {
                            reason,
                            phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
                        },
                        content: Vec::new(),
                        duration_ms: 0,
                        exit_code: None,
                        artifacts: Vec::new(),
                        truncation: None,
                        workflow: None,
                        managed_output: None,
                    }
                }),
                cancellation,
            )
        }

        fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
            crate::tools::deadline::ToolProgressCapability::None
        }
    }

    /// Builds the one-tool-call model script the parked foreground-probe
    /// regressions (cancellation cause, deadline, settlement) drive.
    fn cause_probe_registry_and_script(
        tool: Arc<dyn ToolExecutor>,
    ) -> (crate::tools::executor::ToolRegistry, Vec<FakeStep>) {
        use crate::tools::types::{
            ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin,
            ToolReplayPolicy,
        };
        let definition = ToolDefinition {
            id: crate::runtime::identity::ToolId::new("tool-cause-probe"),
            name: "cause_probe".to_owned(),
            description: "park until cancellation and report the winning cause".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        };
        let mut registry = crate::tools::executor::ToolRegistry::new();
        registry
            .register(definition.clone(), tool)
            .expect("cause probe registration");
        let call_id = ToolCallId::new("call-cause-probe");
        let script = vec![
            FakeStep::Emit(crate::model::event::ModelEvent::Started),
            FakeStep::Emit(crate::model::event::ModelEvent::ToolCallStarted {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCallStart {
                    id: call_id.clone(),
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                },
            }),
            FakeStep::Emit(crate::model::event::ModelEvent::ToolCallArgumentsDelta {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                call_id: call_id.clone(),
                arguments_delta: "{}".to_owned(),
            }),
            FakeStep::Emit(crate::model::event::ModelEvent::ToolCallCompleted {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCall {
                    id: call_id,
                    tool_id: definition.id,
                    name: definition.name,
                    arguments: serde_json::json!({}),
                },
            }),
            FakeStep::Emit(crate::model::event::ModelEvent::Completed {
                finish_reason: crate::model::finish::ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ];
        (registry, script)
    }

    /// M9c (Fix C): a foreground execution that started **before** any
    /// cancellation existed must observe the cause that actually won the
    /// race, not the attempt's start-time default.
    ///
    /// Happens-before: the executor asserts its context is not cancelled and
    /// only then publishes `started`; the test waits for `started` before
    /// calling `shutdown`, so runtime drain is provably the first
    /// cancellation of this attempt. The executor then reads the cause from
    /// the attempt authority through its context.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn foreground_executor_observes_the_winning_runtime_shutdown_cause() {
        let (tool, mut started, observed) = CauseProbeTool::new();
        let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, _model) =
            headless_runtime(&dir, vec![script, one_turn_script()], Some(registry), None).await;
        runtime.activate();
        runtime
            .submit_inbound(text_content("start the foreground tool"))
            .expect("accepted");
        within_liveness_guard(
            "the foreground tool to start before any cancellation",
            started.wait_for(|is_started| *is_started),
        )
        .await
        .expect("start channel stays open");

        within_liveness_guard("runtime shutdown", runtime.shutdown())
            .await
            .expect("drain reaches quiescence");
        assert_eq!(
            *observed.lock().expect("observed cause lock"),
            Some(CancellationReason::RuntimeShutdown),
            "the executor reads the winning cause from the attempt authority"
        );
        let store = runtime.tool_runtime().durable_store();
        assert!(
            store
                .read_events(None, 256)
                .expect("events")
                .events
                .iter()
                .any(|envelope| matches!(
                    &envelope.event,
                    crate::events::types::RuntimeEvent::AttemptCancelled {
                        reason: CancellationReason::RuntimeShutdown,
                        ..
                    }
                )),
            "the attempt terminal event agrees with the executor's observation"
        );
    }

    /// M9c (Fix C, first-winner): a user cancellation that won first stays
    /// the absorbing cause; a later runtime drain never relabels it, and the
    /// executor reads the same first winner.
    ///
    /// Happens-before: the executor publishes `started` before any
    /// cancellation exists; `cancel_current_attempt` then provably wins the
    /// first cancellation under the coordinator lock, and only afterwards
    /// does `shutdown` request `RuntimeShutdown` on the same handle.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn first_cancellation_cause_survives_a_later_runtime_drain() {
        let (tool, mut started, observed) = CauseProbeTool::new();
        let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, _model) =
            headless_runtime(&dir, vec![script, one_turn_script()], Some(registry), None).await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();
        runtime
            .submit_inbound(text_content("start the foreground tool"))
            .expect("accepted");
        within_liveness_guard(
            "the foreground tool to start before any cancellation",
            started.wait_for(|is_started| *is_started),
        )
        .await
        .expect("start channel stays open");

        let observations = await_observation(pending.as_ref(), |observation| {
            matches!(observation, ConversationObservation::AttemptAdmitted { .. })
        })
        .await;
        let attempt_id = observations
            .iter()
            .find_map(|observation| match observation {
                ConversationObservation::AttemptAdmitted { attempt_id, .. } => {
                    Some(attempt_id.clone())
                }
                _ => None,
            })
            .expect("the admitted attempt identity");
        runtime
            .cancel_current_attempt(&attempt_id)
            .expect("user cancellation wins first");

        within_liveness_guard("runtime shutdown", runtime.shutdown())
            .await
            .expect("drain reaches quiescence");
        assert_eq!(
            *observed.lock().expect("observed cause lock"),
            Some(CancellationReason::UserRequested),
            "a later runtime drain cannot relabel the first winning cause"
        );
        let store = runtime.tool_runtime().durable_store();
        assert!(
            store
                .read_events(None, 256)
                .expect("events")
                .events
                .iter()
                .any(|envelope| matches!(
                    &envelope.event,
                    crate::events::types::RuntimeEvent::AttemptCancelled {
                        reason: CancellationReason::UserRequested,
                        ..
                    }
                )),
            "the terminal event reports the first winner"
        );
    }

    /// A foreground executor whose physical settlement evidence arrives
    /// late: it signals `started`, observes the cancellation request,
    /// signals `cancel_observed`, then parks on a settle gate before
    /// returning its `Cancelled` result — exactly the Issue #204 case-M cut
    /// where runtime drain must keep awaiting the executor's real terminal
    /// evidence.
    struct SettleGateProbeTool {
        started: tokio::sync::watch::Sender<bool>,
        cancel_observed: tokio::sync::watch::Sender<bool>,
        settle_gate: tokio::sync::watch::Sender<bool>,
    }

    impl SettleGateProbeTool {
        fn new() -> (
            Self,
            tokio::sync::watch::Receiver<bool>,
            tokio::sync::watch::Receiver<bool>,
            tokio::sync::watch::Sender<bool>,
        ) {
            let (started, started_rx) = tokio::sync::watch::channel(false);
            let (cancel_observed, cancel_observed_rx) = tokio::sync::watch::channel(false);
            let (settle_gate, _) = tokio::sync::watch::channel(false);
            (
                Self {
                    started,
                    cancel_observed,
                    settle_gate: settle_gate.clone(),
                },
                started_rx,
                cancel_observed_rx,
                settle_gate,
            )
        }
    }

    impl ToolExecutor for SettleGateProbeTool {
        fn start<'a>(
            &'a self,
            _invocation: ToolInvocation,
            context: ToolExecutionContext<'a>,
        ) -> crate::tools::executor::ToolExecutionHandle<'a> {
            let started = self.started.clone();
            let cancel_observed = self.cancel_observed.clone();
            let settle_gate = self.settle_gate.clone();
            let cancellation = context.cancellation.clone();
            crate::tools::executor::ToolExecutionHandle::settled_by_operation(
                Box::pin(async move {
                    assert!(!context.cancellation.is_cancelled());
                    started.send_replace(true);
                    context.cancellation.cancelled().await;
                    cancel_observed.send_replace(true);
                    // The settlement gate models late physical settlement
                    // evidence: the outcome is already decided, but the
                    // settlement plane's drive of the operation does not
                    // resolve until the executor's settlement is real.
                    let mut released = settle_gate.subscribe();
                    released
                        .wait_for(|is_released| *is_released)
                        .await
                        .expect("settle gate stays open");
                    let reason = context.cancellation.reason();
                    crate::tools::types::ToolExecutionResult {
                        status: ToolExecutionStatus::Cancelled {
                            reason,
                            phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
                        },
                        content: Vec::new(),
                        duration_ms: 0,
                        exit_code: None,
                        artifacts: Vec::new(),
                        truncation: None,
                        workflow: None,
                        managed_output: None,
                    }
                }),
                cancellation,
            )
        }

        fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
            crate::tools::deadline::ToolProgressCapability::None
        }
    }

    /// Issue #204 (case L, runtime level): the policy frozen at runtime
    /// construction is the exact policy the admitted execution runs under.
    /// The runtime is composed with a 10s hard deadline on the manual
    /// monotonic clock; when the clock crosses that deadline, the loop fires
    /// cancellation intent for exactly this execution and the
    /// executor-proven `Cancelled` settles as the canonical `TimedOut` —
    /// both facts journaled through the real attempt-admission path.
    ///
    /// Happens-before: the executor publishes `started` only after its
    /// executor-start frontier — at which point the frozen liveness window
    /// is already running — and only then is the clock advanced, so the
    /// deadline winner is provably the construction-time hard deadline and
    /// no wall clock is involved. The settlement waiter is registered
    /// before the advance, so the winner cannot settle unobserved.
    #[allow(clippy::too_many_lines)] // one linear deterministic scenario
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn admitted_attempt_executes_under_the_frozen_tool_deadline_policy() {
        let (tool, mut started, _observed) = CauseProbeTool::new();
        let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_35fc42a3-4f04-7145-8c5b-1a37fc603ef5");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id,
                workspace: tool_runtime.workspace().clone(),
                agent_activation: {
                    let mut activation = fixture_activation(&tool_runtime);
                    activation.profile.tools.builtin = registry
                        .definitions()
                        .into_iter()
                        .filter(|tool| tool.origin.source().is_none())
                        .map(|tool| tool.name.clone())
                        .collect();
                    activation
                },
                base_tool_registry: Arc::new(registry),
                extension_tools: tool_runtime.extension_tool_plane(),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let model = Arc::new(FakeModel::new(vec![script, one_turn_script()]));
        let adapter: Arc<dyn ModelAdapter> = model.clone();
        let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
        let config = RuntimeConversationConfig {
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter),
            approval_mode: ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
                std::time::Duration::from_secs(10),
                None,
            ),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator: Arc::new(DefaultTokenEstimator),
                status_engine: Some(AgentStatusEngine::default()),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator,
            clock: None,
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        };
        let runtime = ConversationRuntime::with_test_monotonic_clock(
            config,
            clock.clone() as Arc<dyn crate::runtime::MonotonicClock>,
        )
        .expect("runtime composition");
        runtime.activate();
        runtime
            .submit_inbound(text_content("start the foreground tool"))
            .expect("accepted");
        within_liveness_guard(
            "the foreground tool to start at its executor-start frontier",
            started.wait_for(|is_started| *is_started),
        )
        .await
        .expect("start channel stays open");

        let settled = runtime.settlement_signal().notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        clock.advance(10_000);
        within_liveness_guard("the admitted attempt to settle", settled).await;
        within_liveness_guard("runtime shutdown", runtime.shutdown())
            .await
            .expect("drain reaches quiescence");

        let events = runtime
            .tool_runtime()
            .durable_store()
            .read_events(None, 256)
            .expect("events")
            .events;
        assert!(
            events.iter().any(|envelope| matches!(
                &envelope.event,
                RuntimeEvent::ToolExecutionDeadlineFired {
                    kind: crate::tools::deadline::ToolDeadlineKind::Hard,
                    ..
                }
            )),
            "the frozen construction-time hard deadline fired for the admitted execution"
        );
        assert!(
            events.iter().any(|envelope| matches!(
                &envelope.event,
                RuntimeEvent::ToolExecutionCancellationRequested {
                    cause: crate::tools::deadline::ToolCancellationCause::Deadline(
                        crate::tools::deadline::ToolDeadlineKind::Hard
                    ),
                    ..
                }
            )),
            "the deadline winner's physical cancellation request is journaled"
        );
        assert!(
            events.iter().any(|envelope| matches!(
                &envelope.event,
                RuntimeEvent::ToolExecutionSettlementObserved {
                    certainty: crate::tools::deadline::ToolSettlementCertainty::Confirmed,
                    ..
                }
            )),
            "the executor's proven settlement is journaled"
        );
        let completed = events
            .iter()
            .filter_map(|envelope| match &envelope.event {
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id,
                    result,
                    ..
                } if tool_call_id == &ToolCallId::new("call-cause-probe") => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(completed.len(), 1, "exactly one canonical tool result");
        assert!(
            matches!(completed[0].status, ToolExecutionStatus::TimedOut),
            "executor-proven cancellation after the deadline is the proven timeout: {:?}",
            completed[0].status
        );
    }

    /// Issue #204 (case M): cancellation intent is never settlement.
    /// Runtime drain cancels the admitted attempt and requests physical
    /// cancellation of its foreground execution, but `shutdown` cannot
    /// complete while the executor's settlement is still unresolved — the
    /// drain barrier awaits the executor's settlement authority driving the
    /// operation to its real terminal evidence (guarded against a
    /// contract-violating executor by the settlement control-plane guard,
    /// which this test never approaches on the wall clock) rather than
    /// abandoning the operation.
    ///
    /// This is the runtime quiescence invariant for local ownership: the
    /// settle gate models residual rustX-owned local physical cleanup still
    /// in flight (a kill/wait/reap or join ladder), and `shutdown()` must
    /// never report `Quiescent` while that local cleanup ownership is
    /// pending. Only when the settlement plane returns — cleanup completed —
    /// may drain cross its barrier.
    ///
    /// Happens-before: `cancel_observed` proves the drain's cancellation
    /// request already reached the executor and the executor is now parked
    /// on its settle gate, mid-settlement. The shutdown result channel must
    /// still be empty at that instant; only releasing the gate lets drain
    /// cross its settlement barrier and return.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_waits_for_executor_physical_settlement() {
        let (tool, mut started, mut cancel_observed, settle_release) = SettleGateProbeTool::new();
        let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, _model) =
            headless_runtime(&dir, vec![script, one_turn_script()], Some(registry), None).await;
        runtime.activate();
        runtime
            .submit_inbound(text_content("start the foreground tool"))
            .expect("accepted");
        within_liveness_guard(
            "the foreground tool to start before any cancellation",
            started.wait_for(|is_started| *is_started),
        )
        .await
        .expect("start channel stays open");

        let runtime_for_shutdown = runtime.clone();
        let (shutdown_sender, mut shutdown_receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = shutdown_sender.send(runtime_for_shutdown.shutdown().await);
        });
        within_liveness_guard(
            "the executor to observe the drain's cancellation request",
            cancel_observed.wait_for(|observed| *observed),
        )
        .await
        .expect("cancel-observation channel stays open");
        assert_eq!(
            shutdown_receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty),
            "drain cannot complete while the admitted execution's settlement is unresolved"
        );

        settle_release.send_replace(true);
        within_liveness_guard("runtime shutdown", shutdown_receiver)
            .await
            .expect("shutdown task stays alive")
            .expect("drain reaches quiescence");

        let events = runtime
            .tool_runtime()
            .durable_store()
            .read_events(None, 256)
            .expect("events")
            .events;
        assert!(
            events.iter().any(|envelope| matches!(
                &envelope.event,
                RuntimeEvent::AttemptCancelled {
                    reason: CancellationReason::RuntimeShutdown,
                    ..
                }
            )),
            "the attempt terminal cancellation event is journaled"
        );
        assert_eq!(
            events
                .iter()
                .filter(|envelope| matches!(
                    &envelope.event,
                    RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
                        if tool_call_id == &ToolCallId::new("call-cause-probe")
                ))
                .count(),
            1,
            "exactly one terminal result is committed for the call"
        );
    }

    /// A foreground executor with a genuinely split handle (Issue #204
    /// drain cut, confirmed path): the physical completion plane parks
    /// forever, so only the independent cancellation/settlement control
    /// plane can report. The settlement plane observes the cancellation
    /// request, signals `cancel_observed`, then parks on the settle gate
    /// before reporting `Confirmed` with its proven cancellation result —
    /// the case where runtime drain must keep awaiting the settlement
    /// authority even though the completion plane can never return.
    struct DetachedConfirmGateTool {
        started: tokio::sync::watch::Sender<bool>,
        cancel_observed: tokio::sync::watch::Sender<bool>,
        settle_gate: tokio::sync::watch::Sender<bool>,
    }

    impl DetachedConfirmGateTool {
        fn new() -> (
            Self,
            tokio::sync::watch::Receiver<bool>,
            tokio::sync::watch::Receiver<bool>,
            tokio::sync::watch::Sender<bool>,
        ) {
            let (started, started_rx) = tokio::sync::watch::channel(false);
            let (cancel_observed, cancel_observed_rx) = tokio::sync::watch::channel(false);
            let (settle_gate, _) = tokio::sync::watch::channel(false);
            (
                Self {
                    started,
                    cancel_observed,
                    settle_gate: settle_gate.clone(),
                },
                started_rx,
                cancel_observed_rx,
                settle_gate,
            )
        }
    }

    impl ToolExecutor for DetachedConfirmGateTool {
        fn start<'a>(
            &'a self,
            _invocation: ToolInvocation,
            context: ToolExecutionContext<'a>,
        ) -> crate::tools::executor::ToolExecutionHandle<'a> {
            self.started.send_replace(true);
            // The physical completion plane can never resolve: this
            // execution settles through its settlement authority or not at
            // all.
            let completion: BoxFuture<'a, crate::tools::types::ToolExecutionResult> =
                Box::pin(std::future::pending());
            let cancel_observed = self.cancel_observed.clone();
            let settle_gate = self.settle_gate.clone();
            let settlement: BoxFuture<'a, crate::tools::executor::ToolSettlement> =
                Box::pin(async move {
                    context.cancellation.cancelled().await;
                    cancel_observed.send_replace(true);
                    // The executor's settlement evidence arrives only when
                    // its physical cancellation path has genuinely run to
                    // its end.
                    let mut released = settle_gate.subscribe();
                    released
                        .wait_for(|is_released| *is_released)
                        .await
                        .expect("settle gate stays open");
                    let reason = context.cancellation.reason();
                    crate::tools::executor::ToolSettlement::Confirmed(
                        crate::tools::types::ToolExecutionResult {
                            status: ToolExecutionStatus::Cancelled {
                                reason,
                                phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
                            },
                            content: Vec::new(),
                            duration_ms: 0,
                            exit_code: None,
                            artifacts: Vec::new(),
                            truncation: None,
                            workflow: None,
                            managed_output: None,
                        },
                    )
                });
            crate::tools::executor::ToolExecutionHandle::new(completion, settlement)
        }

        fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
            crate::tools::deadline::ToolProgressCapability::None
        }
    }

    /// A foreground executor modelling the one VALID `Unconfirmed` case
    /// (Issue #204 drain cut): a remote external effect with fully
    /// reclaimed local ownership. `start` returns a completion plane that
    /// parks forever; its settlement plane reports `Unconfirmed` promptly
    /// once it observes the cancellation request. The fixture spawns no
    /// task, thread, or process: when the settlement plane returns, every
    /// rustX-owned local execution ownership of the call has already
    /// settled, and only the remote external effect beyond rustX's
    /// ownership domain remains uncertain — so runtime drain may complete.
    struct DetachedUnconfirmedTool {
        started: tokio::sync::watch::Sender<bool>,
        cancel_observed: tokio::sync::watch::Sender<bool>,
    }

    impl DetachedUnconfirmedTool {
        fn new() -> (
            Self,
            tokio::sync::watch::Receiver<bool>,
            tokio::sync::watch::Receiver<bool>,
        ) {
            let (started, started_rx) = tokio::sync::watch::channel(false);
            let (cancel_observed, cancel_observed_rx) = tokio::sync::watch::channel(false);
            (
                Self {
                    started,
                    cancel_observed,
                },
                started_rx,
                cancel_observed_rx,
            )
        }
    }

    impl ToolExecutor for DetachedUnconfirmedTool {
        fn start<'a>(
            &'a self,
            _invocation: ToolInvocation,
            context: ToolExecutionContext<'a>,
        ) -> crate::tools::executor::ToolExecutionHandle<'a> {
            self.started.send_replace(true);
            // No local task or process exists outside the handle's futures:
            // the entire local ownership of this execution is the parked
            // completion shell and the settlement plane below.
            let completion: BoxFuture<'a, crate::tools::types::ToolExecutionResult> =
                Box::pin(std::future::pending());
            let cancel_observed = self.cancel_observed.clone();
            let settlement: BoxFuture<'a, crate::tools::executor::ToolSettlement> =
                Box::pin(async move {
                    context.cancellation.cancelled().await;
                    cancel_observed.send_replace(true);
                    // Local rustX execution ownership fully settled; only
                    // the remote effect's terminality is unprovable.
                    crate::tools::executor::ToolSettlement::Unconfirmed {
                        detail: "the dispatched operation crossed the external-effect frontier; \
                                 remote terminality cannot be proven"
                            .to_owned(),
                    }
                });
            crate::tools::executor::ToolExecutionHandle::new(completion, settlement)
        }

        fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
            crate::tools::deadline::ToolProgressCapability::None
        }
    }

    /// The execution-fact sequence of one call within one event list, in
    /// journal order — the drain-cut counterpart of the scripted suite's
    /// fact-sequence audit.
    fn drain_call_facts(events: &[RuntimeEvent], call_id: &ToolCallId) -> Vec<&'static str> {
        events
            .iter()
            .filter_map(|event| match event {
                RuntimeEvent::ToolExecutionStarted { tool_call_id, .. }
                    if tool_call_id == call_id =>
                {
                    Some("started")
                }
                RuntimeEvent::ToolExecutionProgress { tool_call_id, .. }
                    if tool_call_id == call_id =>
                {
                    Some("progress")
                }
                RuntimeEvent::ToolExecutionDeadlineFired { tool_call_id, .. }
                    if tool_call_id == call_id =>
                {
                    Some("deadline")
                }
                RuntimeEvent::ToolExecutionCancellationRequested { tool_call_id, .. }
                    if tool_call_id == call_id =>
                {
                    Some("cancellation-requested")
                }
                RuntimeEvent::ToolExecutionSettlementObserved { tool_call_id, .. }
                    if tool_call_id == call_id =>
                {
                    Some("settlement")
                }
                RuntimeEvent::ToolExecutionSettlementControlFailed { tool_call_id, .. }
                    if tool_call_id == call_id =>
                {
                    Some("settlement-control-failed")
                }
                RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
                    if tool_call_id == call_id =>
                {
                    Some("completed")
                }
                _ => None,
            })
            .collect()
    }

    /// Issue #204 (drain, confirmed split settlement): runtime drain awaits
    /// the executor's settlement AUTHORITY, not the physical completion
    /// plane. The fixture's completion plane parks forever; its settlement
    /// plane reports `Confirmed` only after a test gate releases. While the
    /// gate is closed the shutdown future must still be pending; releasing
    /// it lets drain cross the settlement barrier and reach quiescence,
    /// with the canonical `Cancelled { RuntimeShutdown }` committed exactly
    /// once.
    ///
    /// Happens-before: `cancel_observed` proves the drain's cancellation
    /// request already reached the executor and the settlement plane is
    /// parked on its gate, mid-settlement — the shutdown result channel is
    /// provably empty at that instant.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_waits_for_confirmed_settlement_then_drains() {
        let (tool, mut started, mut cancel_observed, settle_release) =
            DetachedConfirmGateTool::new();
        let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, _model) =
            headless_runtime(&dir, vec![script, one_turn_script()], Some(registry), None).await;
        runtime.activate();
        runtime
            .submit_inbound(text_content("start the foreground tool"))
            .expect("accepted");
        within_liveness_guard(
            "the foreground tool to start before any cancellation",
            started.wait_for(|is_started| *is_started),
        )
        .await
        .expect("start channel stays open");

        let runtime_for_shutdown = runtime.clone();
        let (shutdown_sender, mut shutdown_receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = shutdown_sender.send(runtime_for_shutdown.shutdown().await);
        });
        within_liveness_guard(
            "the settlement plane to observe the drain's cancellation request",
            cancel_observed.wait_for(|observed| *observed),
        )
        .await
        .expect("cancel-observation channel stays open");
        assert_eq!(
            shutdown_receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty),
            "drain cannot complete while the settlement authority is still gated"
        );

        settle_release.send_replace(true);
        within_liveness_guard("runtime shutdown", shutdown_receiver)
            .await
            .expect("shutdown task stays alive")
            .expect("drain reaches quiescence");

        let events = runtime
            .tool_runtime()
            .durable_store()
            .read_events(None, 256)
            .expect("events")
            .events;
        assert!(
            events.iter().any(|envelope| matches!(
                &envelope.event,
                RuntimeEvent::AttemptCancelled {
                    reason: CancellationReason::RuntimeShutdown,
                    ..
                }
            )),
            "the attempt terminal cancellation event is journaled"
        );
        assert!(
            events.iter().any(|envelope| matches!(
                &envelope.event,
                RuntimeEvent::ToolExecutionSettlementObserved {
                    certainty: crate::tools::deadline::ToolSettlementCertainty::Confirmed,
                    ..
                }
            )),
            "the executor's confirmed settlement evidence is journaled"
        );
        let completed = events
            .iter()
            .filter_map(|envelope| match &envelope.event {
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id,
                    result,
                    ..
                } if tool_call_id == &ToolCallId::new("call-cause-probe") => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            completed.len(),
            1,
            "exactly one terminal result is committed for the call"
        );
        assert!(
            matches!(
                completed[0].status,
                ToolExecutionStatus::Cancelled {
                    reason: CancellationReason::RuntimeShutdown,
                    phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
                }
            ),
            "the attempt winner owns the canonical reason of the proven cancellation: {:?}",
            completed[0].status
        );
    }

    /// Issue #204 (drain, unconfirmed split settlement): drain waits for
    /// the attempt's canonical settlement — which awaits the executor's
    /// settlement authority — and an `Unconfirmed` settlement resolves that
    /// wait. The canonical `OutcomeUnknown` is committed and `shutdown`
    /// RETURNS with quiescence, which is honest exactly because the
    /// executor's `Unconfirmed` carried the local-ownership invariant: all
    /// rustX-owned local execution ownership of the call already settled
    /// (the fixture spawns no task or process), and only the remote
    /// external effect beyond rustX's ownership domain remains uncertain.
    /// External uncertainty never blocks local quiescence; local rustX
    /// execution would (see the settle-gate cuts above).
    ///
    /// Happens-before: `cancel_observed` proves the drain's cancellation
    /// request reached the executor; the settlement plane then reports
    /// `Unconfirmed` without any further test input, so drain returning
    /// proves it required no physical completion. The terminal canonical
    /// state is absorbing: the journal never gains another fact for the
    /// call.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // one linear deterministic scenario
    async fn shutdown_drains_past_unconfirmed_settlement() {
        let (tool, mut started, mut cancel_observed) = DetachedUnconfirmedTool::new();
        let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, _model) =
            headless_runtime(&dir, vec![script, one_turn_script()], Some(registry), None).await;
        runtime.activate();
        runtime
            .submit_inbound(text_content("start the foreground tool"))
            .expect("accepted");
        within_liveness_guard(
            "the foreground tool to start before any cancellation",
            started.wait_for(|is_started| *is_started),
        )
        .await
        .expect("start channel stays open");

        let runtime_for_shutdown = runtime.clone();
        let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = shutdown_sender.send(runtime_for_shutdown.shutdown().await);
        });
        within_liveness_guard(
            "the executor to observe the drain's cancellation request",
            cancel_observed.wait_for(|observed| *observed),
        )
        .await
        .expect("cancel-observation channel stays open");
        within_liveness_guard("runtime shutdown", shutdown_receiver)
            .await
            .expect("shutdown task stays alive")
            .expect("drain reaches quiescence once local execution ownership settled");

        let call_id = ToolCallId::new("call-cause-probe");
        let store = runtime.tool_runtime().durable_store();
        let events: Vec<RuntimeEvent> = store
            .read_events(None, 256)
            .expect("events")
            .events
            .into_iter()
            .map(|envelope| envelope.event)
            .collect();
        assert!(
            events.iter().any(|event| matches!(
                event,
                RuntimeEvent::AttemptCancelled {
                    reason: CancellationReason::RuntimeShutdown,
                    ..
                }
            )),
            "the attempt terminal cancellation event is journaled"
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                RuntimeEvent::ToolExecutionCancellationRequested {
                    cause: crate::tools::deadline::ToolCancellationCause::Attempt(
                        CancellationReason::RuntimeShutdown
                    ),
                    ..
                }
            )),
            "the drain's physical cancellation request is journaled"
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                RuntimeEvent::ToolExecutionSettlementObserved {
                    certainty: crate::tools::deadline::ToolSettlementCertainty::Unconfirmed,
                    ..
                }
            )),
            "the executor's unconfirmed settlement evidence is journaled"
        );
        assert!(
            !events.iter().any(|event| matches!(
                event,
                RuntimeEvent::ToolExecutionSettlementControlFailed { .. }
            )),
            "the executor's settlement authority returned, so no control-plane failure is journaled"
        );
        let completed = events
            .iter()
            .filter_map(|event| match event {
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id,
                    result,
                    ..
                } if tool_call_id == &call_id => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            completed.len(),
            1,
            "exactly one terminal result is committed for the call"
        );
        let ToolExecutionStatus::OutcomeUnknown { detail } = &completed[0].status else {
            panic!(
                "unconfirmed post-frontier settlement is OutcomeUnknown, never a confirmed \
                 cancellation: {:?}",
                completed[0].status
            );
        };
        assert!(
            detail.contains("external-effect frontier"),
            "the executor's own unconfirmed frontier evidence: {detail}"
        );
        let facts_before_cleanup = drain_call_facts(&events, &call_id);

        // The executor left no rustX-owned local execution behind: the
        // settled terminal state is absorbing and the journal never gains
        // another fact for the call.
        let later: Vec<RuntimeEvent> = store
            .read_events(None, 256)
            .expect("events")
            .events
            .into_iter()
            .map(|envelope| envelope.event)
            .collect();
        assert_eq!(
            drain_call_facts(&later, &call_id),
            facts_before_cleanup,
            "the committed terminal state is absorbing"
        );
    }

    /// A foreground executor that violates the settlement contract at the
    /// runtime level (Issue #204 drain cut, control-plane failure): it
    /// signals `started`, records the drain's cancellation request through
    /// `cancel_observed`, and then never settles — its operation parks
    /// forever, so the settlement plane driving it never returns. The
    /// operation future holds a drop guard flipping `operation_dropped`:
    /// the fixture keeps all local ownership inside the handle's futures
    /// (the `settled_by_operation` contract), so when the settlement
    /// control-plane guard expires and the lifecycle drops the handle, the
    /// drop provably consumes the operation — the bounded ownership rule of
    /// the guard path.
    struct GuardViolationTool {
        started: tokio::sync::watch::Sender<bool>,
        cancel_observed: tokio::sync::watch::Sender<bool>,
        operation_dropped: Arc<std::sync::atomic::AtomicBool>,
    }

    impl GuardViolationTool {
        fn new() -> (
            Self,
            tokio::sync::watch::Receiver<bool>,
            tokio::sync::watch::Receiver<bool>,
            Arc<std::sync::atomic::AtomicBool>,
        ) {
            let (started, started_rx) = tokio::sync::watch::channel(false);
            let (cancel_observed, cancel_observed_rx) = tokio::sync::watch::channel(false);
            let operation_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
            (
                Self {
                    started,
                    cancel_observed,
                    operation_dropped: operation_dropped.clone(),
                },
                started_rx,
                cancel_observed_rx,
                operation_dropped,
            )
        }
    }

    impl ToolExecutor for GuardViolationTool {
        fn start<'a>(
            &'a self,
            _invocation: ToolInvocation,
            context: ToolExecutionContext<'a>,
        ) -> crate::tools::executor::ToolExecutionHandle<'a> {
            struct OperationDropGuard(Arc<std::sync::atomic::AtomicBool>);
            impl Drop for OperationDropGuard {
                fn drop(&mut self) {
                    self.0.store(true, std::sync::atomic::Ordering::Release);
                }
            }
            let started = self.started.clone();
            let cancel_observed = self.cancel_observed.clone();
            let drop_guard = OperationDropGuard(self.operation_dropped.clone());
            let cancellation = context.cancellation.clone();
            crate::tools::executor::ToolExecutionHandle::settled_by_operation(
                Box::pin(async move {
                    // The operation future owns this guard: dropping the
                    // operation drops the guard.
                    let _drop_guard = drop_guard;
                    started.send_replace(true);
                    context.cancellation.cancelled().await;
                    cancel_observed.send_replace(true);
                    // The contract violation under test: the cancellation
                    // request was observed, yet the operation — and with it
                    // the settlement plane — never settles.
                    std::future::pending::<()>().await;
                    unreachable!("the contract-violating execution never settles")
                }),
                cancellation,
            )
        }

        fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
            crate::tools::deadline::ToolProgressCapability::None
        }
    }

    /// Issue #204 (drain, settlement control-plane failure): a
    /// contract-violating executor whose settlement authority never returns
    /// cannot block runtime drain forever, and drain never lies about local
    /// ownership. The hard deadline fires on the manual clock, the
    /// cancellation request provably reaches the executor, the settlement
    /// control-plane guard expires, and the lifecycle commits one canonical
    /// `OutcomeUnknown` — journaled with the typed
    /// `ToolExecutionSettlementControlFailed` fact and NO
    /// `ToolExecutionSettlementObserved`, because no executor settlement
    /// evidence was ever observed. Before shutdown may report quiescence,
    /// the executor's remaining rustX-owned local ownership is consumed by
    /// dropping the handle (proven by the operation's drop guard), so
    /// `Quiescent` is honest: no rustX-owned local execution survives, and
    /// only the external effect frontier — here unprovable by construction —
    /// remains unknown.
    ///
    /// Happens-before: `cancel_observed` proves the cancellation request
    /// reached the executor before the guard window is exhausted; the
    /// settlement waiter is registered before the clock advances, so the
    /// committed `OutcomeUnknown` cannot settle unobserved.
    #[allow(clippy::too_many_lines)] // one linear deterministic scenario
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_drains_after_settlement_control_plane_failure() {
        let (tool, mut started, mut cancel_observed, operation_dropped) = GuardViolationTool::new();
        let (registry, script) = cause_probe_registry_and_script(Arc::new(tool));
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_b6f1eb3d-33f7-7829-864f-4b9d1335716d");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id,
                workspace: tool_runtime.workspace().clone(),
                agent_activation: {
                    let mut activation = fixture_activation(&tool_runtime);
                    activation.profile.tools.builtin = registry
                        .definitions()
                        .into_iter()
                        .filter(|tool| tool.origin.source().is_none())
                        .map(|tool| tool.name.clone())
                        .collect();
                    activation
                },
                base_tool_registry: Arc::new(registry),
                extension_tools: tool_runtime.extension_tool_plane(),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let model = Arc::new(FakeModel::new(vec![script, one_turn_script()]));
        let adapter: Arc<dyn ModelAdapter> = model.clone();
        let clock = Arc::new(crate::runtime::ManualMonotonicClock::new());
        let config = RuntimeConversationConfig {
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter),
            approval_mode: ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
                std::time::Duration::from_secs(10),
                None,
            ),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator: Arc::new(DefaultTokenEstimator),
                status_engine: Some(AgentStatusEngine::default()),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator,
            clock: None,
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        };
        let runtime = ConversationRuntime::with_test_monotonic_clock(
            config,
            clock.clone() as Arc<dyn crate::runtime::MonotonicClock>,
        )
        .expect("runtime composition");
        runtime.activate();
        runtime
            .submit_inbound(text_content("start the foreground tool"))
            .expect("accepted");
        within_liveness_guard(
            "the foreground tool to start at its executor-start frontier",
            started.wait_for(|is_started| *is_started),
        )
        .await
        .expect("start channel stays open");

        let settled = runtime.settlement_signal().notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        // t=10_000: the hard deadline fires cancellation intent.
        clock.advance(10_000);
        within_liveness_guard(
            "the executor to observe the cancellation request",
            cancel_observed.wait_for(|observed| *observed),
        )
        .await
        .expect("cancel-observation channel stays open");
        // t=40_000: the settlement control-plane guard (30s from the
        // intent) expires — the executor's settlement authority never
        // returned, a settlement-contract violation with no evidence.
        clock.advance(30_000);
        within_liveness_guard("the admitted attempt to settle", settled).await;

        // The bounded ownership rule of the guard path is real: dropping
        // the handle consumed the executor's remaining local ownership, so
        // drain's quiescence below is honest.
        assert!(
            operation_dropped.load(std::sync::atomic::Ordering::Acquire),
            "the guard path dropped the handle, consuming the executor's remaining local ownership"
        );
        within_liveness_guard("runtime shutdown", runtime.shutdown())
            .await
            .expect("drain reaches quiescence after the control-plane failure");

        let events = runtime
            .tool_runtime()
            .durable_store()
            .read_events(None, 256)
            .expect("events")
            .events;
        assert!(
            events.iter().any(|envelope| matches!(
                &envelope.event,
                RuntimeEvent::ToolExecutionDeadlineFired {
                    kind: crate::tools::deadline::ToolDeadlineKind::Hard,
                    ..
                }
            )),
            "the hard deadline intent is journaled"
        );
        let control_failures: Vec<&str> = events
            .iter()
            .filter_map(|envelope| match &envelope.event {
                RuntimeEvent::ToolExecutionSettlementControlFailed {
                    tool_call_id,
                    reason,
                    ..
                } if tool_call_id == &ToolCallId::new("call-cause-probe") => Some(reason.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            control_failures.len(),
            1,
            "exactly one typed settlement control-plane failure fact"
        );
        assert!(
            control_failures[0].contains("settlement control plane did not return"),
            "the lifecycle's contract-violation record: {}",
            control_failures[0]
        );
        assert!(
            !events.iter().any(|envelope| matches!(
                &envelope.event,
                RuntimeEvent::ToolExecutionSettlementObserved { tool_call_id, .. }
                    if tool_call_id == &ToolCallId::new("call-cause-probe")
            )),
            "no settlement was ever observed: the executor's authority never returned"
        );
        let completed = events
            .iter()
            .filter_map(|envelope| match &envelope.event {
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id,
                    result,
                    ..
                } if tool_call_id == &ToolCallId::new("call-cause-probe") => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(completed.len(), 1, "exactly one canonical tool result");
        assert!(
            matches!(
                completed[0].status,
                ToolExecutionStatus::OutcomeUnknown { .. }
            ),
            "a settlement control-plane failure is OutcomeUnknown, never TimedOut: {:?}",
            completed[0].status
        );
    }

    /// M9c (Blocker A): a recorded durability failure is an error **fact**,
    /// never permission to stop supervising a sibling owner. One background
    /// execution exhausts its bounded terminal-publication budget while a
    /// provider turn is still parked; drain must keep supervising the live
    /// provider and may return its aggregated settlement failure only after
    /// the Agent Loop has crossed its explicit settlement barrier.
    ///
    /// Happens-before: the model-arbitration pause holds the started provider
    /// stream immediately before the next provider/cancellation arbitration.
    /// `drain_supervision` then fires only from inside the drain task, so
    /// observing it proves drain reached supervision *with the durability
    /// failure already recorded* instead of short-circuiting; the
    /// current-attempt slot is still occupied at that instant, and the
    /// shutdown result channel is still empty. Releasing the exact
    /// arbitration barrier lets the already-requested cancellation settle the
    /// attempt, and only then does shutdown return.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn durability_failure_never_abandons_a_live_provider_turn() {
        use crate::agent::execution::test_sync::ModelArbitrationPause;

        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_d2d949a2-cff1-7341-8c06-4df386858aaa",
            ))
            .expect("in-memory store"),
        );
        let (_release_tx, release_rx) = crate::scripted_suites::support::fake::model_release();
        let script = vec![
            FakeStep::Emit(crate::model::event::ModelEvent::Started),
            FakeStep::ParkUntilReleased(release_rx),
            FakeStep::Emit(crate::model::event::ModelEvent::Failed {
                error: crate::model::error::ModelError {
                    kind: crate::model::error::ModelErrorKind::Cancelled,
                    message: "provider settled cancellation".to_owned(),
                    retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                    retry_after_ms: None,
                    provider_code: None,
                    context_overflow: None,
                    malformed_tool_proposal: None,
                    timeout_phase: None,
                    generation: None,
                },
            }),
        ];
        let (model_pause, mut model_pause_reached, model_pause_release) =
            ModelArbitrationPause::install(1);
        let drain_supervision = Arc::new(tokio::sync::Notify::new());
        let (runtime, _model) = headless_runtime_over_store_with_policy(
            &dir,
            "conv_d2d949a2-cff1-7341-8c06-4df386858aaa",
            store.clone(),
            vec![script],
            Some(CoordinatorProbe {
                drain_supervision: Some(drain_supervision.clone()),
                model_arbitration_pause: Some(model_pause),
                ..CoordinatorProbe::default()
            }),
            crate::model::ModelTimeoutPolicy::new(
                std::time::Duration::from_mins(5),
                std::time::Duration::from_mins(5),
            ),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();
        runtime
            .submit_inbound(text_content("park the provider"))
            .expect("accepted");
        // The provider has emitted Started and the Agent Loop now owns its
        // open stream, but the next provider/cancellation arbitration is
        // held behind an exact test barrier. Shutdown can therefore request
        // cancellation without racing the attempt to settle before drain
        // begins supervision.
        model_pause_reached
            .wait_for(|is_reached| *is_reached)
            .await
            .expect("model arbitration pause channel stays open");

        // A *different* owner records the durability failure: one background
        // execution spends its whole bounded terminal-publication budget.
        let (executor, mut started, release_background) = GatedBackgroundExecutor::new();
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
        let execution_id = commit_background(&runtime, &executor, "call-degrade");
        started
            .wait_for(|is_started| *is_started)
            .await
            .expect("start channel stays open");
        store.arm_fail_accept_times(2);
        release_background.send_replace(true);
        await_observation(pending.as_ref(), |observation| {
            matches!(observation, ConversationObservation::DurabilityFailed { operation, .. }
                if operation == "background_terminal_publication")
        })
        .await;
        assert_eq!(
            runtime
                .background_status(&execution_id)
                .expect("record")
                .state,
            crate::tools::background::BackgroundLifecycle::PublishingTerminal
        );

        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let shutdown_runtime = runtime.clone();
        tokio::spawn(async move {
            let _ = done_tx.send(shutdown_runtime.shutdown().await);
        });
        // Drain reached supervision *despite* the recorded durability
        // failure. The old fail-fast drain returned before this point.
        within_liveness_guard(
            "drain to park on the live provider turn",
            drain_supervision.notified(),
        )
        .await;
        assert!(
            runtime.has_current_attempt(),
            "the provider turn is still runtime-owned when drain begins supervising"
        );
        assert!(
            matches!(
                done_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "a sibling's durability failure must not end supervision of a live provider turn"
        );
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Draining
        );

        // Release the exact post-Started arbitration. The already-requested
        // cancellation then wins the next Agent Loop arbitration while the
        // fake provider stream is still pending, and the attempt settles.
        let _ = model_pause_release.send(());
        let shutdown = done_rx.await.expect("shutdown result channel");
        assert!(
            matches!(
                &shutdown,
                Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                    detail
                }) if detail.contains("terminal publication")
            ),
            "the unresolved terminal publication is honest settlement evidence: {shutdown:?}"
        );
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Draining,
            "unproven terminality never publishes Quiescent"
        );
        assert!(
            !runtime.has_current_attempt(),
            "the supervised provider turn settled before shutdown returned"
        );
        assert!(
            store
                .read_events(None, 256)
                .expect("events")
                .events
                .iter()
                .any(|envelope| matches!(
                    &envelope.event,
                    crate::events::types::RuntimeEvent::AttemptCancelled {
                        reason: CancellationReason::RuntimeShutdown,
                        ..
                    }
                )),
            "the supervised attempt reached its terminal cancellation"
        );
    }

    /// M9c (Blocker A / 4.1): one background record's failed terminal
    /// publication must not release the supervisor from a *sibling*
    /// background execution that is still physically running.
    ///
    /// Happens-before: the failing record's `DurabilityFailed` observation is
    /// awaited first, so the failure is provably recorded before shutdown
    /// starts. `drain_supervision` then proves drain entered supervision with
    /// that failure already known while the sibling is still active, and the
    /// sibling's own explicit release is the only thing that lets shutdown
    /// return.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn durability_failure_never_abandons_a_sibling_background_execution() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_d498b02e-ce4d-7e66-8b25-a4466a0e3a8b",
            ))
            .expect("in-memory store"),
        );
        let drain_supervision = Arc::new(tokio::sync::Notify::new());
        let (runtime, _model) = headless_runtime_over_store_with(
            &dir,
            "conv_d498b02e-ce4d-7e66-8b25-a4466a0e3a8b",
            store.clone(),
            vec![one_turn_script()],
            Some(CoordinatorProbe {
                drain_supervision: Some(drain_supervision.clone()),
                ..CoordinatorProbe::default()
            }),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        let (failing, mut failing_started, release_failing) = GatedBackgroundExecutor::new();
        let failing: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(failing);
        let failing_id = commit_background(&runtime, &failing, "call-failing");
        let (sibling, mut sibling_started, release_sibling) = GatedBackgroundExecutor::new();
        let sibling: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(sibling);
        let sibling_id = commit_background(&runtime, &sibling, "call-sibling");
        failing_started
            .wait_for(|is_started| *is_started)
            .await
            .expect("start channel stays open");
        sibling_started
            .wait_for(|is_started| *is_started)
            .await
            .expect("start channel stays open");

        // Exactly the failing record's bounded publication budget.
        store.arm_fail_accept_times(2);
        release_failing.send_replace(true);
        await_observation(pending.as_ref(), |observation| {
            matches!(observation, ConversationObservation::DurabilityFailed { operation, .. }
                if operation == "background_terminal_publication")
        })
        .await;

        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let shutdown_runtime = runtime.clone();
        tokio::spawn(async move {
            let _ = done_tx.send(shutdown_runtime.shutdown().await);
        });
        within_liveness_guard(
            "drain to park on the sibling background execution",
            drain_supervision.notified(),
        )
        .await;
        assert_eq!(
            runtime
                .background_status(&sibling_id)
                .expect("sibling record")
                .state,
            crate::tools::background::BackgroundLifecycle::Cancelling,
            "the sibling received drain cancellation and is still owned"
        );
        assert!(
            matches!(
                done_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "the failed record must not release the supervisor from the live sibling"
        );

        release_sibling.send_replace(true);
        let shutdown = done_rx.await.expect("shutdown result channel");
        assert!(
            matches!(
                &shutdown,
                Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                    detail
                }) if detail.contains("terminal publication")
            ),
            "the failed record is still reported: {shutdown:?}"
        );
        // The sibling was supervised to its own terminal boundary and its
        // terminal publication went through the one Pending Inbound path.
        assert!(
            runtime
                .background_status(&sibling_id)
                .expect("sibling record")
                .state
                .is_terminal(),
            "the sibling reached its terminal state under supervision"
        );
        assert_eq!(
            runtime
                .background_status(&failing_id)
                .expect("failing record")
                .state,
            crate::tools::background::BackgroundLifecycle::PublishingTerminal,
            "the unresolved candidate stays explicit, never fabricated terminal"
        );
        let pending_items = store.load_pending().expect("pending inbound");
        assert!(
            pending_items
                .iter()
                .any(|item| { format!("{item:?}").contains(sibling_id.as_str()) }),
            "the supervised sibling published its terminal inbound durably"
        );
        assert!(
            !pending_items
                .iter()
                .any(|item| format!("{item:?}").contains(failing_id.as_str())),
            "no false terminal inbound exists for the unresolved record"
        );
    }

    /// M9c (settlement linearization): a background terminal-publication
    /// failure is not logically settled until the runner has completed its
    /// **last conversation-facing failure callback**. `publication_abandoned`
    /// is the fact runtime drain consumes as this owner's settlement, so it
    /// must never become observable while the failure sink can still call
    /// back into the conversation — otherwise drain could aggregate the
    /// abandoned evidence and cache a failed shutdown *before* the runner
    /// published its `DurabilityFailed` observation.
    ///
    /// This regression races shutdown against the failure sink itself, which
    /// the sibling-background regression deliberately does not: there the
    /// `DurabilityFailed` observation is awaited *before* shutdown starts, so
    /// the callback is already complete.
    ///
    /// Happens-before: `background_failure_gate` parks the runner inside
    /// `BackgroundFailureSink::terminal_publication_failed`, before the
    /// coordinator lock and before the durability-health mutation. While it
    /// is parked, the callback has provably started and provably not
    /// returned. `drain_linearization` then proves `Running -> Draining`
    /// committed and `drain_supervision` proves the drain task is committed
    /// to awaiting this exact owner — yet neither shutdown caller may return.
    /// Releasing the gate is the only thing that lets the failure become
    /// observable, then the abandoned fact, then the shutdown failure.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn abandoned_publication_never_precedes_the_last_failure_callback() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_414701e8-d767-76d9-88db-c5d4e270cf81",
            ))
            .expect("in-memory store"),
        );
        let background_failure_gate = Arc::new(super::Gate::default());
        background_failure_gate.arm();
        let drain_linearization = Arc::new(tokio::sync::Notify::new());
        let drain_supervision = Arc::new(tokio::sync::Notify::new());
        let (runtime, _model) = headless_runtime_over_store_with(
            &dir,
            "conv_414701e8-d767-76d9-88db-c5d4e270cf81",
            store.clone(),
            vec![one_turn_script()],
            Some(CoordinatorProbe {
                background_failure_gate: Some(background_failure_gate.clone()),
                subagent_failure_published_gate: None,
                drain_linearization: Some(drain_linearization.clone()),
                drain_supervision: Some(drain_supervision.clone()),
                ..CoordinatorProbe::default()
            }),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();
        let background = runtime.tool_runtime().background().clone();

        let (executor, mut started, release_executor) = GatedBackgroundExecutor::new();
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
        let execution_id = commit_background(&runtime, &executor, "call-abandon-order");
        started
            .wait_for(|is_started| *is_started)
            .await
            .expect("start channel stays open");

        // Exactly the bounded publication budget: durable attempt #1 (inside
        // `finish`) and durable attempt #2 (the one registry-owned retry).
        store.arm_fail_accept_times(2);
        release_executor.send_replace(true);

        // (5)+(6) The runner entered its last conversation-facing callback
        // and is parked *inside* it. The park is the callback boundary
        // itself, so from here the callback has provably started and
        // provably not returned.
        background_failure_gate.wait_entered();
        let before_release = pending.drain();
        assert!(
            !before_release.iter().any(|observation| matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )),
            "the parked callback has not completed, so it published nothing"
        );
        assert!(
            runtime.inner.durability_failure_diagnostic().is_none(),
            "the parked callback has not mutated durability health yet"
        );
        assert_eq!(
            runtime
                .background_status(&execution_id)
                .expect("record")
                .state,
            crate::tools::background::BackgroundLifecycle::PublishingTerminal,
            "the record retains its unresolved terminal candidate"
        );
        assert!(
            background.abandoned_publications().is_empty(),
            "the abandoned settlement fact must not precede the failure callback"
        );

        // (7) Shutdown races the parked failure sink.
        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let shutdown_runtime = runtime.clone();
        tokio::spawn(async move {
            let _ = done_tx.send(shutdown_runtime.shutdown().await);
        });
        within_liveness_guard(
            "the drain transition to linearize",
            drain_linearization.notified(),
        )
        .await;
        within_liveness_guard(
            "drain to park on the failing background execution",
            drain_supervision.notified(),
        )
        .await;

        // A second, concurrent caller must join the same pending drain rather
        // than start a competing one.
        let (second_tx, mut second_rx) = tokio::sync::oneshot::channel();
        let second_runtime = runtime.clone();
        tokio::spawn(async move {
            let _ = second_tx.send(second_runtime.shutdown().await);
        });

        // (8) Give both callers every scheduling opportunity to finish
        // wrongly, then prove neither did.
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
        let completion = runtime
            .inner
            .drain
            .get()
            .expect("the one shared drain completion exists")
            .clone();
        assert!(
            !completion
                .completed
                .load(std::sync::atomic::Ordering::Acquire),
            "the shared drain completion must not be filled while the callback is live"
        );
        assert!(
            matches!(
                done_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "shutdown must not return while the runner still owns a callback"
        );
        assert!(
            matches!(
                second_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "the concurrent caller joins the same pending drain"
        );
        assert!(
            background.abandoned_publications().is_empty(),
            "the record has not crossed its abandoned settlement boundary"
        );
        assert!(
            !pending.drain().iter().any(|observation| matches!(
                observation,
                ConversationObservation::DurabilityFailed { .. }
            )),
            "the parked callback still published nothing"
        );
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Draining,
            "a live owned callback can never publish Quiescent"
        );

        // (9)+(10) Releasing the callback is the only thing that can order
        // the failure, then the abandoned fact, then the shutdown failure.
        background_failure_gate.release();
        await_observation(pending.as_ref(), |observation| {
            matches!(observation, ConversationObservation::DurabilityFailed { operation, .. }
                if operation == "background_terminal_publication")
        })
        .await;
        let shutdown = within_liveness_guard("the supervised shutdown to return", done_rx)
            .await
            .expect("shutdown result channel");
        assert!(
            matches!(
                &shutdown,
                Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                    detail
                }) if detail.contains("terminal publication")
            ),
            "the abandoned publication is honest settlement evidence: {shutdown:?}"
        );
        assert_eq!(
            background.abandoned_publications(),
            vec![execution_id.clone()],
            "the abandoned fact is observable only after the callback returned"
        );
        assert!(
            runtime.inner.durability_failure_diagnostic().is_some(),
            "the failure callback committed before the abandoned fact"
        );
        let second = within_liveness_guard("the joined shutdown to return", second_rx)
            .await
            .expect("second shutdown result channel");
        assert!(
            matches!(
                &second,
                Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                    detail
                }) if detail.contains("terminal publication")
            ),
            "the joined caller observes the same supervised failure: {second:?}"
        );

        // (11) After the cached failure exists, that runner owns nothing.
        let events_after = store.read_events(None, 1024).expect("events").events.len();
        let pending_after = store.load_pending().expect("pending inbound").len();
        let record_after = runtime.background_status(&execution_id).expect("record");
        let health_after = runtime.inner.durability_failure_diagnostic();
        let _ = pending.drain();
        assert!(
            runtime
                .submit_inbound(text_content("after shutdown"))
                .is_err(),
            "a stale inbound handle is refused after the drain decided"
        );
        // A stale settlement handle of the same execution is a local no-op.
        background.finish(
            &execution_id,
            &crate::tools::types::ToolExecutionResult {
                status: crate::tools::types::ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        );
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
        assert!(
            pending.drain().is_empty(),
            "no conversation observation follows the settled runner"
        );
        assert_eq!(
            store.read_events(None, 1024).expect("events").events.len(),
            events_after,
            "no event journal effect follows the settled runner"
        );
        assert_eq!(
            store.load_pending().expect("pending inbound").len(),
            pending_after,
            "no Pending Inbound acceptance follows the settled runner"
        );
        assert_eq!(
            runtime
                .background_status(&execution_id)
                .expect("record")
                .state,
            record_after.state,
            "no background state mutation follows the settled runner"
        );
        assert_eq!(
            runtime.inner.durability_failure_diagnostic(),
            health_after,
            "no durability-health mutation follows the settled runner"
        );

        // (12) The cached failure is honest: the original supervision had
        // already completed before `DrainCompletion` was filled.
        let repeated =
            within_liveness_guard("the cached shutdown failure", runtime.shutdown()).await;
        assert!(
            matches!(
                &repeated,
                Err(crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                    detail
                }) if detail.contains("terminal publication")
            ),
            "a later caller observes the cached supervised failure: {repeated:?}"
        );
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
        assert!(
            pending.drain().is_empty(),
            "the cached failure needs no fresh supervision and triggers no callback"
        );
        assert_eq!(
            store.read_events(None, 1024).expect("events").events.len(),
            events_after,
            "the repeated shutdown produces no durable effect"
        );
    }

    /// M9c (Fix D): the current-attempt **slot** and the attempt **task**
    /// are distinct ownership facts. The slot is cleared inside
    /// `finish_attempt`, but the task still owes the coordinator its final
    /// admission callback, so quiescence must wait for the task itself.
    ///
    /// Happens-before: `attempt_exit_gate` parks the settled attempt task
    /// after the coordinator lock is released and the slot is provably empty,
    /// and before the final callback runs. `drain_linearization` proves the
    /// drain transition committed while the task is parked there, and
    /// `mark_quiescent` — the one authority that publishes quiescence — is
    /// then invoked directly and must refuse. Releasing the gate is the only
    /// thing that lets the task return and shutdown complete.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn attempt_task_exit_belongs_to_the_quiescence_proof() {
        let attempt_exit_gate = Arc::new(super::Gate::default());
        attempt_exit_gate.arm();
        let drain_linearization = Arc::new(tokio::sync::Notify::new());
        let fixture = headless_fixture_with(Some(CoordinatorProbe {
            attempt_exit_gate: Some(attempt_exit_gate.clone()),
            parent_guidance_seal_gate: None,
            drain_linearization: Some(drain_linearization.clone()),
            ..CoordinatorProbe::default()
        }))
        .await;
        fixture
            .runtime
            .submit_inbound(text_content("one turn"))
            .expect("accepted");
        attempt_exit_gate.wait_entered();
        assert!(
            !fixture.runtime.has_current_attempt(),
            "the current-attempt slot is already clear at the parked exit boundary"
        );

        assert!(
            fixture.runtime.idle_epoch().is_err(),
            "idle residency also waits for the native attempt task exit"
        );

        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let shutdown_runtime = fixture.runtime.clone();
        tokio::spawn(async move {
            let _ = done_tx.send(shutdown_runtime.shutdown().await);
        });
        within_liveness_guard("the drain linearization", drain_linearization.notified()).await;
        assert!(
            !fixture.runtime.inner.lifecycle.mark_quiescent(),
            "the attempt task still owes a callback, so quiescence is refused"
        );
        assert!(
            matches!(
                done_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "an empty current-attempt slot is not attempt-task settlement"
        );
        assert_eq!(
            fixture.runtime.lifecycle_state(),
            ConversationLifecycleState::Draining
        );

        let release = {
            let gate = attempt_exit_gate.clone();
            tokio::task::spawn_blocking(move || gate.release())
        };
        release.await.expect("release the parked attempt task");
        done_rx
            .await
            .expect("shutdown result channel")
            .expect("the attempt task exit completes the quiescence proof");
        assert_eq!(
            fixture.runtime.lifecycle_state(),
            ConversationLifecycleState::Quiescent
        );

        // A stale handle to the runtime's own admission callback cannot
        // produce a semantic effect after quiescence.
        let store = fixture.runtime.tool_runtime().durable_store();
        let events_before = store.read_events(None, 256).expect("events").events;
        let canonical_before = store.load_canonical().expect("canonical");
        let inner = fixture
            .runtime
            .weak_inner()
            .upgrade()
            .expect("the test still owns the runtime");
        inner.admit_next_attempt();
        assert!(!fixture.runtime.has_current_attempt());
        assert_eq!(
            store.read_events(None, 256).expect("events").events,
            events_before
        );
        assert_eq!(store.load_canonical().expect("canonical"), canonical_before);
        assert!(matches!(
            fixture.runtime.submit_inbound(text_content("late")),
            Err(InboundAdmissionError::Shutdown)
        ));
    }

    /// M9c (Fix E / 8.1): the exact `Running -> Draining` linearization, not
    /// an arrival hint, is what a competing acceptance must lose to.
    ///
    /// Happens-before: `drain_linearization` fires immediately after the
    /// lifecycle CAS commits, while shutdown still holds the coordinator
    /// lock. The competing `submit_inbound` is released only after that
    /// signal, so it necessarily queues on the coordinator lock and reads the
    /// already-published `Draining` state. It commits nothing and consumes no
    /// sequence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn drain_linearization_precedes_the_refused_acceptance() {
        let admission_gate = Arc::new(super::Gate::default());
        let drain_linearization = Arc::new(tokio::sync::Notify::new());
        let fixture = headless_fixture_with(Some(CoordinatorProbe {
            admission_gate: Some(admission_gate.clone()),
            drain_linearization: Some(drain_linearization.clone()),
            ..CoordinatorProbe::default()
        }))
        .await;
        // Freeze admission so the pre-shutdown item stays pending and the
        // durable acceptance ledger is stable for the assertions below.
        admission_gate.arm();
        let first = fixture
            .runtime
            .submit_inbound(text_content("before"))
            .expect("pre-shutdown acceptance");
        assert_eq!(first.inbound_sequence.get(), 1);
        admission_gate.wait_entered();

        // The competing acceptance is released only after drain has provably
        // linearized.
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let (late_tx, late_rx) = tokio::sync::oneshot::channel();
        let late_runtime = fixture.runtime.clone();
        tokio::spawn(async move {
            release_rx.await.expect("release channel stays open");
            let _ = late_tx.send(late_runtime.submit_inbound(text_content("racing")));
        });

        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let shutdown_runtime = fixture.runtime.clone();
        tokio::spawn(async move {
            let _ = done_tx.send(shutdown_runtime.shutdown().await);
        });
        within_liveness_guard("the drain linearization", drain_linearization.notified()).await;
        assert_eq!(
            fixture.runtime.lifecycle_state(),
            ConversationLifecycleState::Draining,
            "the drain transition is committed before the competing acceptance runs"
        );
        release_tx
            .send(())
            .expect("release the competing acceptance");
        assert!(
            matches!(
                late_rx.await.expect("late acceptance result"),
                Err(InboundAdmissionError::Shutdown)
            ),
            "an acceptance that starts after the drain commit is refused"
        );

        admission_gate.release();
        done_rx
            .await
            .expect("shutdown result channel")
            .expect("drain completes");

        let batch = fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .select_pending_batch()
            .expect("select")
            .expect("exactly one pre-shutdown pending item");
        assert_eq!(
            batch.items().len(),
            1,
            "the refused acceptance committed no pending item"
        );
        assert_eq!(
            batch.items()[0].sequence().get(),
            1,
            "the refused acceptance consumed no sequence"
        );
    }

    /// M9c: a conversation-owned background execution survives attempt
    /// settlement but not conversation lifetime. Drain requests cancellation,
    /// waits for the executor and its exactly-once terminal Pending Inbound
    /// publication, and only then becomes quiescent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn runtime_shutdown_drains_active_background_and_publishes_terminal_inbound() {
        let dir = tempfile::tempdir().expect("temp dir");
        let drain_linearization = Arc::new(tokio::sync::Notify::new());
        let (runtime, _model) = runtime_with_model_probe_at(
            &dir,
            "conv_5955a0a5-829c-7229-b261-cf19b4990758",
            Vec::new(),
            vec![one_turn_script()],
            Some(CoordinatorProbe {
                drain_linearization: Some(drain_linearization.clone()),
                ..CoordinatorProbe::default()
            }),
        )
        .await
        .expect("runtime");
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        let (executor, mut started, release) = GatedBackgroundExecutor::new();
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(executor);
        let invocation = crate::tools::types::ToolInvocation {
            id: crate::tools::types::ToolInvocationId::Agent {
                call_id: crate::runtime::identity::ToolCallId::new("call-m9c-background"),
            },
            tool_id: crate::runtime::identity::ToolId::new("tool-m9c-background"),
            tool_name: "background_gate".to_owned(),
            mode: crate::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let background = runtime.tool_runtime().background();
        let prepared = background
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let crate::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
            background
                .commit_dispatch(
                    prepared,
                    &crate::runtime::cancellation::CancellationSignal::new(),
                )
                .expect("ownership commit")
        else {
            panic!("active background dispatch must commit ownership");
        };
        started
            .wait_for(|is_started| *is_started)
            .await
            .expect("background start channel stays open");

        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let shutdown_runtime = runtime.clone();
        tokio::spawn(async move {
            let result = shutdown_runtime.shutdown().await;
            let _ = done_tx.send(result);
        });
        drain_linearization.notified().await;
        background
            .wait_until_cancelling(&execution_id)
            .await
            .expect("runtime drain requests background cancellation");
        assert!(
            matches!(
                done_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "cancellation intent is not background terminality"
        );

        release.send_replace(true);
        done_rx
            .await
            .expect("shutdown result channel")
            .expect("background drain completes");
        let terminal = background
            .snapshot(&execution_id)
            .expect("terminal background record");
        // The executor raced past the drain's cancellation request and
        // proved success: cancellation intent (including drain intent)
        // cannot overwrite the executor-proven outcome (Issue #202).
        assert_eq!(
            terminal.state,
            crate::tools::background::BackgroundLifecycle::Succeeded
        );
        assert!(matches!(
            terminal.result.as_ref().map(|result| &result.status),
            Some(crate::tools::types::ToolExecutionStatus::Success)
        ));
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Quiescent
        );
        let pending_rows = runtime
            .tool_runtime()
            .mailbox()
            .select_pending_batch()
            .expect("pending terminal notification")
            .expect("terminal notification remains pending after admission closure");
        assert_eq!(pending_rows.items().len(), 1);
        assert_eq!(
            runtime
                .tool_runtime()
                .mailbox()
                .select_pending_batch()
                .expect("retry select")
                .expect("same pending row")
                .items()[0]
                .sequence(),
            pending_rows.items()[0].sequence(),
            "terminal publication is exactly once and retains its identity"
        );
        let terminal_events = runtime
            .tool_runtime()
            .durable_store()
            .read_events(None, 256)
            .expect("events")
            .events
            .into_iter()
            .filter(|event| {
                matches!(
                    event.event,
                    crate::events::types::RuntimeEvent::BackgroundTerminalPublished { .. }
                )
            })
            .count();
        assert_eq!(terminal_events, 1);
    }

    /// Issue #83: conversation drain supervises a Workflow-owned native child
    /// through the same `SubagentRegistry` boundary. Cancellation is observed
    /// at the child control channel, while shutdown remains parked until the
    /// child sends its native terminal frame and the registry proves reap and
    /// durable settlement.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn runtime_shutdown_waits_for_workflow_child_native_quiescence() {
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_0aca8719-5a71-735b-8721-8979eb90371a");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(conversation_id.clone())
                .expect("store"),
        );
        let (runtime, _model, subagents) =
            headless_runtime_over_store_with_subagents(&dir, conversation_id.as_str(), store, None)
                .await;
        runtime.activate();

        let runtime_root = dir.path().join("subagents");
        std::fs::create_dir_all(&runtime_root).expect("subagent runtime root");
        let (driver_end, mut peer) = tokio::net::UnixStream::pair().expect("IPC pair");
        let (observation_end, _observation_peer) =
            tokio::net::UnixStream::pair().expect("observation pair");
        let child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg("true")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .expect("staged native child");
        let child_pid = child.id().expect("child pid");
        let child_root = runtime_root.join(format!("test-child-{child_pid}"));
        std::fs::create_dir_all(&child_root).expect("child runtime root");
        subagents.push_staged_override(crate::runtime::subagent::process::StagedChild::for_test(
            child,
            driver_end,
            observation_end,
            child_root.clone(),
        ));

        let workflow_id =
            crate::runtime::workflow::WorkflowId::parse("drain_workflow").expect("workflow id");
        let node_instance = crate::runtime::workflow::test_instance("drain_workflow", "review");
        let run_id = node_instance.block.run.clone();
        let prepared = subagents
            .prepare(
                &crate::runtime::subagent::SubagentStartSpec {
                    authority: crate::runtime::subagent::DurableAgentAuthority {
                        execution_policy:
                            crate::runtime::subagent::InheritedExecutionPolicy::default(),
                        resolved: test_resolved_subagent("reviewer"),
                        approval_mode: ApprovalMode::Policy,
                    },
                    admission: crate::runtime::subagent::ActivationAdmission {
                        task: "Produce the workflow result.".to_owned(),
                        context: None,
                        origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                            tool_call_id: ToolCallId::new("workflow:drain_workflow:review"),
                        },
                        terminal: crate::runtime::subagent::SubagentTerminalMode::WorkflowOutput {
                            output_schema: serde_json::json!({
                                "type": "object",
                                "properties": {"summary": {"type": "string"}},
                                "required": ["summary"],
                                "additionalProperties": false
                            }),
                            workflow_id,
                            run_id,
                            node_id: Box::new(node_instance),
                        },
                    },
                },
                &CancellationSignal::new(),
            )
            .await
            .expect("prepare workflow child");
        let accepted = match subagents
            .commit(prepared, &CancellationSignal::new())
            .await
            .expect("commit workflow child")
        {
            crate::runtime::subagent::SubagentStartOutcome::Accepted(accepted) => accepted,
            crate::runtime::subagent::SubagentStartOutcome::RolledBack => {
                panic!("workflow child was cancelled before admission")
            }
        };
        assert!(
            runtime.idle_epoch().is_err(),
            "workflow child ownership prevents idle claim"
        );
        assert!(matches!(
            crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
                .await
                .expect("delegate frame"),
            Some(crate::runtime::subagent::ipc::ParentFrame::Delegate(_))
        ));

        assert_pending_mutation_preserves_child(&runtime, &subagents, &accepted.subagent_id);

        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let shutdown_runtime = runtime.clone();
        tokio::spawn(async move {
            let _ = done_tx.send(shutdown_runtime.shutdown().await);
        });

        assert!(matches!(
            crate::runtime::subagent::ipc::read_parent_frame(&mut peer)
                .await
                .expect("cancellation frame"),
            Some(crate::runtime::subagent::ipc::ParentFrame::Cancel {
                reason: Some(CancellationReason::RuntimeShutdown)
            })
        ));
        assert!(matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        assert!(!runtime.is_quiescent());
        assert_eq!(subagents.unsettled_snapshot().len(), 1);

        crate::runtime::subagent::ipc::write_child_frame(
            &mut peer,
            &crate::runtime::subagent::ipc::ChildFrame::Result(
                crate::runtime::subagent::ipc::ResultFrame {
                    status: crate::runtime::subagent::ipc::ChildResultStatus::Cancelled,
                    content: None,
                    diagnostic: None,
                },
            ),
        )
        .await
        .expect("native cancellation result");

        done_rx
            .await
            .expect("shutdown result")
            .expect("workflow child drain");
        assert!(runtime.is_quiescent());
        assert!(subagents.unsettled_snapshot().is_empty());
        assert!(!child_root.exists());
        let snapshot = subagents
            .snapshot(&accepted.subagent_id)
            .expect("retained child snapshot");
        assert_eq!(
            snapshot.state,
            crate::runtime::subagent::SubagentState::Cancelled
        );
        assert!(snapshot.settled);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_shutdown_rejects_unproven_child_physical_settlement() {
        use crate::runtime::subagent::ipc::{
            ChildFrame, ChildResultStatus, ParentFrame, ResultFrame, read_parent_frame,
            write_child_frame,
        };
        use crate::runtime::subagent::{
            ActivationAdmission, AgentActivationOrigin, DurableAgentAuthority,
            InheritedExecutionPolicy, SubagentStartOutcome, SubagentStartSpec,
            SubagentTerminalMode,
        };
        let dir = tempfile::tempdir().unwrap();
        let conversation = ConversationId::generate();
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(conversation.clone()).unwrap(),
        );
        let (runtime, _, subagents) =
            headless_runtime_over_store_with_subagents(&dir, conversation.as_str(), store, None)
                .await;
        runtime.activate();
        let (mut staged, mut peer) = stage_runtime_test_child(&dir.path().join("unproven-child"));
        staged.retain_for_test(
            crate::runtime::identity::ProcessUnitId::new("unproven-unit"),
            i32::MAX,
        );
        subagents.push_staged_override(staged);
        let prepared = subagents
            .prepare(
                &SubagentStartSpec {
                    authority: DurableAgentAuthority {
                        resolved: test_resolved_subagent("reviewer"),
                        execution_policy: InheritedExecutionPolicy::default(),
                        approval_mode: ApprovalMode::Policy,
                    },
                    admission: ActivationAdmission {
                        task: "Exercise failed physical settlement".into(),
                        context: None,
                        origin: AgentActivationOrigin::CreationTool {
                            tool_call_id: ToolCallId::new("unproven-create"),
                        },
                        terminal: SubagentTerminalMode::Normal,
                    },
                },
                &CancellationSignal::new(),
            )
            .await
            .unwrap();
        let SubagentStartOutcome::Accepted(admitted) = subagents
            .commit(prepared, &CancellationSignal::new())
            .await
            .unwrap()
        else {
            panic!("admission cancelled");
        };
        assert!(matches!(
            read_parent_frame(&mut peer).await.unwrap(),
            Some(ParentFrame::Delegate(_))
        ));
        write_child_frame(
            &mut peer,
            &ChildFrame::Result(ResultFrame {
                status: ChildResultStatus::Succeeded,
                content: Some("semantic result".into()),
                diagnostic: None,
            }),
        )
        .await
        .unwrap();
        let terminal = subagents
            .wait_until_settled(&admitted.subagent_id)
            .await
            .unwrap();
        assert!(
            !terminal.settled,
            "semantic termination cannot prove containment"
        );
        let error = runtime
            .shutdown()
            .await
            .expect_err("unproven owner prevents quiescence");
        assert!(
            matches!(error, super::ShutdownError::RuntimeOwnedSettlement { ref detail }
            if detail.contains(admitted.subagent_id.as_str()) && detail.contains("physical settlement is unresolved")),
            "{error:?}"
        );
        assert!(!runtime.is_quiescent());
    }

    /// M9c: the foreground Bash path composes its existing physical process
    /// proof into runtime quiescence. Drain requests cancellation while the
    /// supervised process is owned; the test observes the real process-group
    /// terminal frame, parks before the direct supervisor-child reap, and
    /// proves shutdown cannot complete until that native settlement handoff
    /// is released.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn runtime_shutdown_waits_for_foreground_process_terminality() {
        use crate::runtime::process_runner::RunnerLifecycleHook;
        use crate::runtime::types::ConversationLifecycleState;
        use crate::tools::executor::ToolRegistry;
        use crate::tools::native::{BashTestControl, BashTool};
        use crate::tools::types::{
            ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin,
            ToolReplayPolicy,
        };

        let control = BashTestControl::new().hold_terminal_event();
        let process_lifecycle: RunnerLifecycleHook = control.runner_control().lifecycle.clone();
        let terminal_hold = control
            .terminal_hold()
            .expect("terminal hold is armed")
            .clone();
        let definition = ToolDefinition {
            id: crate::runtime::identity::ToolId::new("tool-bash"),
            name: "bash".to_owned(),
            description: "run one supervised bash command".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "minLength": 1},
                    "timeout": {"type": "integer", "minimum": 1}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Sequential,
            approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        };
        let mut registry = ToolRegistry::new();
        registry
            .register(
                definition.clone(),
                Arc::new(BashTool::with_test_control(control.clone())),
            )
            .expect("test Bash registration");
        let call_id = crate::runtime::identity::ToolCallId::new("call-m9c-process");
        let first_request = vec![
            FakeStep::Emit(crate::model::event::ModelEvent::Started),
            FakeStep::Emit(crate::model::event::ModelEvent::ToolCallStarted {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCallStart {
                    id: call_id.clone(),
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                },
            }),
            FakeStep::Emit(crate::model::event::ModelEvent::ToolCallArgumentsDelta {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                call_id: call_id.clone(),
                arguments_delta: r#"{"command":"sleep 30"}"#.to_owned(),
            }),
            FakeStep::Emit(crate::model::event::ModelEvent::ToolCallCompleted {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCall {
                    id: call_id,
                    tool_id: definition.id,
                    name: definition.name,
                    arguments: serde_json::json!({"command": "sleep 30"}),
                },
            }),
            FakeStep::Emit(crate::model::event::ModelEvent::Completed {
                finish_reason: crate::model::finish::ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ];
        let dir = tempfile::tempdir().expect("temp dir");
        let drain_linearization = Arc::new(tokio::sync::Notify::new());
        let (runtime, _model) = headless_runtime(
            &dir,
            vec![first_request, one_turn_script()],
            Some(registry),
            Some(CoordinatorProbe {
                drain_linearization: Some(drain_linearization.clone()),
                ..CoordinatorProbe::default()
            }),
        )
        .await;
        runtime.activate();
        runtime
            .submit_inbound(text_content("run the supervised process"))
            .expect("accepted");

        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            process_lifecycle.await_ownership_established(),
        )
        .await
        .expect("the foreground process reaches physical ownership");

        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let shutdown_runtime = runtime.clone();
        tokio::spawn(async move {
            let _ = done_tx.send(shutdown_runtime.shutdown().await);
        });
        drain_linearization.notified().await;

        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            terminal_hold.await_held(),
        )
        .await
        .expect("TERM/KILL and process-group terminality are observed");
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Draining
        );
        assert!(
            matches!(
                done_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "group terminality without direct-child reap is not runtime quiescence"
        );
        assert!(
            control
                .recorded_signals()
                .iter()
                .any(|signal| signal.signal == "SIGTERM" && signal.emitted),
            "runtime cancellation reaches the owned process group"
        );

        terminal_hold.release();
        done_rx
            .await
            .expect("shutdown result channel")
            .expect("foreground process drain completes");
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Quiescent
        );
    }

    /// M9c: the Agent Loop remains the foreground batch owner while runtime
    /// drain cancels it. A deterministic tool-start frontier parks after the
    /// first parallel sibling; drain wins there, the second sibling is never
    /// announced or invoked, and the first started sibling still settles
    /// before runtime quiescence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn runtime_shutdown_closes_parallel_foreground_start_frontier() {
        use crate::agent::execution::test_sync::ToolStartPause;
        use crate::tools::executor::ToolRegistry;
        use crate::tools::types::{
            ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin,
            ToolReplayPolicy,
        };

        let definition = |id: &str, name: &str| ToolDefinition {
            id: crate::runtime::identity::ToolId::new(id),
            name: name.to_owned(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Parallel,
            approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        };
        let (tool_a, started_a, _release_a) = GatedBackgroundExecutor::new();
        let (tool_b, started_b, _release_b) = GatedBackgroundExecutor::new();
        let mut registry = ToolRegistry::new();
        registry
            .register(definition("tool-a", "alpha"), Arc::new(tool_a))
            .expect("tool A registration");
        registry
            .register(definition("tool-b", "beta"), Arc::new(tool_b))
            .expect("tool B registration");

        let call_a = crate::scripted_suites::support::fake::ScriptedCall {
            id: "call-a",
            tool_id: "tool-a",
            name: "alpha",
            arguments: serde_json::json!({}),
        };
        let call_b = crate::scripted_suites::support::fake::ScriptedCall {
            id: "call-b",
            tool_id: "tool-b",
            name: "beta",
            arguments: serde_json::json!({}),
        };
        let mut script = vec![FakeStep::Emit(crate::model::event::ModelEvent::Started)];
        script.extend(
            crate::scripted_suites::support::fake::tool_call_events(0, &call_a)
                .into_iter()
                .map(FakeStep::Emit),
        );
        script.extend(
            crate::scripted_suites::support::fake::tool_call_events(1, &call_b)
                .into_iter()
                .map(FakeStep::Emit),
        );
        script.push(FakeStep::Emit(crate::model::event::ModelEvent::Completed {
            finish_reason: crate::model::finish::ModelFinishReason::ToolCalls,
            usage: None,
        }));

        let dir = tempfile::tempdir().expect("temp dir");
        let drain_linearization = Arc::new(tokio::sync::Notify::new());
        let (tool_start_pause, mut tool_start_reached, tool_start_release) =
            ToolStartPause::install();
        let (runtime, _model) = headless_runtime(
            &dir,
            vec![script],
            Some(registry),
            Some(CoordinatorProbe {
                drain_linearization: Some(drain_linearization.clone()),
                tool_start_pause: Some(tool_start_pause),
                drain_supervision: None,
                attempt_exit_gate: None,
                parent_guidance_seal_gate: None,
                ..CoordinatorProbe::default()
            }),
        )
        .await;
        runtime.activate();
        runtime
            .submit_inbound(text_content("run parallel tools"))
            .expect("accepted");
        tool_start_reached
            .wait_for(|reached| *reached)
            .await
            .expect("tool-start gate stays open");

        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let shutdown_runtime = runtime.clone();
        tokio::spawn(async move {
            let _ = done_tx.send(shutdown_runtime.shutdown().await);
        });
        drain_linearization.notified().await;
        assert!(matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        tool_start_release
            .send(())
            .expect("release the first start frontier");
        done_rx
            .await
            .expect("shutdown result channel")
            .expect("parallel foreground work settles");

        assert!(
            !*started_a.borrow(),
            "drain prevents the first physical poll"
        );
        assert!(!*started_b.borrow(), "the second sibling never executes");
        let events = runtime
            .tool_runtime()
            .durable_store()
            .read_events(None, 256)
            .expect("event journal")
            .events;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    crate::events::types::RuntimeEvent::ToolExecutionStarted { .. }
                ))
                .count(),
            1,
            "only the first parallel sibling crossed the start frontier"
        );
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Quiescent
        );
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

    /// A public runtime composition rejects an invalid model timeout policy
    /// before it initializes storage or claims either ownership plane. The
    /// same untouched tool-runtime/capability bundle can immediately build a
    /// valid runtime afterwards.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(
        clippy::too_many_lines,
        reason = "one complete deterministic fixture boundary"
    )]
    async fn invalid_model_timeout_policy_fails_before_runtime_ownership_transfer() {
        struct Fixture {
            tool_runtime: crate::tools::runtime::ConversationToolRuntime,
            capability: crate::capabilities::CapabilityCoordinator,
            resources: Arc<crate::runtime::RuntimeResourceSnapshot>,
            resource_loader: Arc<dyn crate::runtime::RuntimeResourceLoader>,
            _dir: tempfile::TempDir,
        }

        impl Fixture {
            fn config(
                &self,
                model_timeout_policy: crate::model::ModelTimeoutPolicy,
            ) -> RuntimeConversationConfig {
                RuntimeConversationConfig {
                    explicit_model: true,
                    agent_id: AgentId::new("agent-timeout-construction"),
                    model: scripted_session_model(Arc::new(FakeModel::new(Vec::new()))),
                    approval_mode: ApprovalMode::Policy,
                    model_timeout_policy,
                    tool_deadline_policy:
                        crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
                    context: ConversationContextConfig {
                        policy: crate::context::SessionContextPolicy {
                            reserve_tokens: 0,
                            keep_recent_tokens: 0,
                            summary_output_cap: None,
                        },
                        estimator: Arc::new(DefaultTokenEstimator),
                        status_engine: Some(AgentStatusEngine::default()),
                    },
                    tool_runtime: self.tool_runtime.clone(),
                    capability: self.capability.clone(),
                    resources: self.resources.clone(),
                    resource_loader: self.resource_loader.clone(),
                    clock: None,
                    initial_messages: Vec::new(),
                    subagents: None,
                    workflow_output: None,
                }
            }
        }

        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_7aeab4f1-ecc6-7d5d-85f7-f7c3015f36ee");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("tool runtime");
        let capability = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id,
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(ToolRegistry::new()),
                extension_tools: tool_runtime.extension_tool_plane(),
                agent_activation: fixture_activation(&tool_runtime),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("capability coordinator");
        let candidate = capability.prepare_candidate().await.expect("candidate");
        capability.commit(candidate).expect("candidate commit");
        let fixture = Fixture {
            resources: test_resources(&capability),
            resource_loader: test_resource_loader(&capability),
            tool_runtime,
            capability,
            _dir: dir,
        };

        for model_timeout_policy in [
            crate::model::ModelTimeoutPolicy::new(
                std::time::Duration::ZERO,
                std::time::Duration::from_secs(1),
            ),
            crate::model::ModelTimeoutPolicy::new(
                std::time::Duration::from_secs(1),
                std::time::Duration::ZERO,
            ),
        ] {
            assert!(matches!(
                ConversationRuntime::new(fixture.config(model_timeout_policy)),
                Err(ConversationRuntimeError::InvalidModelTimeoutPolicy)
            ));
        }

        let runtime =
            ConversationRuntime::new(fixture.config(crate::model::ModelTimeoutPolicy::default()))
                .expect("valid construction can reuse both untouched ownership planes");
        runtime.activate();
        runtime
            .shutdown()
            .await
            .expect("valid runtime admits no work and shuts down cleanly");
    }

    /// A public runtime composition rejects an invalid tool
    /// execution-liveness policy (Issue #204) before it initializes storage
    /// or claims either ownership plane: a zero hard deadline would admit an
    /// execution that can never live, and a zero idle window would fire at
    /// every observation. The same untouched tool-runtime/capability bundle
    /// can immediately build a valid runtime afterwards.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(
        clippy::too_many_lines,
        reason = "one complete deterministic fixture boundary"
    )]
    async fn invalid_tool_deadline_policy_fails_before_runtime_ownership_transfer() {
        struct Fixture {
            tool_runtime: crate::tools::runtime::ConversationToolRuntime,
            capability: crate::capabilities::CapabilityCoordinator,
            resources: Arc<crate::runtime::RuntimeResourceSnapshot>,
            resource_loader: Arc<dyn crate::runtime::RuntimeResourceLoader>,
            _dir: tempfile::TempDir,
        }

        impl Fixture {
            fn config(
                &self,
                tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy,
            ) -> RuntimeConversationConfig {
                RuntimeConversationConfig {
                    explicit_model: true,
                    agent_id: AgentId::new("agent-tool-deadline-construction"),
                    model: scripted_session_model(Arc::new(FakeModel::new(Vec::new()))),
                    approval_mode: ApprovalMode::Policy,
                    model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                    tool_deadline_policy,
                    context: ConversationContextConfig {
                        policy: crate::context::SessionContextPolicy {
                            reserve_tokens: 0,
                            keep_recent_tokens: 0,
                            summary_output_cap: None,
                        },
                        estimator: Arc::new(DefaultTokenEstimator),
                        status_engine: Some(AgentStatusEngine::default()),
                    },
                    tool_runtime: self.tool_runtime.clone(),
                    capability: self.capability.clone(),
                    resources: self.resources.clone(),
                    resource_loader: self.resource_loader.clone(),
                    clock: None,
                    initial_messages: Vec::new(),
                    subagents: None,
                    workflow_output: None,
                }
            }
        }

        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_b3920643-26f1-71a1-81eb-f19d0c413769");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("tool runtime");
        let capability = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id,
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(ToolRegistry::new()),
                extension_tools: tool_runtime.extension_tool_plane(),
                agent_activation: fixture_activation(&tool_runtime),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("capability coordinator");
        let candidate = capability.prepare_candidate().await.expect("candidate");
        capability.commit(candidate).expect("candidate commit");
        let fixture = Fixture {
            resources: test_resources(&capability),
            resource_loader: test_resource_loader(&capability),
            tool_runtime,
            capability,
            _dir: dir,
        };

        for tool_deadline_policy in [
            crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
                std::time::Duration::ZERO,
                None,
            ),
            crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
                std::time::Duration::from_secs(1),
                Some(std::time::Duration::ZERO),
            ),
        ] {
            assert!(matches!(
                ConversationRuntime::new(fixture.config(tool_deadline_policy)),
                Err(ConversationRuntimeError::InvalidToolDeadlinePolicy)
            ));
        }

        let runtime = ConversationRuntime::new(
            fixture.config(crate::tools::deadline::ToolExecutionDeadlinePolicy::default()),
        )
        .expect("valid construction can reuse both untouched ownership planes");
        runtime.activate();
        runtime
            .shutdown()
            .await
            .expect("valid runtime admits no work and shuts down cleanly");
    }

    /// Builds a conversation runtime over an existing artifacts directory
    /// (whose `conversation.sqlite` may already be populated), returning the
    /// construction result so recovery-gate tests can assert the typed error.
    async fn runtime_with_model_at(
        dir: &tempfile::TempDir,
        conversation_id: &str,
        initial_messages: Vec<MessageBlock>,
        scripts: Vec<Vec<FakeStep>>,
    ) -> Result<(ConversationRuntime, Arc<FakeModel>), ConversationRuntimeError> {
        runtime_with_model_probe_at(dir, conversation_id, initial_messages, scripts, None).await
    }

    /// The probe-armed variant of [`runtime_with_model_at`] (Issue #12, M9b
    /// deterministic race tests).
    async fn runtime_with_model_probe_at(
        dir: &tempfile::TempDir,
        conversation_id: &str,
        initial_messages: Vec<MessageBlock>,
        scripts: Vec<Vec<FakeStep>>,
        probe: Option<CoordinatorProbe>,
    ) -> Result<(ConversationRuntime, Arc<FakeModel>), ConversationRuntimeError> {
        let conversation_id = ConversationId::new(conversation_id);
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let artifacts = dir.path().join("artifacts");
        std::fs::create_dir_all(&artifacts).expect("artifacts");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(&workspace, &artifacts)
                .with_extensions(crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                )),
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
                extension_tools: tool_runtime.extension_tool_plane(),
                agent_activation: fixture_activation(&tool_runtime),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
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
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter),
            approval_mode: ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_engine: Some(AgentStatusEngine::default()),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator,
            clock: None,
            initial_messages,
            subagents: None,
            workflow_output: None,
        };
        match probe {
            Some(probe) => ConversationRuntime::with_probe(config, probe),
            None => ConversationRuntime::new(config),
        }
        .map(|runtime| (runtime, model))
    }

    /// The conversation-runtime-only variant of [`runtime_with_model_at`].
    async fn runtime_at(
        dir: &tempfile::TempDir,
        conversation_id: &str,
        initial_messages: Vec<MessageBlock>,
        scripts: Vec<Vec<FakeStep>>,
    ) -> Result<ConversationRuntime, ConversationRuntimeError> {
        runtime_with_model_at(dir, conversation_id, initial_messages, scripts)
            .await
            .map(|(runtime, _)| runtime)
    }

    fn fixed_time() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
            .expect("parse")
            .with_timezone(&chrono::Utc)
    }

    fn assistant_tool_block(id: &str, call: &str) -> MessageBlock {
        MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
            id: crate::runtime::identity::MessageId::new(id),
            content: vec![crate::message::types::AssistantContentBlock::ToolCall(
                crate::tools::types::ToolCall {
                    id: crate::runtime::identity::ToolCallId::new(call),
                    tool_id: crate::runtime::identity::ToolId::new("tool-a"),
                    name: "alpha".to_owned(),
                    arguments: serde_json::json!({}),
                },
            )],
        })
    }

    fn tool_result_block(call: &str) -> MessageBlock {
        MessageBlock::Tool(crate::message::types::ToolMessageBlock {
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                crate::runtime::identity::MessageId::new("assistant-tool"),
                crate::message::types::ContentBlockIndex::new(0),
            ),
            id: crate::runtime::identity::MessageId::new(format!("tool-{call}")),
            tool_call_id: crate::runtime::identity::ToolCallId::new(call),
            tool_id: crate::runtime::identity::ToolId::new("tool-a"),
            result: crate::tools::types::ToolExecutionResult {
                status: crate::tools::types::ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 1,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        })
    }

    fn seed_user(id: &str, text: &str) -> MessageBlock {
        MessageBlock::User(crate::message::types::UserMessageBlock {
            id: crate::runtime::identity::MessageId::new(id),
            content: text_content(text),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(fixed_time()),
        })
    }

    /// Issue #12 (M9a): an incomplete Assistant tool-call durable tail is
    /// **repaired**, not refused.
    ///
    /// The M8 gate returned `RecoveryRequired` here, which left the
    /// conversation permanently unusable. M9a supersedes it: the missing
    /// canonical sibling is committed from durable evidence — this call has
    /// no `ToolExecutionStarted` fact at all, so it provably never ran and is
    /// recorded as parent-cancelled rather than as an unknown outcome — and
    /// the recovered pending inbound keeps its exact durable identity.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn incomplete_tool_tail_is_repaired_and_pending_remains_intact() {
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_6536aaf7-ead1-793b-883d-d1105c35ac07");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let artifacts = dir.path().join("artifacts");
        std::fs::create_dir_all(&artifacts).expect("artifacts");
        let store_path = artifacts.join("conversation.sqlite");
        let seed = vec![seed_user("msg-u0", "start")];
        {
            let store =
                crate::durable::SqliteConversationStore::open(conversation_id.clone(), &store_path)
                    .expect("open");
            store.initialize(&seed).expect("seed");
            store
                .append_canonical(&assistant_tool_block("assistant-tool", "call-1"))
                .expect("assistant tool call");
            store
                .accept_inbound(crate::durable::inbox::InboundDraft {
                    message_id: None,
                    source: UserSource::Human,
                    kind: InboundKind::Message,
                    content: text_content("pending"),
                    timestamp: fixed_time(),
                    correlation: None,
                })
                .expect("accept pending");
        }

        let runtime = runtime_at(
            &dir,
            "conv_6536aaf7-ead1-793b-883d-d1105c35ac07",
            seed,
            vec![one_turn_script()],
        )
        .await
        .expect("M9a repairs the incomplete tool turn instead of refusing it");
        // The classification is Class D: no attempt ever entered durable
        // authority in this fixture, so nothing is terminalized — only the
        // structure is repaired.
        assert_eq!(
            runtime.recovery().attempt_class(),
            &crate::runtime::recovery::AttemptRecoveryClass::NotStarted
        );
        assert_eq!(
            runtime.recovery().reconciliation().repaired_tool_results,
            vec![ToolCallId::new("call-1")],
            "the missing canonical sibling was committed"
        );
        assert_eq!(runtime.recovery().pending_inbound(), 1);

        // The repaired turn is canonical and structurally complete, and the
        // recovered pending inbound kept its exact durable identity.
        let active = runtime
            .coordinator_active_ids()
            .expect("the coordinator owns the recovered state while idle");
        assert!(
            active
                .iter()
                .any(|id| id.as_str() == "assistant-tool-recovered-tool-call-1"),
            "the recovery-generated ToolResult is active: {active:?}"
        );
        let ledger = runtime.coordinator_ledger().expect("ledger");
        let repaired = ledger
            .iter()
            .find_map(|block| match block {
                MessageBlock::Tool(tool) if tool.tool_call_id == ToolCallId::new("call-1") => {
                    Some(tool.clone())
                }
                _ => None,
            })
            .expect("the repaired sibling is a canonical Ledger fact");
        assert_eq!(
            repaired.result.status,
            crate::tools::types::ToolExecutionStatus::Cancelled {
                reason: CancellationReason::ParentCancelled,
                phase: crate::tools::types::ToolCancellationPhase::BeforeStart,
            },
            "a call with no durable start evidence never ran; it is not reported as unknown"
        );
        assert!(
            repaired.result.content.is_empty(),
            "recovery never invents a result body"
        );

        let reopened = crate::durable::SqliteConversationStore::open(conversation_id, &store_path)
            .expect("reopen");
        let pending = reopened.load_pending().expect("load pending");
        assert_eq!(pending.len(), 1, "recovered pending stays intact");
        assert_eq!(pending[0].sequence.get(), 1);
        assert_eq!(
            pending[0].message_id.as_str(),
            "conv_6536aaf7-ead1-793b-883d-d1105c35ac07-inbound-1"
        );
    }

    /// Issue #63 (Blocker 1, test B): a structurally complete tool group is
    /// a recovery-safe boundary; the runtime reconstructs and admits normally.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn complete_tool_group_is_recoverable_and_admits_normally() {
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_47e0d303-9630-73a3-8913-fea76e160536");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let artifacts = dir.path().join("artifacts");
        std::fs::create_dir_all(&artifacts).expect("artifacts");
        let store_path = artifacts.join("conversation.sqlite");
        let seed = vec![seed_user("msg-u0", "start")];
        {
            let store =
                crate::durable::SqliteConversationStore::open(conversation_id.clone(), &store_path)
                    .expect("open");
            store.initialize(&seed).expect("seed");
            store
                .append_canonical(&assistant_tool_block("assistant-tool", "call-1"))
                .expect("assistant tool call");
            store
                .append_canonical(&tool_result_block("call-1"))
                .expect("tool result");
        }

        let runtime = runtime_at(
            &dir,
            "conv_47e0d303-9630-73a3-8913-fea76e160536",
            seed,
            vec![one_turn_script()],
        )
        .await
        .expect("a complete tool group is recovery-safe");
        runtime.activate();

        // Normal admission is allowed and the attempt settles.
        let admission = runtime
            .submit_inbound(text_content("hello"))
            .expect("accepted after recovery");
        runtime.settlement_signal().notified().await;
        let ledger = runtime.coordinator_ledger().expect("settled");
        assert!(
            ledger
                .iter()
                .any(|m| matches!(m, MessageBlock::User(user) if user.id == admission.message_id)),
            "the recovered runtime admitted the inbound exactly once"
        );
    }

    /// M8 regression: durable compaction history reopens as one exact Surface
    /// state instead of using the obsolete Ledger-only recovery gate.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn durable_compaction_surface_reopens_as_one_state() {
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_b0652278-531a-725d-8621-2ecfe4f19ad7");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let artifacts = dir.path().join("artifacts");
        std::fs::create_dir_all(&artifacts).expect("artifacts");
        let store_path = artifacts.join("conversation.sqlite");
        let seed = vec![seed_user("msg-a", "A"), seed_user("msg-b", "B")];
        {
            let store =
                crate::durable::SqliteConversationStore::open(conversation_id.clone(), &store_path)
                    .expect("open");
            store.initialize(&seed).expect("seed");
            // A summary is durable only through the complete compaction
            // transition: Ledger row, Surface Replace, checkpoint metadata,
            // and completion fact share one transaction.
            let summary = crate::message::types::UserMessageBlock {
                id: crate::runtime::identity::MessageId::new(
                    "conv_b0652278-531a-725d-8621-2ecfe4f19ad7-summary-1",
                ),
                content: text_content("earlier context"),
                source: UserSource::Runtime,
                kind: InboundKind::CompactionSummary(CompactionSummaryMetadata::empty()),
                timestamp: None,
            };
            store
                .commit_compaction(CompactionCommitInput {
                    summary,
                    span: SurfaceSpan::new(
                        crate::runtime::identity::MessageId::new("msg-a"),
                        crate::runtime::identity::MessageId::new("msg-a"),
                    ),
                    expected_revision: store.load_head().expect("head").revision,
                    tokens_before: TokenMeasurement {
                        input_tokens: 20,
                        source: TokenMeasurementSource::Estimated,
                    },
                    estimated_tokens_after: 10,
                    attempt_id: None,
                    turn_id: None,
                    timestamp: fixed_time(),
                })
                .expect("atomic compaction summary");
        }

        let runtime = runtime_at(
            &dir,
            "conv_b0652278-531a-725d-8621-2ecfe4f19ad7",
            seed,
            vec![one_turn_script()],
        )
        .await
        .expect("durable summary history reopens");
        let store = crate::durable::SqliteConversationStore::open(conversation_id, &store_path)
            .expect("reopen store");
        let head = store.load_head().expect("head");
        assert_eq!(
            store
                .reconstruct_surface(head.revision)
                .expect("current Surface"),
            head.active_message_ids
        );
        assert_eq!(
            runtime.coordinator_active_ids().expect("coordinator head"),
            head.active_message_ids
        );
    }

    // -----------------------------------------------------------------
    // Issue #12 (M9a) — the runtime half of durable startup recovery
    //
    // The durable-evidence and classification regressions live in
    // `tests/issue12_recovery.rs`; these are the ones that need a real
    // admission and a driven model turn. The crash boundary is the same
    // everywhere: an exact committed durable prefix written through a store
    // handle that is then dropped, followed by a fresh runtime over the same
    // database.
    // -----------------------------------------------------------------

    /// Seeds an exact durable crash prefix at the path `runtime_at` uses, and
    /// returns the durable database path so a test can reopen it afterwards.
    fn seed_crash_prefix(
        dir: &tempfile::TempDir,
        conversation_id: &str,
        seed: &[MessageBlock],
        commit: impl FnOnce(&crate::durable::SqliteConversationStore, &ConversationId),
    ) -> std::path::PathBuf {
        let conversation_id = ConversationId::new(conversation_id);
        std::fs::create_dir_all(dir.path().join("workspace")).expect("workspace");
        let artifacts = dir.path().join("artifacts");
        std::fs::create_dir_all(&artifacts).expect("artifacts");
        let store_path = artifacts.join("conversation.sqlite");
        {
            let store =
                crate::durable::SqliteConversationStore::open(conversation_id.clone(), &store_path)
                    .expect("open");
            store.initialize(seed).expect("seed");
            commit(&store, &conversation_id);
        }
        store_path
    }

    fn attempt_event(
        conversation_id: &ConversationId,
        event_id: &str,
        attempt_id: &AttemptId,
        event: crate::events::types::RuntimeEvent,
    ) -> crate::events::types::RuntimeEventEnvelope {
        crate::events::types::RuntimeEventEnvelope {
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::identity::EventId::new(event_id),
            sequence: 0,
            conversation_id: conversation_id.clone(),
            attempt_id: Some(attempt_id.clone()),
            turn_id: None,
            timestamp: fixed_time(),
            event,
        }
    }

    fn append_completed_request(
        store: &crate::durable::SqliteConversationStore,
        conversation_id: &ConversationId,
        attempt_id: &AttemptId,
        request_id: &crate::runtime::identity::RequestId,
    ) {
        let mut event = attempt_event(
            conversation_id,
            "request-completed",
            attempt_id,
            crate::events::types::RuntimeEvent::ModelRequestCompleted {
                request_id: request_id.clone(),
                finish_reason: crate::model::finish::ModelFinishReason::Stop,
                usage: None,
                generation: None,
            },
        );
        event.turn_id = Some(TurnId::new("0"));
        store.append_event(event).expect("request completed");
    }

    /// Issue #12 (M9a), Test A (runtime half): an idle runtime recovered with
    /// pending inbound admits it by itself.
    ///
    /// No Runtime Client is constructed, no attachment exists, and no client
    /// request is made: activation alone is enough, because the durable
    /// Pending Inbound Inbox *is* the queue of accepted-but-unadopted work.
    /// There is deliberately no separate "recovery queue".
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn recovered_pending_inbound_is_auto_admitted_with_zero_clients() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut accepted_id = None;
        let store_path = seed_crash_prefix(
            &dir,
            "conv_c68da6fc-963b-77c0-887f-620f3cbbc2cc",
            &[],
            |store, _| {
                let accepted = store
                    .accept_inbound(crate::durable::inbox::InboundDraft {
                        message_id: None,
                        source: UserSource::Human,
                        kind: InboundKind::Message,
                        content: text_content("recovered work"),
                        timestamp: fixed_time(),
                        correlation: None,
                    })
                    .expect("accept");
                accepted_id = Some(accepted.message_id);
            },
        );
        let accepted_id = accepted_id.expect("accepted");

        let (runtime, model) = runtime_with_model_at(
            &dir,
            "conv_c68da6fc-963b-77c0-887f-620f3cbbc2cc",
            Vec::new(),
            vec![one_turn_script()],
        )
        .await
        .expect("runtime recovers");
        assert_eq!(runtime.recovery().pending_inbound(), 1);
        assert_eq!(
            runtime.recovery().resume(),
            crate::runtime::recovery::ResumeDisposition::PendingInboundOnly
        );

        // Activation is the only trigger. Nothing submits, nothing attaches.
        runtime.activate();
        runtime.settlement_signal().notified().await;

        let ledger = runtime.coordinator_ledger().expect("settled");
        assert_eq!(
            ledger
                .iter()
                .filter(|block| crate::conversation::message_id_of(block) == accepted_id)
                .count(),
            1,
            "the recovered pending item is adopted exactly once"
        );
        assert_eq!(model.requests().len(), 1, "exactly one model turn ran");
        let store = crate::durable::SqliteConversationStore::open(
            ConversationId::new("conv_c68da6fc-963b-77c0-887f-620f3cbbc2cc"),
            &store_path,
        )
        .expect("reopen");
        assert!(
            store.load_pending().expect("pending").is_empty(),
            "the adopted item is no longer pending"
        );
    }

    /// Issue #12 (M9a), recovery Class B: an attempt that crashed before any
    /// external start commit lets the already-canonical turn continue.
    ///
    /// The continuation runs as a **new** attempt over the existing canonical
    /// history: the `UserMessage` is neither re-adopted nor duplicated, and
    /// the model sees exactly the recovered turn.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn class_b_restart_continues_the_adopted_turn_without_duplicating_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let dead_attempt = AttemptId::for_conversation(
            &ConversationId::new("conv_4b829d29-175d-72f9-8648-175183dc40d2"),
            0,
        );
        let dead = dead_attempt.clone();
        let mut adopted_id = None;
        seed_crash_prefix(
            &dir,
            "conv_4b829d29-175d-72f9-8648-175183dc40d2",
            &[],
            |store, conversation_id| {
                let accepted = store
                    .accept_inbound(crate::durable::inbox::InboundDraft {
                        message_id: None,
                        source: UserSource::Human,
                        kind: InboundKind::Message,
                        content: text_content("answer me"),
                        timestamp: fixed_time(),
                        correlation: None,
                    })
                    .expect("accept");
                adopt_accepted(store, &accepted);
                adopted_id = Some(accepted.message_id);
                store
                    .append_event(attempt_event(
                        conversation_id,
                        "attempt-started",
                        &dead,
                        crate::events::types::RuntimeEvent::AttemptStarted {
                            attempt_id: dead.clone(),
                        },
                    ))
                    .expect("attempt started");
                // CRASH: no request start, no tool start.
            },
        );
        let adopted_id = adopted_id.expect("adopted");

        let (runtime, model) = runtime_with_model_at(
            &dir,
            "conv_4b829d29-175d-72f9-8648-175183dc40d2",
            Vec::new(),
            vec![one_turn_script()],
        )
        .await
        .expect("runtime recovers");
        assert_eq!(
            runtime.recovery().attempt_class(),
            &crate::runtime::recovery::AttemptRecoveryClass::AdmittedWithoutExternalStart {
                attempt_id: dead_attempt.clone(),
            }
        );
        assert_eq!(
            runtime.recovery().resume(),
            crate::runtime::recovery::ResumeDisposition::ContinueAdoptedTurn { goal: None }
        );

        runtime.activate();
        runtime.settlement_signal().notified().await;

        let ledger = runtime.coordinator_ledger().expect("settled");
        assert_eq!(
            ledger
                .iter()
                .filter(|block| crate::conversation::message_id_of(block) == adopted_id)
                .count(),
            1,
            "the adopted turn is never duplicated by the continuation"
        );
        assert_eq!(
            model.requests().len(),
            1,
            "the continuation runs exactly one model turn"
        );
        assert!(
            model.requests()[0]
                .messages
                .iter()
                .any(|block| block.canonical_id() == Some(&adopted_id)),
            "the continuation carries the recovered canonical turn"
        );
        // The continuation is a genuinely new attempt, above every durable
        // ordinal.
        assert_eq!(
            runtime.recovery().next_attempt_ordinal(),
            1,
            "the allocator starts past the interrupted attempt"
        );
    }

    /// Issue #12 (M9b), cancellation wins before start — through the real
    /// runtime control path. The attempt is parked immediately before the
    /// model-turn start arbitration; `cancel_current_attempt` fully
    /// completes while the execution is parked, so cancellation provably
    /// linearized first: no provider request, no `ModelRequestStarted`, no
    /// Request Snapshot, and no request-scoped context commit.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancellation_before_model_turn_start_never_starts_the_request() {
        use crate::agent::execution::test_sync::StartBoundaryPause;
        let dir = tempfile::tempdir().expect("temp dir");
        let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
        let mut pre_start = pre_start.expect("pre-start phase installed");
        let (runtime, model) = headless_runtime(
            &dir,
            vec![one_turn_script()],
            None,
            Some(CoordinatorProbe {
                start_boundary_pause: Some(pause),
                ..CoordinatorProbe::default()
            }),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();
        runtime
            .submit_inbound(text_content("answer me"))
            .expect("accepted");

        // The attempt is parked immediately before the cancellation-vs-start
        // arbitration: preparation completed, nothing request-scoped
        // committed, no provider request exists.
        pre_start.await_park(1).await;
        let attempt = AttemptId::for_conversation(
            &ConversationId::new("conv_d9e0ddde-2c51-755b-8a4e-1932c402870e"),
            0,
        );
        runtime
            .cancel_current_attempt(&attempt)
            .expect("the parked attempt is cancellable");
        pre_start.release();

        let observations = await_observation(pending.as_ref(), |o| {
            matches!(o, ConversationObservation::Event { event, .. } if is_terminal_event(event))
        })
        .await;
        let terminal_count = observations
            .iter()
            .filter(|o| {
                matches!(o, ConversationObservation::Event { event, .. } if is_terminal_event(event))
            })
            .count();
        assert_eq!(terminal_count, 1, "the attempt settles exactly once");
        assert!(
            observations.iter().any(|o| matches!(
                o,
                ConversationObservation::Event { event, .. }
                    if matches!(event, crate::events::types::RuntimeEvent::AttemptCancelled { .. })
            )),
            "the attempt settles cancelled"
        );

        let store = runtime.tool_runtime().durable_store();
        assert!(
            model.requests().is_empty(),
            "the provider request never started"
        );
        assert!(
            store
                .read_request_snapshots(None, 32)
                .expect("snapshots")
                .snapshots
                .is_empty(),
            "no started Request Snapshot exists"
        );
        let journal = store.read_events(None, 128).expect("journal").events;
        assert!(
            !journal.iter().any(|envelope| matches!(
                envelope.event,
                crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
            )),
            "no request-start fact exists"
        );
        assert!(
            matches!(
                journal.last().map(|envelope| &envelope.event),
                Some(crate::events::types::RuntimeEvent::AttemptCancelled { .. })
            ),
            "the cancellation terminal is unique and last"
        );
        let canonical = store.load_canonical().expect("canonical");
        assert_eq!(
            canonical.len(),
            1,
            "only the adopted inbound is canonical: no request-scoped context half-commit"
        );
        assert!(
            matches!(&canonical[0], MessageBlock::User(user) if user.kind == InboundKind::Message),
            "the one canonical message is the user's inbound"
        );
    }

    /// M9c: runtime drain uses the same M9b start gate as explicit attempt
    /// cancellation. Drain wins while the turn is parked before arbitration,
    /// so the attempt settles with the runtime-owned cause and no provider or
    /// durable request-start fact exists.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn runtime_shutdown_before_model_turn_start_never_starts_the_request() {
        use crate::agent::execution::test_sync::StartBoundaryPause;

        let dir = tempfile::tempdir().expect("temp dir");
        let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
        let mut pre_start = pre_start.expect("pre-start phase installed");
        let drain_linearization = Arc::new(tokio::sync::Notify::new());
        let (runtime, model) = headless_runtime(
            &dir,
            vec![one_turn_script()],
            None,
            Some(CoordinatorProbe {
                drain_linearization: Some(drain_linearization.clone()),
                start_boundary_pause: Some(pause),
                ..CoordinatorProbe::default()
            }),
        )
        .await;
        runtime.activate();
        runtime
            .submit_inbound(text_content("shutdown before start"))
            .expect("accepted");
        pre_start.await_park(1).await;

        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let shutdown_runtime = runtime.clone();
        tokio::spawn(async move {
            let _ = done_tx.send(shutdown_runtime.shutdown().await);
        });
        drain_linearization.notified().await;
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Draining
        );
        assert!(
            matches!(
                done_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "drain waits for the parked attempt rather than treating cancellation as settlement"
        );

        pre_start.release();
        done_rx
            .await
            .expect("shutdown result channel")
            .expect("runtime reaches quiescence");
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Quiescent
        );
        assert!(model.requests().is_empty(), "provider request never starts");

        let store = runtime.tool_runtime().durable_store();
        assert!(
            store
                .read_request_snapshots(None, 32)
                .expect("snapshots")
                .snapshots
                .is_empty(),
            "runtime cancellation before arbitration creates no request snapshot"
        );
        let journal = store.read_events(None, 128).expect("journal").events;
        assert!(
            !journal.iter().any(|envelope| matches!(
                envelope.event,
                crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
            )),
            "runtime cancellation before arbitration creates no start fact"
        );
        assert!(matches!(
            journal.last().map(|envelope| &envelope.event),
            Some(crate::events::types::RuntimeEvent::AttemptCancelled {
                reason: CancellationReason::RuntimeShutdown,
                ..
            })
        ));
    }

    /// Issue #12 (M9b), start wins, then cancellation: the durable start
    /// commit linearizes first, the provider request is in flight, and the
    /// later cancellation settles exactly that started request — it can
    /// never be reclassified as never-started.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn started_request_then_cancellation_settles_the_in_flight_request() {
        let dir = tempfile::tempdir().expect("temp dir");
        let script = vec![
            FakeStep::Emit(crate::model::event::ModelEvent::Started),
            FakeStep::Emit(crate::model::event::ModelEvent::TextDelta {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                text: "partial".to_owned(),
            }),
            FakeStep::ParkUntilCancelled,
        ];
        let (runtime, model) = headless_runtime(&dir, vec![script], None, None).await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();
        runtime
            .submit_inbound(text_content("answer me"))
            .expect("accepted");

        // The provider request is in flight and parked. The durable start
        // commit therefore already won — the provider is only invoked after
        // a successful start commit.
        let mut parked = model.parked();
        parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("parked channel stays open");
        let store = runtime.tool_runtime().durable_store();
        assert_eq!(
            store
                .read_request_snapshots(None, 32)
                .expect("snapshots")
                .snapshots
                .len(),
            1,
            "exactly one started Request Snapshot exists before cancellation"
        );
        assert_eq!(
            store
                .read_events(None, 128)
                .expect("journal")
                .events
                .iter()
                .filter(|envelope| matches!(
                    envelope.event,
                    crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
                ))
                .count(),
            1,
            "exactly one request-start fact exists before cancellation"
        );

        let attempt = AttemptId::for_conversation(
            &ConversationId::new("conv_d9e0ddde-2c51-755b-8a4e-1932c402870e"),
            0,
        );
        runtime
            .cancel_current_attempt(&attempt)
            .expect("the in-flight attempt is cancellable");

        let observations = await_observation(pending.as_ref(), |o| {
            matches!(o, ConversationObservation::Event { event, .. } if is_terminal_event(event))
        })
        .await;
        assert!(
            observations.iter().any(|o| matches!(
                o,
                ConversationObservation::Event { event, .. }
                    if matches!(event, crate::events::types::RuntimeEvent::AttemptCancelled { .. })
            )),
            "the in-flight request settles the attempt as cancelled"
        );
        assert_eq!(
            model.requests().len(),
            1,
            "no second provider request exists"
        );
        // The terminal event is unique and last; the in-flight request it
        // settles durably started, and the partial streamed content never
        // became a canonical Assistant message.
        let journal = store.read_events(None, 128).expect("journal").events;
        assert!(
            matches!(
                journal.last().map(|envelope| &envelope.event),
                Some(crate::events::types::RuntimeEvent::AttemptCancelled { .. })
            ),
            "the cancellation terminal is unique and last"
        );
        let canonical = store.load_canonical().expect("canonical");
        assert!(
            !canonical
                .iter()
                .any(|message| matches!(message, MessageBlock::Assistant(_))),
            "the cancelled in-flight request commits no assistant message"
        );
    }

    /// M9c: a provider request that crossed the durable start boundary keeps
    /// runtime drain pending until the native request future settles. The
    /// release handle is then exercised again after quiescence to prove that
    /// a stale callback cannot create a late semantic effect.
    ///
    /// # Why the live turn is held at an exact barrier
    ///
    /// `begin_drain` requests cancellation of the current attempt inside the
    /// coordinator critical section, and the Agent Loop's provider
    /// arbitration is `stream.next()` first, *attempt cancellation second*.
    /// A provider stream that is merely pending therefore lets the attempt
    /// settle as cancelled at any moment after that request — including
    /// before this test task is polled again. Reading `Draining` or an empty
    /// shutdown channel right after `drain_linearization` was consequently
    /// an assertion about an instant nothing ordered, and it observed
    /// `Quiescent` whenever the attempt happened to settle first.
    ///
    /// [`ModelArbitrationPause`] holds the attempt **inside** its stream loop
    /// — after the two provider items are fully processed and before the next
    /// provider/cancellation arbitration is constructed — so the attempt
    /// provably *cannot* settle while the barrier holds. Every fact this test
    /// reads while it holds is therefore stable rather than raced:
    /// `has_current_attempt()`, the lifecycle state, and the shutdown channel
    /// all describe a runtime that is structurally unable to move. The
    /// `drain_supervision` signal is the stronger claim on top of that: drain
    /// registered its settlement waiter while the slot was still occupied, so
    /// supervision is committed to this exact owner.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn runtime_shutdown_waits_for_started_model_settlement_and_blocks_late_effects() {
        use crate::agent::execution::test_sync::ModelArbitrationPause;

        let dir = tempfile::tempdir().expect("temp dir");
        let (release_tx, release_rx) = crate::scripted_suites::support::fake::model_release();
        let script = vec![
            FakeStep::Emit(crate::model::event::ModelEvent::Started),
            FakeStep::Emit(crate::model::event::ModelEvent::TextDelta {
                block_index: crate::message::types::ContentBlockIndex::new(0),
                text: "partial".to_owned(),
            }),
            FakeStep::ParkUntilReleased(release_rx),
            FakeStep::Emit(crate::model::event::ModelEvent::Failed {
                error: crate::model::error::ModelError {
                    kind: crate::model::error::ModelErrorKind::Cancelled,
                    message: "provider settled cancellation".to_owned(),
                    retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                    retry_after_ms: None,
                    provider_code: None,
                    context_overflow: None,
                    malformed_tool_proposal: None,
                    timeout_phase: None,
                    generation: None,
                },
            }),
        ];
        let drain_linearization = Arc::new(tokio::sync::Notify::new());
        let drain_supervision = Arc::new(tokio::sync::Notify::new());
        // Armed after the two scripted provider items — `Started` and the
        // text delta — so the attempt parks with a durably started request it
        // still owns, and before the arbitration that would let the drain's
        // cancellation settle it.
        let (model_pause, mut model_pause_reached, model_pause_release) =
            ModelArbitrationPause::install(2);
        let (runtime, model) = headless_runtime(
            &dir,
            vec![script],
            None,
            Some(CoordinatorProbe {
                drain_linearization: Some(drain_linearization.clone()),
                drain_supervision: Some(drain_supervision.clone()),
                model_arbitration_pause: Some(model_pause),
                ..CoordinatorProbe::default()
            }),
        )
        .await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();
        runtime
            .submit_inbound(text_content("park the provider"))
            .expect("accepted");

        // The provider stream is open and owned by the attempt, and the
        // attempt is held at the exact arbitration barrier: it cannot settle
        // until this test releases it, whatever cancellation is requested
        // meanwhile.
        model_pause_reached
            .wait_for(|is_reached| *is_reached)
            .await
            .expect("model arbitration pause channel stays open");
        let store = runtime.tool_runtime().durable_store();
        assert_eq!(
            model.requests().len(),
            1,
            "provider crossed the start boundary"
        );
        assert_eq!(
            store
                .read_request_snapshots(None, 32)
                .expect("snapshots")
                .snapshots
                .len(),
            1,
            "the durable request-start boundary is committed"
        );

        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let shutdown_runtime = runtime.clone();
        tokio::spawn(async move {
            let result = shutdown_runtime.shutdown().await;
            let _ = done_tx.send(result);
        });
        within_liveness_guard("the drain linearization", drain_linearization.notified()).await;
        // Drain then parks on this exact owner: the waiter is registered and
        // the current-attempt slot is still occupied. A drain that had
        // short-circuited could never reach this signal.
        within_liveness_guard(
            "drain to park on the started provider turn",
            drain_supervision.notified(),
        )
        .await;
        assert!(
            runtime.has_current_attempt(),
            "the started provider turn is still runtime-owned while drain supervises it"
        );
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Draining,
            "drain linearized before the provider was released"
        );
        assert!(
            matches!(
                done_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "cancellation request is not provider settlement"
        );

        // The provider emulator deliberately ignores cancellation until this
        // explicit release. This is the physical settlement proof: releasing
        // it first, and the arbitration barrier only afterwards, means the
        // attempt's next arbitration observes the provider's own terminal
        // event rather than a stream that is merely pending.
        release_tx.send_replace(true);
        let _ = model_pause_release.send(());
        done_rx
            .await
            .expect("shutdown result channel")
            .expect("runtime reaches quiescence");
        assert_eq!(
            runtime.lifecycle_state(),
            ConversationLifecycleState::Quiescent
        );
        assert!(matches!(
            store
                .read_events(None, 256)
                .expect("terminal events")
                .events
                .last()
                .map(|envelope| &envelope.event),
            Some(crate::events::types::RuntimeEvent::AttemptCancelled {
                reason: CancellationReason::RuntimeShutdown,
                ..
            })
        ));
        let requests_before = model.requests().len();
        let events_before = store.read_events(None, 256).expect("events").events;
        let canonical_before = store.load_canonical().expect("canonical");
        let head_before = store.load_head().expect("head");
        let pending_before = store.load_pending().expect("pending inbound");
        let revision_before = runtime.capability().current_snapshot().revision();
        let background_before = runtime.tool_runtime().background().active_snapshot().len();

        // The stale callback source must be proven *gone*, not merely
        // observed to do nothing: every model invocation stream owner has
        // left, so no task remains that could still read this watch channel.
        within_liveness_guard(
            "every model stream owner to exit",
            model
                .streams_exited()
                .wait_for(|exited| *exited >= requests_before as u64),
        )
        .await
        .expect("stream-exit channel stays open");

        // A stale release/callback handle retained by the old owner is now a
        // no-op: it cannot start another provider request or append a fact.
        release_tx.send_replace(false);
        release_tx.send_replace(true);
        // Give the runtime's executor a real scheduling opportunity: any
        // task that *could* still act would run before this join returns.
        tokio::task::yield_now().await;
        tokio::spawn(async {})
            .await
            .expect("a scheduling opportunity for any surviving owner");

        // A stale admission callback handle is refused too.
        let inner = runtime
            .weak_inner()
            .upgrade()
            .expect("the test still owns the runtime");
        inner.admit_next_attempt();
        assert!(
            !runtime.has_current_attempt(),
            "attempt admission is closed"
        );

        assert_eq!(model.requests().len(), requests_before, "provider requests");
        assert_eq!(
            store.read_events(None, 256).expect("events").events,
            events_before,
            "quiescence is a terminal ownership boundary"
        );
        assert_eq!(store.load_canonical().expect("canonical"), canonical_before);
        assert_eq!(store.load_head().expect("head"), head_before);
        assert_eq!(
            store.load_pending().expect("pending inbound"),
            pending_before
        );
        assert_eq!(
            runtime.capability().current_snapshot().revision(),
            revision_before,
            "capability revisions"
        );
        assert_eq!(
            runtime.tool_runtime().background().active_snapshot().len(),
            background_before,
            "background ownership"
        );
        assert!(matches!(
            runtime.submit_inbound(text_content("post-quiescence")),
            Err(InboundAdmissionError::Shutdown)
        ));
        assert_eq!(
            store.load_pending().expect("pending inbound"),
            pending_before,
            "the refused acceptance consumed no sequence"
        );
    }

    /// Issue #12 (M9b), recovery Class B reaching the same gate: the
    /// recovered pending continuation drives the attempt to the identical
    /// model-turn start arbitration. Cancelling there is again
    /// cancellation-before-start — the same gate, the same adjudication, the
    /// same never-started outcome.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn class_b_recovery_uses_the_same_model_turn_start_gate() {
        use crate::agent::execution::test_sync::StartBoundaryPause;
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation = ConversationId::new("conv_2af196f9-fdf0-75af-85c5-b012b4f771a9");
        let dead_attempt = AttemptId::for_conversation(&conversation, 0);
        let dead = dead_attempt.clone();
        let store_path = seed_crash_prefix(
            &dir,
            "conv_2af196f9-fdf0-75af-85c5-b012b4f771a9",
            &[],
            |store, conversation_id| {
                let accepted = store
                    .accept_inbound(crate::durable::inbox::InboundDraft {
                        message_id: None,
                        source: UserSource::Human,
                        kind: InboundKind::Message,
                        content: text_content("start the turn"),
                        timestamp: fixed_time(),
                        correlation: None,
                    })
                    .expect("accept");
                adopt_accepted(store, &accepted);
                store
                    .append_event(attempt_event(
                        conversation_id,
                        "attempt-started",
                        &dead,
                        crate::events::types::RuntimeEvent::AttemptStarted {
                            attempt_id: dead.clone(),
                        },
                    ))
                    .expect("attempt started");
                // CRASH: no request start, no tool start.
            },
        );
        let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
        let mut pre_start = pre_start.expect("pre-start phase installed");
        let (runtime, model) = runtime_with_model_probe_at(
            &dir,
            "conv_2af196f9-fdf0-75af-85c5-b012b4f771a9",
            vec![],
            vec![one_turn_script()],
            Some(CoordinatorProbe {
                start_boundary_pause: Some(pause),
                ..CoordinatorProbe::default()
            }),
        )
        .await
        .expect("runtime");
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("bridge");
        runtime.activate();

        // The recovered continuation attempt is parked immediately before the
        // start arbitration — the same boundary as a fresh inbound turn.
        pre_start.await_park(1).await;
        let attempt = AttemptId::for_conversation(&conversation, 1);
        runtime
            .cancel_current_attempt(&attempt)
            .expect("the recovered attempt is cancellable");
        pre_start.release();

        let observations = await_observation(pending.as_ref(), |o| {
            matches!(o, ConversationObservation::Event { event, .. } if is_terminal_event(event))
        })
        .await;
        assert!(
            observations.iter().any(|o| matches!(
                o,
                ConversationObservation::Event { event, .. }
                    if matches!(event, crate::events::types::RuntimeEvent::AttemptCancelled { .. })
            )),
            "the recovered attempt settles cancelled"
        );
        assert!(
            model.requests().is_empty(),
            "the provider request never started"
        );

        let store = crate::durable::SqliteConversationStore::open(conversation, &store_path)
            .expect("reopen");
        assert!(
            store
                .read_request_snapshots(None, 32)
                .expect("snapshots")
                .snapshots
                .is_empty(),
            "no started Request Snapshot exists"
        );
        let journal = store.read_events(None, 128).expect("journal").events;
        assert!(
            !journal.iter().any(|envelope| matches!(
                envelope.event,
                crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
            )),
            "no request-start fact exists"
        );
        let canonical = store.load_canonical().expect("canonical");
        assert_eq!(
            canonical.len(),
            1,
            "only the adopted inbound is canonical: no request-scoped context half-commit"
        );
    }

    /// Issue #12 (M9a), recovery Class C: a restart after a committed
    /// request start starts **nothing**.
    ///
    /// The proof is deterministic rather than a timed absence: a new
    /// user-driven turn is submitted and awaited, and because at most one
    /// attempt runs per conversation, its settlement happens-after any
    /// recovery-initiated attempt would have. Exactly one provider request
    /// exists at that point, so recovery issued none.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn class_c_restart_issues_no_provider_request_of_its_own() {
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation = ConversationId::new("conv_e4c44f69-7140-7c07-8660-a8791da60c71");
        let dead_attempt = AttemptId::for_conversation(&conversation, 0);
        let dead = dead_attempt.clone();
        seed_crash_prefix(
            &dir,
            "conv_e4c44f69-7140-7c07-8660-a8791da60c71",
            &[],
            |store, conversation_id| {
                let accepted = store
                    .accept_inbound(crate::durable::inbox::InboundDraft {
                        message_id: None,
                        source: UserSource::Human,
                        kind: InboundKind::Message,
                        content: text_content("ask the model"),
                        timestamp: fixed_time(),
                        correlation: None,
                    })
                    .expect("accept");
                adopt_accepted(store, &accepted);
                store
                    .append_event(attempt_event(
                        conversation_id,
                        "attempt-started",
                        &dead,
                        crate::events::types::RuntimeEvent::AttemptStarted {
                            attempt_id: dead.clone(),
                        },
                    ))
                    .expect("attempt started");
                let snapshot = crate::model::RequestSnapshot::new(
                    crate::model::RequestIdentity {
                        attempt_id: dead.clone(),
                        turn: crate::runtime::identity::TurnId::new("0"),
                        retry_number: 0,
                    },
                    store.load_head().expect("head").revision,
                    "frozen prompt".to_owned(),
                    Vec::new(),
                    crate::runtime::RuntimeResourceRevision::new(1),
                    crate::model::ModelInvocationConfig {
                        model: "model-before-restart".to_owned(),
                        protocol: crate::model::ModelProtocol::OpenAiChatCompletions,
                        max_output_tokens: 64,
                        request_params: crate::model::RequestParams::new(),
                        capabilities: crate::model::catalog::ModelCapabilities::text_only(
                            true, true,
                        ),
                        compat: crate::model::catalog::ModelCompat::default(),
                    },
                    64_000,
                    None,
                    false,
                    Vec::new(),
                    crate::runtime::identity::CapabilityRevision::new(1),
                    crate::context::ContextGeneration {
                        id: 1,
                        contributors: Vec::new(),
                    },
                    None,
                    Vec::new(),
                );
                store
                    .commit_model_turn_start(&[], &snapshot, fixed_time())
                    .expect("request start");
                // CRASH: the provider may or may not have executed this request.
            },
        );

        let (runtime, model) = runtime_with_model_at(
            &dir,
            "conv_e4c44f69-7140-7c07-8660-a8791da60c71",
            Vec::new(),
            vec![one_turn_script()],
        )
        .await
        .expect("runtime recovers");
        assert!(matches!(
            runtime.recovery().attempt_class(),
            crate::runtime::recovery::AttemptRecoveryClass::IndeterminateExternalOutcome {
                attempt_id,
                model_request: Some(_),
                ..
            } if attempt_id == &dead_attempt
        ));
        assert_eq!(
            runtime.recovery().resume(),
            crate::runtime::recovery::ResumeDisposition::BlockedIndeterminate
        );

        runtime.activate();
        // New user-driven work is still admissible: the ambiguity belongs to
        // the old request, not to the conversation.
        runtime
            .submit_inbound(text_content("a new turn"))
            .expect("accepted after recovery");
        runtime.settlement_signal().notified().await;
        assert_eq!(
            model.requests().len(),
            1,
            "only the user-driven turn reached the provider; recovery resent nothing"
        );
    }

    /// Issue #12 (M9a), recovery Class E: a restart after a **durably
    /// completed** model request — whose canonical Assistant message never
    /// committed — starts nothing either.
    ///
    /// The provider already executed the request; rustX durably observed the
    /// outcome. The proof mirrors Class C: a new user-driven turn is
    /// submitted and awaited, and because at most one attempt runs per
    /// conversation, its settlement happens-after any recovery-initiated
    /// attempt would have. Exactly one provider request exists at that
    /// point, so the recovered runtime itself initiated no replacement
    /// request — and it fabricated no Assistant body.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn class_e_restart_issues_no_replacement_request() {
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation = ConversationId::new("conv_0224cd71-1cda-74e4-8010-dad3ad2ce447");
        let dead_attempt = AttemptId::for_conversation(&conversation, 0);
        let dead = dead_attempt.clone();
        seed_crash_prefix(
            &dir,
            "conv_0224cd71-1cda-74e4-8010-dad3ad2ce447",
            &[],
            |store, conversation_id| {
                let accepted = store
                    .accept_inbound(crate::durable::inbox::InboundDraft {
                        message_id: None,
                        source: UserSource::Human,
                        kind: InboundKind::Message,
                        content: text_content("ask the model"),
                        timestamp: fixed_time(),
                        correlation: None,
                    })
                    .expect("accept");
                adopt_accepted(store, &accepted);
                store
                    .append_event(attempt_event(
                        conversation_id,
                        "attempt-started",
                        &dead,
                        crate::events::types::RuntimeEvent::AttemptStarted {
                            attempt_id: dead.clone(),
                        },
                    ))
                    .expect("attempt started");
                let snapshot = crate::model::RequestSnapshot::new(
                    crate::model::RequestIdentity {
                        attempt_id: dead.clone(),
                        turn: crate::runtime::identity::TurnId::new("0"),
                        retry_number: 0,
                    },
                    store.load_head().expect("head").revision,
                    "frozen prompt".to_owned(),
                    Vec::new(),
                    crate::runtime::RuntimeResourceRevision::new(1),
                    crate::model::ModelInvocationConfig {
                        model: "model-before-restart".to_owned(),
                        protocol: crate::model::ModelProtocol::OpenAiChatCompletions,
                        max_output_tokens: 64,
                        request_params: crate::model::RequestParams::new(),
                        capabilities: crate::model::catalog::ModelCapabilities::text_only(
                            true, true,
                        ),
                        compat: crate::model::catalog::ModelCompat::default(),
                    },
                    64_000,
                    None,
                    false,
                    Vec::new(),
                    crate::runtime::identity::CapabilityRevision::new(1),
                    crate::context::ContextGeneration {
                        id: 1,
                        contributors: Vec::new(),
                    },
                    None,
                    Vec::new(),
                );
                store
                    .commit_model_turn_start(&[], &snapshot, fixed_time())
                    .expect("request start");
                append_completed_request(store, conversation_id, &dead, &snapshot.request_id);
                // CRASH: the provider outcome is durably known, but the
                // canonical Assistant message never committed.
            },
        );

        let (runtime, model) = runtime_with_model_at(
            &dir,
            "conv_0224cd71-1cda-74e4-8010-dad3ad2ce447",
            Vec::new(),
            vec![one_turn_script()],
        )
        .await
        .expect("runtime recovers");
        assert!(matches!(
            runtime.recovery().attempt_class(),
            crate::runtime::recovery::AttemptRecoveryClass::ExternalOutcomeKnown {
                attempt_id,
                model_request: Some(_),
                ..
            } if attempt_id == &dead_attempt
        ));
        assert_eq!(
            runtime.recovery().resume(),
            crate::runtime::recovery::ResumeDisposition::PendingInboundOnly,
            "the answered model turn is never automatically continued"
        );

        runtime.activate();
        // A later user-driven turn proceeds according to the intended runtime
        // semantics — startup itself must not replay the missing Assistant
        // turn.
        runtime
            .submit_inbound(text_content("a new turn"))
            .expect("accepted after recovery");
        runtime.settlement_signal().notified().await;
        assert_eq!(
            model.requests().len(),
            1,
            "exactly the user-driven turn reached the provider; recovery initiated no replacement request"
        );
    }

    /// Issue #12 (M9a), Test L (runtime half): a restart never reuses an
    /// `AttemptId` that already appears in durable history.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_restart_allocates_an_attempt_id_past_durable_history() {
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation = ConversationId::new("conv_d739e286-16f6-783d-89be-a52ec3d0651f");
        let mut durable_attempts = Vec::new();
        for ordinal in 0..3 {
            durable_attempts.push(AttemptId::for_conversation(&conversation, ordinal));
        }
        let seeded = durable_attempts.clone();
        let store_path = seed_crash_prefix(
            &dir,
            "conv_d739e286-16f6-783d-89be-a52ec3d0651f",
            &[],
            |store, id| {
                for (ordinal, attempt) in seeded.iter().enumerate() {
                    store
                        .append_event(attempt_event(
                            id,
                            &format!("started-{ordinal}"),
                            attempt,
                            crate::events::types::RuntimeEvent::AttemptStarted {
                                attempt_id: attempt.clone(),
                            },
                        ))
                        .expect("attempt started");
                    store
                        .append_event(attempt_event(
                            id,
                            &format!("completed-{ordinal}"),
                            attempt,
                            crate::events::types::RuntimeEvent::AttemptCompleted {
                                attempt_id: attempt.clone(),
                                finish_reason: crate::model::finish::ModelFinishReason::Stop,
                            },
                        ))
                        .expect("attempt completed");
                }
            },
        );

        let runtime = runtime_at(
            &dir,
            "conv_d739e286-16f6-783d-89be-a52ec3d0651f",
            Vec::new(),
            vec![one_turn_script()],
        )
        .await
        .expect("runtime recovers");
        assert_eq!(runtime.recovery().next_attempt_ordinal(), 3);
        runtime.activate();
        runtime
            .submit_inbound(text_content("after restart"))
            .expect("accepted");
        runtime.settlement_signal().notified().await;

        // The durable Event Journal is the proof: the new attempt's start
        // fact carries an identity that never appeared before.
        let store =
            crate::durable::SqliteConversationStore::open(conversation.clone(), &store_path)
                .expect("reopen");
        let mut started = Vec::new();
        let mut cursor = None;
        loop {
            let page = store.read_events(cursor, 64).expect("events");
            if page.events.is_empty() {
                break;
            }
            for envelope in &page.events {
                if let crate::events::types::RuntimeEvent::AttemptStarted { attempt_id } =
                    &envelope.event
                {
                    started.push(attempt_id.clone());
                }
            }
            cursor = page.next_sequence;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(started.len(), 4, "one new attempt started: {started:?}");
        let new_attempt = started.last().expect("the new attempt");
        assert!(
            !durable_attempts.contains(new_attempt),
            "the restarted allocator reused a durable identity: {new_attempt}"
        );
        assert_eq!(new_attempt.conversation_ordinal(&conversation), Some(3));
    }
}
