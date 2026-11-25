//! The conversation runtime coordinator (Issue #61): the semantic owner of
//! conversation coordination for one conversation — session model state,
//! attempt admission, the current-attempt slot, between-attempt current
//! Surface state, durable request-history reads, the shared lifecycle/drain
//! authority, and settlement handoff through quiescence.
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
//! The Runtime Client (see `crate::runtime_client`) is a projection, control,
//! and attachment adapter over this coordinator. It observes runtime
//! observations and forwards control requests; it never owns the semantic
//! state above.
//!
//! # The one admission owner
//!
//! Any producer may publish ordinary inbound work through the conversation
//! inbound boundary (the [`ConversationInboundMailbox`]
//! of the owned tool runtime). Only [`ConversationRuntime`] admits the next
//! [`AgentExecution`]:
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
//! the [`ConversationLifecycle`]
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
//! runtime-owned `ConversationObservation`
//! into the runtime-owned observation fan-out. The Runtime Client projection
//! owns the primary `PendingObservations`
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
//! `ConversationRuntime::install_observation_bridge` is the one runtime
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
    SessionContextPolicy, render_effective_system_prompt,
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
use crate::runtime::configuration_commit::{
    AdmittedConfiguration, ConfigurationCut, PreparedConfiguration, StagedConfiguration,
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

pub(crate) mod activity;
mod admission;
mod attempt;
mod bootstrap;
mod child;
#[cfg(test)]
mod cold_attach_work_tests;
mod compaction;
mod configuration;
mod controls;
mod drain;
mod durability;
mod foreground;
mod history;
#[cfg(test)]
mod issue606_tests;
mod model_selection;
mod observer;
mod probe;
pub(crate) mod scheduled;
#[cfg(test)]
mod test_support;
mod title;

use admission::{SemanticInboundClass, WakeGate};
pub(crate) use compaction::ManualCompactionHandle;
pub(crate) use configuration::ConfigurationHealth;
use drain::{DrainCompletion, DrainTrigger};
use durability::{
    AdmissionDurabilityCycle, AdmissionRetryBudget, BackgroundFailureSink, SubagentFailureSink,
};

#[cfg(test)]
pub(crate) use admission::IdleBusyReason;
pub(crate) use admission::TurnInputAdmission;
pub use admission::{InboundAdmission, InboundAdmissionError};
pub use attempt::CancelAttemptError;
pub(crate) use attempt::{NativeAttemptOutcome, NativeAttemptSettlement};
pub use bootstrap::RuntimeBootstrapError;
pub(crate) use bootstrap::RuntimeBootstrapSnapshot;
pub(crate) use child::ParentGuidanceSeal;
pub use compaction::{ManualCompactionError, ManualCompactionOutcome};
pub use drain::ShutdownError;
pub(crate) use foreground::{
    AttemptTaskPoisonGuard, CurrentAttempt, CurrentManualCompaction, ManualTail, ManualTailState,
    ScheduledCancellation,
};
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
    /// Clock for Agent Status engines materialized from accepted configuration.
    /// This is separate from runtime event timestamps and monotonic deadlines.
    pub status_clock: Arc<dyn crate::context::AgentStatusClock>,
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
    /// The UTC clock for durable operations including inbound submission and
    /// scheduled execution. The system clock is used when omitted.
    pub clock: Option<Arc<dyn RuntimeClock>>,
    /// The canonical conversation history the runtime starts from.
    pub initial_messages: Vec<MessageBlock>,
    /// The conversation-owned subagent registry (Issue #60), when this
    /// runtime may delegate to child runtimes. A subagent child itself is
    /// composed without one, so recursive delegation is absent by
    /// construction.
    pub subagents: Option<crate::runtime::subagent::SubagentRegistry>,
    /// The explicit conversation role (Issue #606): root session, subagent
    /// child, or Workflow `AgentRun` execution. The role is decided at
    /// composition and never inferred from incidental state.
    pub role: ConversationRole,
}

