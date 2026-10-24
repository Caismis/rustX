//! The deterministic agent execution loop (M3 + M4 context integration).
//!
//! The loop turns one provider-independent `ModelStreamItem` stream into an
//! attempt execution. Canonical `Event(ModelEvent)` items enter assembly;
//! ephemeral `Progress` items only advance request-local deadlines:
//!
//! ```text
//! input
//!  ↓
//! ConversationState (Message Ledger + Conversation Surface) + pending
//! FreshInboundTurn
//!  ↓
//! Context Assembly (native + extension + deferred post-tool proposals)
//!  ↓
//! PreStepPolicy → Enter | Reject(reason)
//!  ↓
//! transient staged context / PreparedModelTurn (nothing commits)
//!  ↓
//! cancellation-vs-start arbitration
//!    ├─ cancel: discard staged request
//!    └─ start: fused durable model-turn-start transaction
//!  ↓
//! provider invocation
//!  ↓
//! ModelStreamItem (canonical Event or ephemeral Progress)
//!  ├─ Progress → request deadline state only
//!  └─ Event → message assembly + RuntimeEvent emission
//!  ↓
//! tool calls (if requested): resolve, execute, record
//!  ↓
//! tool batch structural settlement → ToolResultObserver pass →
//! Agent-Loop-owned deferred context buffer
//!  ↓
//! TurnCompleted → safe boundary → one finite inbound mailbox drain
//!  ↓
//! continuation (or proactive compaction / compact-and-retry on overflow)
//!  ↓
//! terminal settlement candidate → durable terminal RuntimeEvent when commit succeeds
//! ```
//!
//! Ownership: the loop owns execution semantics, message assembly, tool
//! execution, continuation state, cancellation observation, context assembly,
//! request admission/snapshots, fresh-inbound lifecycle, safe-boundary inbound
//! consumption, and lifecycle-extension coordination. The durable
//! `ConversationStore` owns canonical facts and the Event Journal; the loop
//! keeps only bounded active execution state and the current conversation
//! working set. The adapter owns provider protocol translation only. No
//! provider protocol concept appears in this module.
//!
//! The typed lifecycle seams of Issue #56 live on the required immutable
//! [`AttemptLifecycle`]: exactly one [`PreStepPolicy`] evaluation per primary
//! step and exactly one [`ToolResultObserver`] pass per structurally settled
//! tool batch. Neither seam receives canonical-history, tool-identity,
//! cancellation, or terminal authority, and neither can create a second
//! context-admission path: deferred post-tool proposals re-enter the same
//! Context Assembly → pre-step policy → admission pipeline as every other
//! proposal.
//!
//! [`PreStepPolicy`]: super::lifecycle::PreStepPolicy
//! [`ToolResultObserver`]: super::lifecycle::ToolResultObserver
//!
//! The context path is mandatory: every normal `AgentExecution` is
//! constructed with a [`ContextRuntime`], and there is exactly one normal
//! execution model — no no-context/unbounded mode. Agent Status is optional
//! `FreshInbound`/`PostToolBatch` enrichment for an already-existing primary
//! step. The conversation inbound mailbox is owned by the conversation tool runtime: the loop drains exactly
//! `tool_runtime.mailbox()` at every safe boundary, so background terminal
//! notifications always enter the same mailbox the Agent Loop drains.
//! Cancellation is a generic Agent Loop invariant for every execution:
//! observable cancellation is checked before every model turn begins.

#[cfg(test)]
pub(crate) mod attribution;
mod compaction;
mod construction;
mod publication;
mod recovery;
mod request;
mod start;
mod step;
mod stream;
mod terminal;
#[cfg(test)]
pub(crate) mod test_sync;
mod tools;

