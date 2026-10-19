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
use self::recovery::SemanticGenerationRecovery;
use self::request::{FrozenCarryover, FrozenLogicalInput};
use self::start::RequestObservationIdentity;
use self::step::LogicalModelStepState;
use self::terminal::{TerminalCarryoverDecision, TerminalCommitState};
use self::tools::PendingPostToolBatch;
use super::cancellation::AgentCancellation;
use super::lifecycle::AttemptLifecycle;
use super::observer::AgentExecutionObserver;
use super::state::{ExecutionState, ExecutionStateMachine};
use crate::capabilities::AttemptCapabilityLease;
use crate::context::tokens::{ObservedAnchor, ProviderObservedInput};
use crate::context::{AcceptedContext, ContextRuntime, DeferredContextProposal};
use crate::conversation::ConversationState;
use crate::durable::ConversationStore;
use crate::events::types::AttemptOutcome;
use crate::message::types::MessageBlock;
use crate::model::deadline::ModelTimeoutPolicy;
use crate::model::generation::GenerationSafetyPolicy;
use crate::model::generation_evidence::GenerationTiming;
use crate::model::session::AttemptModelSnapshot;
use crate::model::snapshot::RequestIdentity;
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
    pub(crate) subagent_context: Option<crate::runtime::subagent::AttemptSubagentContext>,
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
    /// The attempt-local marker installed after one complete canonical
    /// `ToolResult` batch settles, together with the durable transcript
    /// position that batch committed at. It is consumed by the next
    /// primary-step preparation and is never persisted or reconstructed.
    pending_post_tool_batch: Option<PendingPostToolBatch>,
    context_runtime: ContextRuntime,
    /// The attempt-scoped subagent resolution view, frozen at admission
    /// together with the attempt's model snapshot and capability lease.
    subagent_context: Option<crate::runtime::subagent::AttemptSubagentContext>,
    /// The Workflow-owned terminal-output authority, when present.
    workflow_output: Option<Arc<dyn crate::runtime::workflow::WorkflowOutputTerminal>>,
    /// Bounded feedback for the next child model turn after a protocol or
    /// schema rejection. This is request-only context, never canonical
    /// conversation history.
    workflow_output_feedback: Option<String>,
    /// Workflow correction frozen for this logical generation, including recovery versions.
    frozen_workflow_feedback: Option<String>,
    /// The semantic-generation recovery state of the current logical model
    /// step: its one spent corrective-generation budget, shared by every
    /// anomaly class, and — while a corrective generation is pending — the
    /// one bounded corrective hint every actual request of that generation
    /// carries.
    ///
    /// This is noncanonical request-only state. It is frozen into the exact
    /// `RequestSnapshot` of every actual request that used it — durable
    /// audit evidence, so a historical request stays byte-for-byte
    /// reconstructible — but it never enters canonical Assistant/User
    /// history and never survives the corrective generation as semantic
    /// context. The discarded generation itself is never stored here at
    /// all.
    semantic_recovery: SemanticGenerationRecovery,
    /// The transient accepted context for the current admitted logical model
    /// step. It is retained across every actual-request retry and discarded
    /// only when the next logical step begins.
    accepted_context: Option<AcceptedContext>,
    /// The one publication-audit carryover frozen for the current logical
    /// primary step. It is never reloaded for a retry.
    frozen_carryover: Option<FrozenCarryover>,
    /// Latest authorized immutable semantic version of the current logical generation.
    current_input: Option<Box<FrozenLogicalInput>>,
    /// The current logical generation's accepted Context identities, exactly once.
    logical_context_ids: Vec<MessageId>,
    /// The last request identity that durably crossed model start in the
    /// current logical step. It is the input to the shared live/recovery
    /// carryover selector.
    last_started_request: Option<RequestIdentity>,
    /// Whether the current logical step has started model execution without
    /// reaching canonical Assistant acceptance yet.
    logical_model_step: LogicalModelStepState,
    /// A terminal cannot be recorded until publication evidence and the
    /// carryover source-selection decision are durably ready.
    terminal_commit_state: TerminalCommitState,
    /// The producer decision frozen after the current unresolved step's audit
    /// settles. `Some(None)` is meaningful: the new unresolved step replaces
    /// any older pending source with an explicit clear.
    terminal_carryover_decision: TerminalCarryoverDecision,
    /// The one required immutable lifecycle configuration of the attempt.
    /// It carries the attempt's single pre-step policy owner and its
    /// identity-registered tool-result observers; the inert configuration is
    /// the identity.
    lifecycle: AttemptLifecycle,
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
    observed: Option<ProviderObservedInput>,
    /// The request identity [`Self::observed`] was measured under.
    ///
    /// A provider counted the input of one concrete request. That count is
    /// evidence about a later request only while the parts of the request
    /// the conversation projection does not describe — the resolved
    /// invocation and the opaque provider continuation — are still the
    /// same. When they are not, the measurement is dropped rather than
    /// stretched over a request the provider never measured.
    observed_request_identity: Option<Arc<RequestObservationIdentity>>,
    /// The request identity of the last started request, promoted alongside
    /// its measurement when the provider reports one.
    last_request_identity: Option<Arc<RequestObservationIdentity>>,
    /// The structural anchor of the last started request, kept so its
    /// provider-reported input measurement can be reused for every later
    /// request context that extends it.
    last_request_anchor: Option<ObservedAnchor>,
    /// The runtime's deterministic input estimate for the last started
    /// request, kept so a provider overflow can be turned into a measured
    /// estimator correction instead of being retried against the same
    /// budget.
    last_request_estimated_input: Option<u64>,
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
    /// Test-only control point parked at the turn-continuation boundary:
    /// after a completed turn (and all its mailbox drain/append work)
    /// returned "continue", before the generic cancellation check of the
    /// next model turn; never present outside `#[cfg(test)]`. The `Mutex`
    /// keeps an execution with an installed pause `Sync` (the pause holds
    /// a `std::sync::mpsc::Receiver`), so host-driven attempt tasks can be
    /// spawned across threads in tests.
    #[cfg(test)]
    continuation_pause: std::sync::Mutex<Option<test_sync::ContinuationBoundaryPause>>,
    /// Test-only control point at the one M9b model-turn start boundary:
    /// immediately before the cancellation-vs-start arbitration, and inside
    /// the arbitration critical section immediately before the durable
    /// start commit. No request-scoped context, Request Snapshot, start
    /// fact, or provider request exists while the first phase is parked;
    /// while the second phase is parked the arbitration holds the start
    /// gate, so a concurrent cancellation provably blocks behind it.
    #[cfg(test)]
    start_boundary_pause: std::sync::Mutex<Option<test_sync::StartBoundaryPause>>,
    /// Test-only control point after `ModelRetryScheduled` commits and after
    /// the retry deadline is captured, before the monotonic backoff wait
    /// begins. It makes the cancellation/backoff ordering observable without
    /// sleeping.
    #[cfg(test)]
    retry_backoff_pause: std::sync::Mutex<Option<test_sync::RetryBackoffPause>>,
    /// Test-only control point after one provider stream item has been
    /// classified and fully processed. It makes exact provider/deadline and
    /// publication/deadline cuts observable without scheduler timing.
    #[cfg(test)]
    model_stream_item_pause: std::sync::Mutex<Option<test_sync::ModelStreamItemPause>>,
    /// Test-only control point immediately before one provider arbitration.
    /// The controller can make multiple branches ready while the execution is
    /// parked, then release it to prove the explicit biased precedence.
    #[cfg(test)]
    model_arbitration_pause: std::sync::Mutex<Option<test_sync::ModelArbitrationPause>>,
    /// Test-only control point after a rejected physical generation is
    /// durably settled and before the logical step decides whether a
    /// corrective generation is authorized. It makes the
    /// cancellation-versus-generation-anomaly race an exact interleaving
    /// rather than a likely one.
    #[cfg(test)]
    generation_anomaly_pause: std::sync::Mutex<Option<test_sync::GenerationAnomalyPause>>,
    /// Test-only control point after a foreground tool-start fact and before
    /// the next sibling's start frontier advances. This makes cancellation
    /// during a parallel batch deterministic without changing production
    /// scheduling.
    #[cfg(test)]
    tool_start_pause: std::sync::Mutex<Option<test_sync::ToolStartPause>>,
    /// Test-only signal published when one foreground call's hard deadline
    /// becomes an absolute monotonic value, which is strictly after the
    /// `ToolExecutionStarted` fact. A manual-clock test uses it to advance
    /// the clock across a frontier that provably already exists.
    #[cfg(test)]
    tool_deadline_armed: std::sync::Mutex<Option<test_sync::ToolDeadlineArmedSignal>>,
    /// Test-only control point after cancellation wins the foreground
    /// physical-result arbitration and before the late physical future is
    /// awaited. It makes the cancellation linearization observable without
    /// changing production scheduling or adding a runtime event.
    #[cfg(test)]
    tool_cancellation_settlement_pause:
        std::sync::Mutex<Option<test_sync::ToolCancellationSettlementPause>>,
    /// Test-only control point immediately after the physical-result branch
    /// wins foreground settlement arbitration. It lets a test request attempt
    /// cancellation after the physical result is authoritative, before any
    /// post-arbitration normalization runs.
    #[cfg(test)]
    tool_physical_settlement_pause:
        std::sync::Mutex<Option<test_sync::ToolPhysicalSettlementPause>>,
    turn: u32,
    terminal_emitted: bool,
}

#[cfg(test)]
mod test_fixtures;