/// The one declared role of a conversation runtime, fixed at composition
/// (Issue #606).
///
/// The role replaces the old `workflow_output.is_some()` /
/// `parent_turn_permits.is_none()` inference: a runtime is exactly one of
/// root, subagent child, or Workflow `AgentRun`, and role-gated machinery
/// (parent guidance seal, turn permits, workspace-change recording,
/// workflow output settlement) follows from this declaration alone.
pub enum ConversationRole {
    /// A user-facing root conversation runtime.
    Root,
    /// A child runtime driven by a parent through parent guidance, turn
    /// permits, terminal seal and physical rollback.
    SubagentChild,
    /// A headless child runtime executing one Workflow-owned `AgentRun`:
    /// it carries the reserved output terminal plus turn
    /// permits/settlement/rollback, but never parent guidance or the
    /// terminal seal.
    WorkflowAgentRun {
        /// The reserved Workflow Agent terminal protocol; `None` for a
        /// schema-free run (`WorkflowChildTerminal::Text`) whose final
        /// reply is itself the result.
        output: Option<Arc<dyn crate::runtime::workflow::WorkflowOutputTerminal>>,
    },
}

impl ConversationRole {
    /// Whether this is a root conversation runtime.
    pub(crate) const fn is_root(&self) -> bool {
        matches!(self, Self::Root)
    }

    /// Whether this is any child runtime (`SubagentChild` or
    /// `WorkflowAgentRun`).
    pub(crate) const fn is_child(&self) -> bool {
        !self.is_root()
    }