use self::publication::OpenPublication;
use self::start::RequestMeasurements;
use self::step::LogicalStep;
use self::tools::PendingPostToolBatch;
use super::cancellation::AgentCancellation;
use super::lifecycle::AttemptLifecycle;
use super::observer::AgentExecutionObserver;
use super::state::{ExecutionState, ExecutionStateMachine};
use crate::capabilities::AttemptCapabilityLease;
use crate::context::{ContextRuntime, DeferredContextProposal};
use crate::conversation::ConversationState;
use crate::durable::ConversationStore;
use crate::events::types::AttemptOutcome;
use crate::message::types::MessageBlock;
use crate::model::deadline::ModelTimeoutPolicy;
use crate::model::generation::GenerationSafetyPolicy;
use crate::model::generation_evidence::GenerationTiming;
use crate::model::session::AttemptModelSnapshot;
use crate::publication::CoalescePolicy;
use crate::runtime::MonotonicClock;
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, RequestId};
use crate::runtime::inbound::{FreshInboundTurn, InitialTurnTrigger};
use crate::tools::deadline::ToolExecutionDeadlinePolicy;
use crate::tools::executor::ToolRegistry;
use crate::tools::runtime::ConversationToolRuntime;
use std::sync::Arc;

/// The bounded M4 recovery budget for `ContextWindowExceeded`.
///
/// Context-overflow recovery is semantic compaction, not generic transient
/// retry. It remains one recovery opportunity per logical model step, while
/// both recovery paths share the actual-request ordinal owned by the Agent
/// Loop.
pub const MAX_CONTEXT_OVERFLOW_RETRIES_PER_MODEL_TURN: u32 = 1;

/// The maximum number of adapter-classified transient retries in one logical
/// model step.
pub const MAX_TRANSIENT_MODEL_RETRIES_PER_LOGICAL_STEP: u32 = 3;

/// The bounded **semantic corrective-generation** budget of one logical
/// model step, counted in semantic generations.
///
/// A physical generation can fail semantically in several ways: its tool
/// intent cannot cross `ToolCall` acceptance (Issue #201), one of its
/// generated channels deterministically degenerates, or it exhausts a
/// generation budget before completing (Issue #203). Every one of those is
/// the same kind of fact — *this generation is unusable* — so they share one
/// budget rather than each owning a private retry:
///
/// ```text
/// logical model step
///   semantic generation #1
///       accepted        -> commit
///       semantic anomaly
///           budget unused -> discard, bounded ephemeral correction,
///                            semantic generation #2
///           budget spent  -> explicit terminal generation failure
/// ```
///
/// With a budget of one, a logical step can never have a third semantic
/// generation caused by these conditions, in any order. `malformed ->
/// degeneration -> corrective` and `degeneration -> malformed -> corrective`
/// are not representable, because the second anomaly finds the budget spent.
///
/// This counts generations, not requests. One corrective generation may be
/// realized by several actual provider requests when a transport failure or
/// a context overflow interrupts it; those are attempts at the same frozen
/// semantic state and consume the transient and overflow budgets, never this
/// one. The step-owned `SemanticGenerationRecovery` state owns that
/// distinction.
///
/// This budget is deliberately separate from — and never resets — the
/// transient transport budget, the context-overflow recovery budget, or any
/// Tool-side behaviour. It is keyed on the error classes
/// `SemanticGenerationAnomaly::classify` recognizes, which no other recovery
/// path accepts.
pub const MAX_SEMANTIC_CORRECTIVE_GENERATIONS_PER_LOGICAL_STEP: u32 = 1;

/// The upper bound for an adapter-provided retry delay.
pub const MAX_MODEL_RETRY_DELAY_MS: u64 = 60_000;

/// Everything the loop needs to know about one attempt.
#[derive(Debug, PartialEq)]
pub struct AgentExecutionRequest {
    /// The agent being executed.
    pub agent_id: AgentId,
    /// The conversation the attempt belongs to.
    pub conversation_id: ConversationId,
    /// The attempt identity reported by attempt-level events.
    pub attempt_id: AttemptId,
    /// The one canonical conversation state the attempt takes ownership of.
    ///
    /// Ownership transfers into the attempt: while the attempt runs it is
    /// the single mutable conversation authority, and settlement transfers
    /// the state back to the host through
    /// [`AgentExecutionResult::conversation`]. There is deliberately no
    /// clone-based `initial_messages` API, so two independently mutable
    /// conversation copies are not representable.
    pub conversation: ConversationState,
    /// The explicit execution trigger of the attempt's first model turn.
    ///
    /// Fresh inbound identity is explicit execution state, never inferred
    /// from message role or history shape. [`InitialTurnTrigger::FreshInbound`]
    /// makes the `FreshInbound` Agent Status opportunity eligible.
    pub initial_turn_trigger: InitialTurnTrigger,
    /// The one immutable model ownership object of the attempt.
    ///
    /// The loop receives exactly this instead of independent
    /// mutable-looking model fields. Every model turn of the attempt — the
    /// first request, every tool→model continuation, every context-overflow
    /// retry, every proactive-compaction continuation, and every compaction
    /// summary — uses this frozen snapshot and never reads live session
    /// model state.
    pub model: AttemptModelSnapshot,
}

