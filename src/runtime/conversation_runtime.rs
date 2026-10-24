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

use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use crate::agent::cancellation::AgentCancellation;
use crate::agent::observer::{AgentExecutionObserver, AgentStatusObservation};
use crate::agent::{AgentExecution, AgentExecutionRequest};
use crate::capabilities::{CapabilityCoordinator, CapabilityObserver, CapabilitySnapshot};
use crate::context::compaction::{
    CompactionAttribution, CompactionExecutionError, ExecutedCompaction,
    execute_compaction_in_operation,
};
use crate::context::tokens::TokenEstimator;
use crate::context::{
    AgentStatusEngine, CompactionConstraints, ContextError, ContextErrorKind, ContextRuntime,
    NativeContextInput, SessionContextPolicy, render_effective_system_prompt,
};
use crate::conversation::ConversationState;
use crate::conversation::SurfaceRevision;
use crate::durable::{
    ConversationStore, ConversationStoreError, InboundDraft, TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT,
    TranscriptPage,
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
use crate::runtime::adoption::{
    AdmittedConfiguration, AdoptionEligibility, AdoptionError, ConfigurationCut,
    PreparedConfiguration, configuration_cache_impact,
};
use crate::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolExecutionId};
use crate::runtime::inbound::{
    ConversationInboundMailbox, FreshInboundTurn, InboundItem, InboundObserver, InboundSequence,
    InitialTurnTrigger, MailboxError,
};
use crate::runtime::interaction::{
    InteractionCoordinator, InteractionObserver, InteractionOutcome, InteractionRef,
    InteractionRoute, RoutedInteractionError, route_error,
};
use crate::runtime::monotonic::{MonotonicClock, SystemMonotonicClock};
use crate::runtime::observation::{
    ConversationObservation, ObservationFanout, PendingObservations,
};
use crate::runtime::request_history::RequestHistory;
use crate::runtime::resources::{
    ContextBinding, PreparedRuntimeResources, RuntimeConfiguration, RuntimeResourceSnapshot,
};
use crate::runtime::types::{
    ApprovalMode, CancellationReason, ConversationLifecycle, ConversationLifecycleState,
    DurabilityGate, DurableOperation, RuntimeClock, SystemClock,
};
use crate::tools::background::{BackgroundExecutionSnapshot, BackgroundObserver};
use crate::tools::runtime::ConversationToolRuntime;
use crate::tools::types::ModelToolDefinition;

mod admission;
mod attempt;
pub mod auxiliary;
mod bootstrap;
mod child;
mod compaction;
mod configuration;
mod controls;
mod drain;
mod durability;
mod history;
mod model_selection;
mod observer;
mod probe;
mod scheduled;
#[cfg(test)]
mod test_support;
mod title;

use admission::{IdleProbe, SemanticInboundClass, WakeGate};
pub(crate) use compaction::ManualCompactionHandle;
use compaction::{CurrentManualCompaction, ManualTail};
use drain::{DrainCompletion, DrainTrigger};
use durability::{
    AdmissionDurabilityCycle, AdmissionRetryBudget, BackgroundFailureSink, SubagentFailureSink,
};

#[cfg(test)]
pub(crate) use admission::IdleBusyReason;
pub use admission::{InboundAdmission, InboundAdmissionError};
pub use attempt::CancelAttemptError;
pub(crate) use attempt::{NativeAttemptOutcome, NativeAttemptSettlement};
pub use bootstrap::RuntimeBootstrapError;
pub(crate) use bootstrap::RuntimeBootstrapSnapshot;
pub(crate) use child::ParentGuidanceSeal;
pub use compaction::{ManualCompactionError, ManualCompactionOutcome};
pub use drain::ShutdownError;
pub(crate) use model_selection::validate_model_context;
pub use model_selection::{ApprovalModeUpdateError, ModelUpdateError};
pub(crate) use observer::RuntimeObserver;
#[cfg(test)]
pub(crate) use probe::{CoordinatorProbe, Gate, runtime_with_recovered_registry_for_test};

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
    configuration: AdmittedConfiguration,
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
    scheduled: Option<scheduled::CurrentScheduled>,
    scheduled_retries: std::collections::BTreeMap<
        crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
        u64,
    >,
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
    /// Latest handoff; child turn permits prevent admission past an unconsumed receipt.
    latest_settlement: Option<NativeAttemptSettlement>,
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