    /// The Workflow output terminal owned by a `WorkflowAgentRun` role.
    pub(crate) fn workflow_output(
        &self,
    ) -> Option<Arc<dyn crate::runtime::workflow::WorkflowOutputTerminal>> {
        match self {
            Self::WorkflowAgentRun { output } => output.clone(),
            Self::Root | Self::SubagentChild => None,
        }
    }
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
pub(crate) enum AttemptProvenance {
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

/// The successful observation cut captured before this candidate was prepared.
/// A later observation remains unconsumed if this older candidate is rejected.
pub(crate) struct PendingRuntimeCatalog {
    catalog_revision: u64,
    authority: crate::capabilities::RuntimeRefreshAuthority,
    refresh_sequence: Option<u64>,
    candidate: crate::capabilities::PreparedCapabilityCandidate,
}

pub(crate) struct RuntimeCatalogFailure {
    authority: crate::capabilities::RuntimeRefreshAuthority,
    /// Explicit whole-batch failure survives pure catalog publication.
    refresh_sequence: Option<u64>,
}

/// Foreground execution ownership and admission bookkeeping.
///
/// Every fact about which operation owns the conversation's single mutable
/// state — or permanently fences it — lives here (Issue #606). The grouped
/// state stays under the one coordinator lock: regrouping narrows the
/// ownership surface, it does not add a second linearization domain.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct ForegroundState {
    /// The one foreground execution slot: `Attempt`, `ManualCompaction`,
    /// `Scheduled`, `Idle`, or `Poisoned`, mutually exclusive by type
    /// (Issue #606).
    slot: foreground::ForegroundSlot,
    /// The physical manual-compaction task reclamation state:
    /// `None` | `Pending` | `Joining`, under this lock, so a claimed but
    /// unjoined task is never misread as reaped.
    manual_tail: foreground::ManualTailState,
    /// The next attempt identity ordinal.
    ///
    /// Seeded by startup recovery to one past the highest ordinal that
    /// already entered durable authority (Issue #12, M9a), so a restart can
    /// never reuse an `AttemptId` that names a different logical attempt in
    /// durable history. The durable Event Journal rejects a second
    /// `AttemptStarted` for one identity, so the invariant is enforced on
    /// both sides.
    next_attempt_seq: u64,
    /// Startup recovery's exact provenance for the already-canonical adopted
    /// turn that may continue through one new attempt.
    ///
    /// This is a one-shot permission, consumed by the first admission that
    /// finds no pending inbound. It is never set for an indeterminate
    /// external outcome (Class C), where continuing would risk duplicating an
    /// external side effect rustX cannot observe.
    recovered_continuation: Option<AttemptProvenance>,
    /// Candidate provenance of the recovered turn, not committed task authority.
    recovered_permission_task_message: Option<MessageId>,
    /// The coordinator-owned transient admission-cycle retry bookkeeping
    /// (Issue #63). The absorbing `DurabilityFailed` fact itself is NOT
    /// stored here: it lives in exactly one place, the runtime-owned
    /// [`DurabilityGate`], which every reader (`submit_inbound`, `model_set`,
    /// attempt admission, the registries, the shutdown diagnostic, the
    /// observation) consults. This field only tracks the finite-cycle retry
    /// allowances and the latest pending re-kick of the admission worker.
    admission_durability_cycle: AdmissionDurabilityCycle,
    /// How many attempts this coordinator has admitted, counted at the one
    /// admission publication point (Issue #193).
    ///
    /// This is the child driver's completeness proof, not a lifecycle fact:
    /// every admitted attempt produces exactly one terminal observation, so
    /// a driver that has consumed `n` terminals knows an attempt it has not
    /// observed exists precisely when this counter exceeds `n`.
    admitted_attempts: u64,
    /// Latest settlement handoff; child turn permits prevent admission past
    /// an unconsumed receipt.
    latest_settlement: Option<NativeAttemptSettlement>,
    /// The committed one-shot cancellation intent (Issue #60 child side):
    /// armed before any attempt exists, consumed by the next admission so
    /// the admitted attempt starts already-cancelled and its first
    /// model-turn arbitration resolves `CancelledBeforeStart`. At most one
    /// admission ever consumes it.
    one_shot_cancel: Option<CancellationReason>,
}

/// Configuration, model, and resource publication state (Issue #606).
///
/// The Session binding, the staged configuration commit pipeline, the
/// runtime MCP catalog pipeline, and the committed model/resource/approval
/// frontier future work freezes — all linearized together with foreground
/// admission under the one coordinator lock.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct ConfigurationState {
    binding_revision: u64,
    catalog_revision: u64,
    pending_runtime_catalog: Option<PendingRuntimeCatalog>,
    catalog_commit_blocked: bool,
    catalog_admission_deferred: bool,
    /// The one fully prepared configuration candidate waiting for this
    /// runtime's safe boundary. Committed before the next independent
    /// Attempt is admitted; a newer candidate supersedes it.
    staged_configuration: Option<StagedConfiguration>,
    /// The staged commit found a capability lifecycle admission in flight.
    configuration_commit_waiting: bool,
    /// An admission pass that reached a waiting staged commit; the worker
    /// resumes exactly that pass once the commit boundary is released.
    configuration_admission_deferred: bool,
    runtime_refresh_sequence: u64,
    runtime_catalog_failure: Option<RuntimeCatalogFailure>,
    explicit_model: bool,
    /// The session's authoritative mutable model state.
    ///
    /// It lives under the *same* lock that owns attempt admission, so a
    /// model update and an attempt admission can never interleave
    /// ambiguously: whichever acquires the lock first linearizes first.
    model: SessionModelState,
    /// The complete resource/capability generation future attempts acquire.
    resources: Arc<RuntimeResourceSnapshot>,
    /// The effective mode frozen for the currently admitted attempt boundary.
    effective_approval_mode: ApprovalMode,
    /// Mode last successfully injected as a permission reminder on this
    /// conversation. Cleared only when the next admission must re-emit
    /// (mode change, first admission, or post-compaction).
    last_permission_reminder_mode: Option<ApprovalMode>,
    /// Exact canonical task input committed by the latest primary request start.
    permission_task_message: Option<crate::runtime::identity::MessageId>,
}