/// The durable authority stage that failed while an attempt was executing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableFailureKind {
    /// A canonical Message Ledger/Surface commit failed.
    CanonicalCommit,
    /// The immutable request-start snapshot/event transaction failed.
    RequestStart,
    /// The atomic compaction transition failed.
    Compaction,
    /// A standalone or terminal Event Journal append failed.
    EventJournal,
    /// A durable publication-plane transition failed: a stream could not
    /// open, frames could not be staged before release, the publication
    /// terminal could not commit, or an audit could not terminalize.
    Publication,
}

impl DurableFailureKind {
    /// The stable coordinator diagnostic name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CanonicalCommit => "canonical_commit",
            Self::RequestStart => "request_start",
            Self::Compaction => "compaction",
            Self::EventJournal => "event_journal",
            Self::Publication => "publication",
        }
    }
}

/// The deterministic result of one attempt execution.
///
/// The `outcome` field is the execution state-machine settlement candidate;
/// it remains meaningful when the final terminal Event Journal append fails,
/// in which case no terminal fact is published and `durable_failure_kind` is
/// `EventJournal`.
///
/// `conversation` is the bounded current working state handed back to the
/// host: active Ledger bodies and the current Conversation Surface. The
/// complete append-only Ledger and historical Surface revisions remain in the
/// durable `ConversationStore` and are read there on demand.
#[derive(Debug, PartialEq)]
pub struct AgentExecutionResult {
    /// The executed attempt.
    pub attempt_id: AttemptId,
    /// The platform outcome of the execution settlement candidate. For a
    /// normally settled attempt it is also the outcome of the one committed
    /// terminal event; a terminal Event Journal failure can leave that event
    /// absent while this value remains available to the coordinator.
    pub outcome: AttemptOutcome,
    /// The terminal state-machine settlement that produced the terminal
    /// event: [`ExecutionState::Completed`] for successful settlement and
    /// [`ExecutionState::Failed`] for failure and cancellation settlement.
    pub terminal_state: ExecutionState,
    /// The bounded current conversation read model, transferred back to the
    /// runtime. Historical durable facts remain in `ConversationStore`.
    pub conversation: ConversationState,
    /// The durable-authority failure the attempt encountered, when one
    /// caused or contributed to its failure settlement (Issue #63).
    ///
    /// The coordinator uses this to keep its durability-health state
    /// honest: after an active-attempt durable canonical-write failure the
    /// runtime must not return to a false healthy state and admit further
    /// work as though storage were fine. The in-memory conversation state
    /// handed back stayed consistent with the durable Ledger — a failed
    /// durable commit installed nothing — but the durable authority itself
    /// is marked failed.
    pub durable_failure: Option<String>,
    /// The typed durable stage associated with [`Self::durable_failure`].
    pub durable_failure_kind: Option<DurableFailureKind>,
}

impl AgentExecutionResult {
    /// The hot committed message bodies available in the settled attempt, in
    /// current Surface order. After a durable restart this is the current
    /// Surface working set; use the `ConversationStore` for Ledger commit order
    /// and retired history.
    ///
    /// This explicitly enumerates the hot Ledger read model. The engine's
    /// normal projection and compaction paths never enumerate the durable
    /// historical Ledger; callers needing retired history page the store.
    #[must_use]
    pub fn messages(&self) -> Vec<MessageBlock> {
        self.conversation.ledger().audit_records()
    }

    /// The active ordered message identities of the settled Conversation
    /// Surface.
    #[must_use]
    pub fn active_ids(&self) -> Vec<MessageId> {
        self.conversation.active_ids()
    }
}