/// The shared state of one conversation runtime.
pub(crate) struct RuntimeInner {
    configuration_owner: Arc<()>,
    model_selection_owner: ModelSelectionOwner,
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
    /// linearize against it. The coordinator requests failure without waiting
    /// for SQL; the gate publishes after any earlier ownership writer exits.
    durability_gate: Arc<DurabilityGate>,
    auxiliary: crate::runtime::auxiliary::executor::AuxiliaryExecutor,
    external_auxiliary: Arc<auxiliary::ExternalAuxiliary>,
    manual_tail: Mutex<Option<ManualTail>>,
    /// Root-local optional work remains owned through its Catalog business tail.
    session_title_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
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
    /// The configuration-adoption eligibility this runtime last published. Its
    /// lock orders every derivation with its publication (taken before the
    /// coordinator lock, never after it).
    published_eligibility: Mutex<Option<AdoptionEligibility>>,
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
    /// Acquires the one admission synchronization boundary.
    fn lock_state(&self) -> MutexGuard<'_, CoordinatorState> {
        self.state
            .lock()
            .expect("conversation runtime lock poisoned")
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

/// Chosen at composition, before either runtime or client authority is exposed.
/// A durable Session can only change selection through its native Session owner.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelSelectionOwner {
    Conversation,
    Session,
}

impl core::fmt::Debug for ConversationRuntime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConversationRuntime")
            .field("conversation_id", &self.inner.conversation_id)
            .finish()
    }
}

impl ConversationRuntime {
    pub(crate) fn schedule_operation(
        &self,
        binding: &crate::scheduler::ScheduleBinding,
        operation: &crate::scheduler::ScheduleOperation,
        clock: &dyn crate::runtime::types::RuntimeClock,
    ) -> Result<crate::scheduler::ScheduleOperationResult, crate::runtime::inbound::MailboxError>
    {
        let state = self.inner.lock_state();
        if let crate::scheduler::ScheduleOperation::WorkflowReconcile { id, after, limit } =
            operation
        {
            return self
                .inner
                .reconcile_scheduled(&state, binding, id, (*after, *limit), clock);
        }
        let prepared;
        let operation = if let crate::scheduler::ScheduleOperation::Command(command) = operation {
            if let crate::scheduler::ScheduleMutation::CreateWorkflow {
                workflow_id,
                arguments,
                ..
            }
            | crate::scheduler::ScheduleMutation::UpdateWorkflow {
                workflow_id,
                arguments,
                ..
            } = &command.mutation
            {
                if let Some(receipt) = self.inner.store.schedule_command_receipt(
                    binding,
                    command,
                    &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
                )? {
                    return Ok(crate::scheduler::ScheduleOperationResult::Command(receipt));
                }
                let native =
                    crate::runtime::workflow::scheduled::ScheduledWorkflowPreparation::new(
                        binding.clone(),
                        state.resources.clone(),
                        state.model.config().clone(),
                    )
                    .and_then(|preparation| {
                        preparation
                            .with_approval_mode(state.effective_approval_mode)
                            .prepare(workflow_id, arguments)
                    });
                let native = match native {
                    Ok(native) => native,
                    Err(error) => {
                        return Ok(crate::scheduler::ScheduleOperationResult::Command(
                            crate::scheduler::ScheduleCommandResult::Invalid {
                                reason: error.to_string(),
                            },
                        ));
                    }
                };
                prepared = crate::scheduler::ScheduleOperation::WorkflowCommand {
                    command: command.clone(),
                    contract: Box::new(native.contract().clone()),
                };
                &prepared
            } else {
                operation
            }
        } else {
            operation
        };
        let result = self.inner.mailbox.schedule_operation(
            self.inner.store.as_ref(),
            binding,
            operation,
            clock,
        )?;
        if let crate::scheduler::ScheduleOperation::WorkflowCancel { id } = operation
            && let Some(current) = state.scheduled.as_ref().filter(|owner| &owner.id == id)
        {
            current
                .cancellation
                .cancel(crate::runtime::types::CancellationReason::UserRequested);
        }
        Ok(result)
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

    /// The complete generation future attempts currently acquire.
    #[must_use]
    pub fn runtime_resources(&self) -> Arc<RuntimeResourceSnapshot> {
        self.inner.lock_state().resources.clone()
    }

    /// The conversation-owned Agent and finite activation authority.
    pub(crate) fn subagent_registry(&self) -> Option<&crate::runtime::subagent::SubagentRegistry> {
        self.inner.subagents.as_ref()
    }

    pub(crate) fn background_registry(
        &self,
    ) -> &crate::tools::background::ConversationBackgroundRegistry {
        self.inner.tool_runtime.background()
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
}