/// Child-control admission facts of one child-scoped runtime (Issue #606).
///
/// Present only when this runtime runs child work: turn permits, the
/// terminal guidance seal, the one-shot cancellation intent, and the
/// settlement ledger the child driver consumes. The facts stay under the
/// same coordinator lock as every other admission fact.
/// The child-only control facts (Issue #606). A `Root` runtime never holds
/// this state: the `Option` is `Some` exactly when the declared
/// [`ConversationRole`] is a child role. Generic admission bookkeeping
/// (admitted attempts, latest settlement, one-shot cancellation intent)
/// lives on `ForegroundState` instead.
#[derive(Default)]
pub(crate) struct ChildControlState {
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
}

/// The coordinator's persistent health fences (Issue #606).
pub(crate) struct HealthState {
    /// The runtime-owned persistent fence for an MCP physical settlement that
    /// is not proven. The retirement registry remains the complete diagnostic
    /// authority; this latch is the coordinator's admission authority and is
    /// published under the same lock that serializes activation.
    mcp_settlement_failure: Option<String>,
}

/// The one synchronized coordinator state (the admission linearization
/// owner). Its fields are coordinator-internal: extracted owners reach them
/// only through the narrow `RuntimeInner` admission/settlement interfaces
/// (Issue #606).
pub(crate) struct CoordinatorState {
    /// Foreground execution slot, manual-compaction tail reclamation, and
    /// admission bookkeeping.
    foreground: ForegroundState,
    /// Configuration, model, and resource publication state.
    config: ConfigurationState,
    /// Child-control facts; `Some` exactly for a child-scoped runtime
    /// (installed when child control attaches).
    child: Option<ChildControlState>,
    /// Persistent health fences consulted at every admission boundary.
    health: HealthState,
}

impl CoordinatorState {
    /// The child-control facts, when this runtime carries them.
    fn child(&self) -> Option<&ChildControlState> {
        self.child.as_ref()
    }

    /// The mutable child-control facts. Present only on a child-scoped
    /// runtime; generic bookkeeping uses `ForegroundState` fields instead.
    fn child_mut(&mut self) -> &mut ChildControlState {
        self.child
            .as_mut()
            .expect("child control state requires a child conversation role")
    }
}