/// Runtime-owned generic execution inputs frozen for one admitted operation.
///
/// The runtime supplies the generic execution policy and synchronization
/// authority explicitly at this boundary; the context plane is not queried
/// for either value. This is a narrow admission handoff, not a second owner
/// of runtime execution policy.
#[derive(Clone)]
pub(crate) struct AgentExecutionRuntimePolicy {
    /// The timeout policy frozen for this admitted attempt or operation.
    pub(crate) model_timeout_policy: ModelTimeoutPolicy,
    /// The tool execution-liveness policy frozen for this admitted attempt
    /// or operation (Issue #204). It is copied here at admission: a running
    /// `ToolCall` can never observe a later policy value, and no executor
    /// ever receives a runtime-current configuration handle.
    pub(crate) tool_deadline_policy: ToolExecutionDeadlinePolicy,
    /// The shared runtime monotonic clock.
    pub(crate) monotonic_clock: Arc<dyn MonotonicClock>,
    /// The attempt-scoped subagent resolution view (Issue #144): the
    /// immutable runtime resource generation this attempt was admitted with,
    /// plus the model authority frozen at the same admission boundary.
    ///
    /// It is frozen here rather than read by the registered `subagent`
    /// executor, so an attempt admitted under generation R1 keeps resolving
    /// R1 even after a reload has committed R2 as runtime-current. `None`
    /// means the composition owns no subagent plane at all — a standalone
    /// execution fixture, or a subagent child itself.
    pub(crate) subagent_context: Option<crate::runtime::subagent::NativeExecutionContext>,
    /// The reserved Workflow Agent terminal protocol, when this execution
    /// belongs to a Workflow-owned child. It is consumed before ordinary
    /// Tool Plane preflight and dispatch.
    pub(crate) workflow_output: Option<Arc<dyn crate::runtime::workflow::WorkflowOutputTerminal>>,
}