/// The shared state of one conversation runtime.
pub(crate) struct RuntimeInner {
    activity: activity::TaskActivity,
    configuration_owner: Arc<()>,
    /// The first configuration commit this allocation published but could
    /// not record. The durability gate keeps only its first failure, so this
    /// fact is kept apart from it.
    unrecorded_configuration: Arc<std::sync::OnceLock<String>>,
    initialization_cancellation:
        std::sync::OnceLock<crate::runtime::cancellation::CancellationSignal>,
    runtime_catalog_preparer:
        std::sync::OnceLock<Arc<crate::runtime::configuration_commit::RuntimeCatalogPreparer>>,
    runtime_catalog_publication: tokio::sync::watch::Sender<u64>,
    #[cfg(test)]
    runtime_catalog_processed: tokio::sync::watch::Sender<u64>,
    #[cfg(test)]
    runtime_catalog_blocked: tokio::sync::Notify,
    #[cfg(test)]
    runtime_refresh_completed_gate: Mutex<Option<Arc<Gate>>>,
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
    /// The explicit conversation role declared at composition (Issue #606).
    role: ConversationRole,
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
    /// The extracted scheduled-workflow executor (Issue #606): the runtime
    /// keeps only this handle for drain sequencing — no scheduled
    /// preparation, claim, execution or settlement logic remains on the
    /// runtime.
    scheduled_executor: Arc<crate::runtime::workflow::scheduled::executor::ScheduledExecutor>,
    /// The extracted external-auxiliary service (Issue #606): the runtime
    /// keeps only this handle for drain sequencing and for handing the
    /// service to composed consumers — no auxiliary business logic remains
    /// on `ConversationRuntime` itself.
    external_auxiliary: Arc<crate::runtime::auxiliary::external::ExternalAuxiliary>,
    /// The shared native auxiliary executor (Issue #606): a core runtime
    /// capability consumed directly by Attempt, Compaction, and the
    /// coordinator's auxiliary-admission interface — never reached through
    /// the external service's classify/BTW/title business surface.
    auxiliary_executor: crate::runtime::auxiliary::executor::AuxiliaryExecutor,
    /// The runtime-owned host-task slot: optional host-installed work (the
    /// automatic session-title task) supervised by drain, installable once
    /// and only while the runtime is `Inactive` (Issue #606).
    host_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
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
    state: Arc<Mutex<CoordinatorState>>,
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
    settlement: Arc<tokio::sync::Notify>,
    /// Test-only coordinator synchronization hooks.
    #[cfg(test)]
    probe: Arc<Mutex<Option<CoordinatorProbe>>>,
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
    ///
    /// Controlled recovery boundary (Issue #606): a panic *while holding*
    /// the coordinator guard poisons the mutex. Re-panicking every later
    /// caller would wedge the manual-tail reaper, the scheduled owner and
    /// shutdown on a lock they can never fix, so the guard is recovered
    /// instead. The interrupted mutation may have left the coordinator
    /// state mid-transition, so recovery converges business admission to
    /// the absorbing failed state — the `DurabilityFailed` fact commits
    /// through the one authority — while physical drain keeps working:
    /// reapers, joins and shutdown still need the guard and observe the
    /// mid-transition facts as-is.
    ///
    /// The lent foreground slot is deliberately NOT poisoned here: a
    /// poisoned mutex proves the critical section panicked, not that the
    /// live foreground task exited. Its owner record carries the
    /// cancellation handle and the settlement responsibility — destroying
    /// them would also pre-empt the task's own
    /// `poison_foreground_for_task` handoff, which is the path that knows
    /// the task exited and publishes the `RuntimeFailed` receipt. Once
    /// the state has converged to its explicit failure fact, the mutex's
    /// poison mark is cleared so non-blocking readers (`try_activity`)
    /// observe the same converged state instead of spinning on a poison
    /// flag that can never heal.
    fn lock_state(&self) -> MutexGuard<'_, CoordinatorState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let guard = poisoned.into_inner();
                let _ = self.durability_gate.commit_failure(
                    DurableOperation::CanonicalCommit,
                    "the coordinator state lock was poisoned by a panic inside a critical section"
                        .to_owned(),
                );
                self.state.clear_poison();
                guard
            }
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
    ) -> Result<crate::scheduler::ScheduleOperationResult, crate::runtime::inbound::MailboxError>
    {
        if let crate::scheduler::ScheduleOperation::WorkflowReconcile { id, after, limit } =
            operation
        {
            return self
                .inner
                .scheduled_executor
                .reconcile(binding, id, (*after, *limit));
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
                // Contract preparation runs off the coordinator lock, on a
                // configuration generation frozen through the coordinator's
                // snapshot interface; the Claim critical section re-validates
                // that generation before the start commits.
                let native = self.inner.scheduled_executor.prepare_workflow_command(
                    binding,
                    workflow_id,
                    arguments,
                );
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
            self.inner.clock.as_ref(),
        )?;
        if let crate::scheduler::ScheduleOperation::WorkflowCancel { id } = operation {
            self.inner.cancel_scheduled_owner(id);
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

    /// Cache reclamation is advisory: no shutdown claim, durable probe or wait.
    pub(crate) fn reclaim_idle_store_cache(&self) -> bool {
        let Ok(state) = self.inner.state.try_lock() else {
            return false;
        };
        if self
            .inner
            .idle_epoch_probed(&state, admission::IdleProbe::InMemory)
            .is_err()
        {
            return false;
        }
        match self.inner.tool_runtime.durable_store().reclaim_idle_cache() {
            Ok(reclaimed) => reclaimed,
            Err(error) => {
                tracing::warn!(%error, "idle store cache maintenance failed");
                false
            }
        }
    }

    /// Context inputs projected from one immutable published generation.
    /// Admitted attempts retain their own captured resources and engine state.
    #[must_use]
    pub fn context_config(&self) -> ConversationContextConfig {
        let resources = self.runtime_resources();
        let configuration = resources.configuration();
        ConversationContextConfig {
            status_clock: Arc::clone(&self.inner.context.status_clock),
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
                        .agent_status_engine(Arc::clone(&self.inner.context.status_clock))
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
        self.inner.lock_state().config.resources.clone()
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

    /// The extracted external-auxiliary service (Issue #606). Composed
    /// consumers (Runtime Client Host, session title) use it directly; the
    /// runtime keeps this handle only for drain sequencing.
    #[must_use]
    pub fn external_auxiliary(
        &self,
    ) -> Arc<crate::runtime::auxiliary::external::ExternalAuxiliary> {
        self.inner.external_auxiliary.clone()
    }

    /// The conversation-level durable authority (Issue #606): exposed for
    /// extracted consumers whose business logic lives outside the runtime.
    pub(crate) fn store(&self) -> Arc<dyn ConversationStore> {
        self.inner.store.clone()
    }

    /// The runtime's stamping clock.
    pub(crate) fn clock(&self) -> Arc<dyn RuntimeClock> {
        self.inner.clock.clone()
    }

    /// The generic optional host-task hook (Issue #606): exactly one
    /// host-installed background task (the automatic session-title owner)
    /// may be installed, and only while the runtime is `Inactive`.
    ///
    /// The lifecycle check and the registration are one coordinator
    /// critical section: a pre-activation drain can never take a free slot
    /// between the check and the install. The caller passes a task
    /// *constructor* — the runtime spawns it only after admission, so a
    /// refused install can never detach live work the caller already lost.
    pub(crate) fn install_host_task(
        &self,
        task: impl FnOnce() -> tokio::task::JoinHandle<()>,
    ) -> Result<(), crate::context::ContextError> {
        let _state = self.inner.lock_state();
        if self.inner.lifecycle.state() != ConversationLifecycleState::Inactive {
            return Err(crate::context::ContextError::new(
                crate::context::ContextErrorKind::Cancelled,
                "host task installation requires an inactive runtime",
            ));
        }
        let mut slot = self.inner.host_task.lock().expect("host task slot");
        if slot.is_some() {
            return Err(crate::context::ContextError::new(
                crate::context::ContextErrorKind::AuxiliaryBusy,
                "a host task is already installed",
            ));
        }
        *slot = Some(task());
        Ok(())
    }

    /// Waits for the latest admitted attempt's settlement receipt to
    /// advance past `after` — generic foreground bookkeeping, shared by the
    /// child driver's `ChildControl::child_attempt_settlement`.
    pub(crate) async fn attempt_settlement(&self, after: u64) -> NativeAttemptSettlement {
        loop {
            let notified = self.inner.settlement.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(receipt) = self
                .inner
                .lock_state()
                .foreground
                .latest_settlement
                .clone()
                .filter(|receipt| receipt.ordinal > after)
            {
                return receipt;
            }
            notified.await;
        }
    }

    /// Test capture of the existing retained execution receipt ordinal.
    #[cfg(test)]
    pub(crate) fn settlement_ordinal_for_test(&self) -> u64 {
        self.inner
            .lock_state()
            .foreground
            .latest_settlement
            .as_ref()
            .map_or(0, |receipt| receipt.ordinal)
    }

    /// The child's control handle (Issue #606): `Some` exactly when the
    /// declared [`ConversationRole`] is a child role (`SubagentChild` or
    /// `WorkflowAgentRun`), `None` for `Root`. Available from composition
    /// while `Inactive`; it does not wait for activation.
    #[must_use]
    pub fn child_control(&self) -> Option<child::ChildControl> {
        self.inner
            .role
            .is_child()
            .then(|| child::ChildControl::new(self.clone()))
    }

    /// The test-only coordinator probe slot, for extracted consumers whose
    /// test hooks live outside the runtime (Issue #606).
    #[cfg(test)]
    pub(crate) fn coordinator_probe(&self) -> &Mutex<Option<CoordinatorProbe>> {
        &self.inner.probe
    }
}