/// One agent attempt execution.
///
/// The loop borrows the model adapter, the immutable tool registry, the
/// attempt cancellation signal, and owns the attempt capability lease, the
/// mandatory M4 context runtime, the conversation tool runtime (whose
/// canonical mailbox the loop drains), the execution state machine, the
/// bounded current read model, the retained continuation state, and the
/// pending `FreshInbound` trigger and attempt-local `PostToolBatch` opportunity.
pub struct AgentExecution<'a> {
    request: AgentExecutionRequest,
    capability: AttemptCapabilityLease,
    admitted_tools: std::sync::Arc<ToolRegistry>,
    cancellation: &'a AgentCancellation,
    tool_runtime: &'a ConversationToolRuntime,
    /// The conversation-level durable authority. Tool execution receives
    /// only the mailbox capability; canonical/request/event durability is
    /// owned by the conversation execution plane.
    store: std::sync::Arc<dyn ConversationStore>,
    state: ExecutionStateMachine,
    /// The attempt's owned conversation state: the single mutable
    /// conversation authority for the attempt's lifetime.
    conversation: ConversationState,
    /// The durable-authority failure encountered by this attempt, when
    /// any (Issue #63): carried into [`AgentExecutionResult`] so the
    /// coordinator never returns to a false healthy durability state
    /// after an active-attempt durable failure, regardless of how the
    /// terminal outcome itself is classified.
    durable_failure: Option<String>,
    durable_failure_kind: Option<DurableFailureKind>,
    pending_continuation: Option<ProviderContinuationState>,
    /// The committed Assistant message that established the pending
    /// continuation, when one is pending.
    continuation_owner: Option<MessageId>,
    continuation_source: Option<crate::runtime::continuation::AcceptedOutputContinuationSource>,
    /// The pending fresh inbound turn: `Some` until a successful model
    /// invocation has observed it. One pending fresh inbound turn produces
    /// at most one Agent Status generation.
    pending_fresh_inbound: Option<FreshInboundTurn>,
    /// One consumable authorization owned by the current Human request.
    /// Foreground Goal Tools borrow it to consume only after create commits.
    goal_creation_authorization: std::sync::Mutex<Option<crate::goal::GoalOrigin>>,
    schedule_authorization: std::sync::Mutex<Option<crate::scheduler::control::ScheduleHumanGrant>>,
    /// The attempt-local marker installed after one complete canonical
    /// `ToolResult` batch settles, together with the durable transcript
    /// position that batch committed at. It is consumed by the next
    /// primary-step preparation and is never persisted or reconstructed.
    pending_post_tool_batch: Option<PendingPostToolBatch>,
    context_runtime: ContextRuntime,
    /// The attempt-scoped subagent resolution view, frozen at admission
    /// together with the attempt's model snapshot and capability lease.
    subagent_context: Option<crate::runtime::subagent::NativeExecutionContext>,
    /// The Workflow-owned terminal-output authority, when present.
    workflow_output: Option<Arc<dyn crate::runtime::workflow::WorkflowOutputTerminal>>,
    /// Bounded feedback for the next child model turn after a protocol or
    /// schema rejection. This is request-only context, never canonical
    /// conversation history.
    workflow_output_feedback: Option<String>,
    /// The one required immutable lifecycle configuration of the attempt.
    /// It carries the attempt's single pre-step policy owner and its
    /// identity-registered tool-result observers; the inert configuration is
    /// the identity.
    lifecycle: AttemptLifecycle,
    /// The working set of the current logical primary step. It is replaced
    /// as one value at every logical-step boundary; the actual-request
    /// retries of the step reuse it.
    logical_step: LogicalStep,
    /// The Agent-Loop-owned staging buffer of deferred context.
    ///
    /// It is **not** canonical history and **not** a second transcript: a
    /// staged proposal is a transient value that becomes a conversation fact
    /// only after the next Context Assembly, the pre-step policy, and the
    /// admission boundary accept it. The buffer is filled after a tool batch
    /// reaches structural settlement, in canonical `(ToolCall batch position,
    /// producer identity, proposal FIFO)` order, and drained by the very next
    /// primary step's assembly, so it never accumulates across turns.
    ///
    /// Each entry carries the trusted producer identity the Agent Loop stamped
    /// from the observer's registration. The buffer records *timing*; the
    /// identity records *ownership*, and Context Assembly derives lane and
    /// provenance from the identity alone.
    deferred_context: Vec<DeferredContextProposal>,
    /// Per-attempt context-generation allocator owned by the Agent Loop.
    context_generation_serial: u64,
    /// Provider-observed input measurements. They outlive the request that
    /// produced them and are reused by later compatible requests and steps.
    measurements: RequestMeasurements,
    /// The exact request identity of the in-flight provider request. The
    /// provider outcome fact (P) and the publication stream both name it, so
    /// neither can be attributed to a different request of the same turn.
    last_request_id: Option<RequestId>,
    /// Generation timing evidence being accumulated for the in-flight actual
    /// request. Installed at that request's dispatch frontier and taken by
    /// whichever settlement path records its provider terminal, so exactly
    /// one terminal fact can carry it and a retry never inherits it.
    generation_timing: Option<GenerationTiming>,
    /// The bounded publication policy of the attempt.
    publication_policy: CoalescePolicy,
    /// The monotonic clock the publication latency policy reads.
    monotonic_clock: std::sync::Arc<dyn MonotonicClock>,
    /// The frozen runtime execution policy copied into every actual request
    /// admitted by this attempt.
    model_timeout_policy: ModelTimeoutPolicy,
    /// The resolved runtime generation-safety policy of every actual request
    /// admitted by this attempt, frozen once from the attempt's own model
    /// invocation. It is execution state, never model input.
    generation_policy: GenerationSafetyPolicy,
    /// The frozen tool execution-liveness policy of this attempt (Issue
    /// #204), copied at admission and owning the hard/idle deadlines of
    /// every foreground call this attempt starts.
    tool_deadline_policy: ToolExecutionDeadlinePolicy,
    /// The open, not-yet-settled publication stream of the in-flight model
    /// request. Exactly one stream is open at a time: a stream settles — as
    /// canonical, unaccepted, or incomplete — before the next one opens.
    publication: Option<OpenPublication>,
    /// Set when an attempted audit settlement failed. The stream remains in
    /// durable authority for startup recovery, but the Agent Loop must not
    /// retry the transition in the same control-flow turn and accidentally
    /// continue into a second request.
    publication_settlement_failed: bool,
    /// The optional live observation seam: when attached, every emitted
    /// runtime fact, every committed canonical message, and every composed
    /// Agent Status is observed at its commit linearization point. The
    /// durable Event Journal remains the historical authority regardless of
    /// attachment.
    observer: Option<&'a dyn AgentExecutionObserver>,
    /// Test-only deterministic synchronization points; never present outside
    /// `#[cfg(test)]`.
    #[cfg(test)]
    pauses: test_sync::ExecutionPauses,
    turn: u32,
    terminal_emitted: bool,
}

#[cfg(test)]
mod test_fixtures;
