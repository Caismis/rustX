//! Typed attempt lifecycle interception (Issue #56).
//!
//! The Agent Loop stays the lifecycle owner. This module adds exactly two
//! phase-specific typed seams, each carrying only the authority its phase
//! justifies:
//!
//! ```text
//! Context Assembly
//!     ↓ final immutable AcceptedContext
//! PreStepPolicy                       Enter | Reject(reason)
//!     ↓
//! staging (scratch validation — no durable effect)
//!     ↓
//! cancellation-vs-start arbitration   ← the one linearization point (M9b)
//!     ↓
//! commit_model_turn_start (Ledger + Surface
//! + RequestSnapshot + ModelRequestStarted, one transaction) → provider request
//!
//! Assistant(ToolCall A, ToolCall B) committed
//!     ↓ ToolRegistry preflight is already complete
//! PreToolPolicy → Allow | Deny(reason) | Ask(reason)
//!     ↓ on Ask: ConversationRuntime-owned InteractionCoordinator
//!     ↓ cancellation/start frontier
//! exact original PreparedInvocation → executor (or denied result slot)
//!     ↓ execute, settle every CallSlot, commit ToolResult A then ToolResult B
//! tool batch is structurally settled
//!     ↓
//! cancellation checkpoint  ← before each observer, and again once it settles
//!     ↓
//! ToolResultObserver (canonical ToolCall order, then producer order)
//!     ↓ validate count + content at the transaction boundary
//!     ↓ stamp the observer's bound producer reference
//! Agent-Loop-owned deferred buffer
//!     ↓ next Context Assembly → resolve producer → lane + provenance
//!     ↓ PreStepPolicy → admission
//! canonical User context, owned by whoever produced it
//! ```
//!
//! # Lifecycle timing is not semantic ownership
//!
//! The left column above is **timing**, owned by the Agent Loop: when a
//! proposal becomes eligible. It is deliberately silent about **who owns the
//! fact**. A deferred proposal is stamped with the producer its observer was
//! *bound* to, and Context Assembly resolves that reference against its own
//! registrations before deriving the lane, the `UserSource`, and the
//! `ContextKind` — using the same table it applies to that owner's
//! request-time proposals.
//!
//! A certified extension (Issue #58) therefore produces deferred post-tool
//! context while keeping its extension identity, its extension provenance, and
//! its own lane. Nothing is rewritten into native runtime context merely
//! because a tool batch happened to precede it.
//!
//! # Binding is not admission
//!
//! This module can *bind behavior* to a semantic owner. It can never
//! *establish* one. `ContextAssembly::register_extension` is the single
//! semantic admission authority: it validates the logical key, rejects
//! native-reserved keys, and records the attestation. Binding an observer with
//! [`AttemptLifecycle::with_extension_tool_result_observer`] only says "run
//! this code for that owner"; if the attempt's Context Assembly does not know
//! the key, the deferred proposals are rejected, no
//! [`UserSource::Extension`](crate::message::types::UserSource) is assigned,
//! and no generation is synthesized. There is exactly one place where an
//! extension becomes trusted.
//!
//! # Authority
//!
//! Neither seam receives mutable runtime state. Concretely, a lifecycle
//! extension can never:
//!
//! - mutate arbitrary Agent Loop state;
//! - append to the Message Ledger or advance the Conversation Surface;
//! - allocate a `MessageId` or commit a canonical message;
//! - mutate a `ToolCallId`, `ToolId`, tool name, or tool arguments (an
//!   observer reads the validated invocation arguments, and holds them only
//!   after the result is already canonical);
//! - choose its own `UserSource`, `ContextKind`, semantic lane, or contributor
//!   identity, or make itself a trusted extension by naming one;
//! - contribute to the Effective System Prompt from the post-tool phase;
//! - mutate a finalized `ToolExecutionResult` or produce a second one;
//! - own, replace, or observe-and-convert the attempt cancellation signal;
//! - issue, retry, or suppress a provider request;
//! - decide the attempt's terminal outcome.
//!
//! Both seams are values on one **required** immutable [`AttemptLifecycle`]
//! configuration. [`AttemptLifecycle::inert`] is the identity configuration:
//! its policy always enters and it has no registered observers, so no deferred
//! context is produced. There is no optional/`Option`-shaped branch whose mere
//! presence changes ordering or cancellation semantics, and there is no hook
//! chain, middleware, or around-dispatch wrapper.
//!
//! # Owners, not a chain
//!
//! The pre-step phase has exactly one owner per attempt. A chain of policies
//! would need a second ordering model purely to sequence hook implementations,
//! and rustX has no consumer that needs several independent admission
//! decisions; composition belongs inside a single implementation.
//!
//! The tool-result phase is different, because its output is *context*, and
//! context already has a rustX-owned identity ordering (Issue #55). A native
//! runtime owner and one or more certified extensions can each own deferred
//! context about the same settled call without any of them being able to
//! speak for another, short-circuit another, or replace another's result.
//! Observers are bound to a [`DeferredContextProducer`], at most one per
//! semantic owner, and the deferred ordering key is
//! `(canonical ToolCall batch position, producer identity, proposal FIFO)`.
//! There is no priority number, no registration-order term, and no new
//! ordering model — only the identity order that already existed.
//!
//! # Cancellation precedence
//!
//! Cancellation ownership stays entirely with the Agent Loop. An observer
//! already in flight is allowed to settle — it is never dropped mid-flight
//! just to implement this rule — but observable cancellation is checked
//! *before* each observer starts and *again* once it settles, before its
//! return value is consumed. So once cancellation is observable, no later
//! observer starts, and neither an observer's success nor its failure can
//! decide the terminal outcome.
//!
//! # Pre-tool ownership
//!
//! `PreToolPolicy` is the one typed pre-tool owner. It runs after
//! `ToolRegistry::preflight` and after the Assistant `ToolCall` is canonical,
//! but before the corresponding executor starts. Its view is immutable and
//! contains only the facts the registry already resolved. An `Ask` decision
//! is handed to the attempt's concrete native interaction binding; it never
//! grants a tool-start capability and it never carries replacement arguments.
//!
//! # Seams that are intentionally absent
//!
//! - **`ToolExecutionWrapper` / around-dispatch middleware**: no concrete
//!   native requirement exists that the two seams above cannot express.
//! - **Post-tool result replacement/blocking**: `ToolResultObservation` is
//!   immutable by construction. A finalized result is a canonical fact by the
//!   time an observer sees it.
//! - **Generic forms/workflows**, **subagent lifecycle** (Issue #60), and
//!   **turn-stopping/forced continuation**: each remains outside this bounded
//!   pre-tool seam. The native `ask_user` Tool owns its Questionnaire interaction
//!   through the normal Tool Plane instead of adding an Agent Loop branch.
//!
//! [`AgentExecutionObserver`](super::AgentExecutionObserver) is a different
//! responsibility and stays a read-only projection observer of committed
//! facts; it is not a policy or a deferred-context producer.

use std::sync::Arc;

use futures_util::future::BoxFuture;

use crate::context::{AcceptedContext, DeferredContextProducer, UserMessageProposal};
use crate::conversation::SurfaceRevision;
use crate::runtime::cancellation::ExecutionCancellation;
use crate::runtime::identity::{
    AttemptId, CertifiedExtensionIdentity, ConversationId, ToolCallId, ToolId,
};
#[cfg(test)]
use crate::runtime::interaction::TestInteractionRendezvous;
use crate::runtime::interaction::{
    ApprovalFacts, InteractionCoordinator, InteractionOutcome, QuestionnaireRequester,
};
use crate::runtime::types::ApprovalMode;
use crate::tools::types::{
    ToolApprovalPolicy, ToolExecutionResult, ToolInvocationMode, ToolOrigin,
};

/// The bounded failure of one lifecycle extension invocation.
///
/// A lifecycle extension reports only a diagnostic. It never selects the
/// attempt's terminal outcome: the Agent Loop maps a pre-step failure to
/// [`RuntimeError::PreStepPolicyFailed`] and an observation failure to
/// [`RuntimeError::ToolResultObservationFailed`], and settles the attempt
/// with the attempt terminal settlement; successful durable publication
/// commits the corresponding terminal event.
///
/// [`RuntimeError::PreStepPolicyFailed`]: crate::runtime::types::RuntimeError::PreStepPolicyFailed
/// [`RuntimeError::ToolResultObservationFailed`]: crate::runtime::types::RuntimeError::ToolResultObservationFailed
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleError {
    /// The bounded diagnostic message.
    pub message: String,
}

impl LifecycleError {
    /// Creates a lifecycle failure with the given diagnostic.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl core::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LifecycleError {}

/// The final immutable description of one primary step's proposal batch.
///
/// This is the *complete* batch that would otherwise be admitted: native
/// Agent Status, the native Skill system section, certified-extension
/// proposals, and any deferred post-tool observation context staged by the
/// previous tool batch.
/// No contributor and no observer has a path around this evaluation.
///
/// Every field is an immutable borrow of an already-validated transient
/// value. There is deliberately no way to rewrite, extend, or replace the
/// batch: a policy that could synthesize an unrelated replacement batch would
/// be a second context authority.
#[derive(Debug)]
pub struct PreStepBatch<'a> {
    /// The attempt proposing the model step.
    pub attempt_id: &'a AttemptId,
    /// The owning conversation.
    pub conversation_id: &'a ConversationId,
    /// The primary model turn number of the proposed step.
    pub turn: u32,
    /// The Surface revision the proposals were assembled against. Nothing
    /// from this batch is committed yet, so this is the pre-start revision.
    pub surface_revision: SurfaceRevision,
    /// The validated transient context batch, with rustX-assigned lanes,
    /// provenance, and contributor generation.
    pub context: &'a AcceptedContext,
}

/// The bounded decision of one pre-step policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreStepDecision {
    /// Admit the proposal batch and start the model step.
    Enter,
    /// Do not start the model step.
    ///
    /// A rejection is observed strictly before the start arbitration, and
    /// staging has no durable effect, so no proposed dynamic context is
    /// committed, the Surface does not advance because of the proposals, no
    /// `RequestSnapshot` is frozen, and no provider request begins.
    Reject {
        /// The bounded reason reported by the attempt's terminal event.
        reason: String,
    },
}

/// The typed pre-step policy seam of one attempt.
///
/// A policy observes the final immutable proposal batch and returns
/// [`PreStepDecision`]. It cannot allocate `MessageId`s, append Ledger facts,
/// advance the Surface, own cancellation, issue or retry a model request,
/// mutate Agent Loop state, mutate a `ToolCall`, or dispatch a tool.
///
/// Evaluation is awaited, and the policy is given no cancellation handle: if
/// attempt cancellation becomes observable while a bounded evaluation is
/// pending, the evaluation settles and the Agent Loop's own start
/// arbitration still decides — exactly like a pending `ContextContributor`
/// future in Issue #55.
pub trait PreStepPolicy: Send + Sync {
    /// Evaluates one final proposal batch.
    ///
    /// # Errors
    ///
    /// Returns a [`LifecycleError`] when the policy cannot produce a bounded
    /// decision. The Agent Loop then settles the attempt before admission, so
    /// no partial context admission exists.
    fn evaluate<'a>(
        &'a self,
        batch: &'a PreStepBatch<'a>,
    ) -> BoxFuture<'a, Result<PreStepDecision, LifecycleError>>;
}

/// The identity pre-step policy: every batch enters.
///
/// This is the behavior of an attempt with no configured policy; it exists so
/// the lifecycle configuration is required rather than optional.
#[derive(Debug, Default, Clone, Copy)]
pub struct AlwaysEnter;

impl PreStepPolicy for AlwaysEnter {
    fn evaluate<'a>(
        &'a self,
        _batch: &'a PreStepBatch<'a>,
    ) -> BoxFuture<'a, Result<PreStepDecision, LifecycleError>> {
        Box::pin(async { Ok(PreStepDecision::Enter) })
    }
}

/// The immutable facts presented to the one pre-tool policy owner.
///
/// The view is borrowed from the original `ToolCall` and the exact
/// `PreparedInvocation` produced by `ToolRegistry::preflight`. It carries no
/// mutable Agent Loop state, canonical mutation handle, cancellation handle,
/// executor, or replacement-invocation channel.
#[derive(Debug)]
pub struct PreToolView<'a> {
    /// The owning conversation.
    pub conversation_id: &'a ConversationId,
    /// The current attempt.
    pub attempt_id: &'a AttemptId,
    /// The primary model turn that issued the call.
    pub turn: u32,
    /// The canonical model-issued call identity.
    pub call_id: &'a ToolCallId,
    /// The registry-resolved tool identity.
    pub tool_id: &'a ToolId,
    /// The registry-resolved model-facing name.
    pub tool_name: &'a str,
    /// The registry-resolved typed origin.
    pub origin: &'a ToolOrigin,
    /// The registry-resolved execution mode.
    pub mode: ToolInvocationMode,
    /// The schema-validated business arguments.
    pub arguments: &'a serde_json::Value,
    /// The exact model-issued arguments of the canonical `ToolCall`, before
    /// reserved-metadata stripping and normalization.
    ///
    /// This is the value the Message Ledger owns by value, so it is the one an
    /// approval audit subject can pin verifiably. It is descriptive data only;
    /// a policy never receives a channel to replace it.
    pub canonical_arguments: &'a serde_json::Value,
    /// The tool-owned approval policy resolved by preflight.
    pub approval_policy: ToolApprovalPolicy,
}

impl PreToolView<'_> {
    /// Copies the immutable view into the coordinator's owned approval facts.
    ///
    /// The copy is made by the semantic owner, not by client input. The
    /// response path has no corresponding arguments field.
    #[must_use]
    pub(crate) fn approval_facts(&self, reason: impl Into<String>) -> ApprovalFacts {
        ApprovalFacts {
            turn: self.turn,
            call_id: self.call_id.clone(),
            tool_id: self.tool_id.clone(),
            tool_name: self.tool_name.to_owned(),
            origin: self.origin.clone(),
            mode: self.mode,
            arguments: self.arguments.clone(),
            canonical_arguments: self.canonical_arguments.clone(),
            reason: reason.into(),
        }
    }
}

/// The only production interaction binding an attempt may carry.
///
/// `Native` is installed by `ConversationRuntime`, which owns the concrete
/// coordinator.  The test-only variant exists only for deterministic Agent
/// Loop fixtures; it is not compiled into production and cannot become a
/// second runtime interaction authority.
#[derive(Clone)]
enum InteractionBinding {
    Unavailable,
    Native(Arc<InteractionCoordinator>),
    #[cfg(test)]
    Test(Arc<dyn TestInteractionRendezvous>),
}

impl InteractionBinding {
    async fn request_approval(
        &self,
        attempt_id: AttemptId,
        facts: ApprovalFacts,
        cancellation: ExecutionCancellation,
    ) -> InteractionOutcome {
        match self {
            Self::Unavailable => InteractionOutcome::Unavailable,
            Self::Native(coordinator) => {
                coordinator
                    .request_approval(attempt_id, facts, cancellation)
                    .await
            }
            #[cfg(test)]
            Self::Test(rendezvous) => rendezvous.request_approval(facts, cancellation).await,
        }
    }

    fn native_questionnaire_requester(
        &self,
        attempt_id: AttemptId,
        cancellation: ExecutionCancellation,
        turn: u32,
    ) -> Option<QuestionnaireRequester> {
        match self {
            Self::Native(coordinator) => Some(QuestionnaireRequester::new(
                Arc::clone(coordinator),
                attempt_id,
                cancellation,
                turn,
            )),
            Self::Unavailable => None,
            #[cfg(test)]
            Self::Test(_) => None,
        }
    }
}

/// The finite decision of the one pre-tool policy owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreToolDecision {
    /// Continue to the existing cancellation/tool-start frontier.
    Allow,
    /// Do not invoke the executor; settle one typed denied result slot.
    Deny {
        /// The bounded policy reason.
        reason: String,
    },
    /// Ask the conversation-owned interaction rendezvous. The request facts
    /// are constructed from the immutable view after this decision; policy
    /// code cannot replace tool identity or arguments.
    Ask {
        /// The bounded explanation shown to the client.
        reason: String,
    },
}

/// The required typed pre-tool policy seam of one attempt.
pub trait PreToolPolicy: Send + Sync {
    /// Evaluates one already-preflighted invocation.
    fn evaluate<'a>(
        &'a self,
        view: &'a PreToolView<'a>,
    ) -> BoxFuture<'a, Result<PreToolDecision, LifecycleError>>;
}

/// The runtime-owned effective approval evaluator.
///
/// This policy only selects whether the already-preflighted invocation enters
/// the existing approval rendezvous. It never changes availability, arguments,
/// execution ownership, or concurrency.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConfiguredApprovalPolicy {
    mode: ApprovalMode,
}

impl ConfiguredApprovalPolicy {
    pub(crate) const fn new(mode: ApprovalMode) -> Self {
        Self { mode }
    }
}

impl PreToolPolicy for ConfiguredApprovalPolicy {
    fn evaluate<'a>(
        &'a self,
        view: &'a PreToolView<'a>,
    ) -> BoxFuture<'a, Result<PreToolDecision, LifecycleError>> {
        Box::pin(async move {
            if self.mode == ApprovalMode::FullAccess
                || view.approval_policy == ToolApprovalPolicy::Never
            {
                Ok(PreToolDecision::Allow)
            } else {
                Ok(PreToolDecision::Ask {
                    reason: "tool approval policy requires approval".to_owned(),
                })
            }
        })
    }
}

/// The immutable invocation facts of one call that reached invocation
/// resolution.
///
/// This is a read-only copy of what the registry actually resolved and
/// validated, taken from the same [`PreparedInvocation`] that executed. It
/// carries no authority: an observer holds it after the result is already
/// canonical, and there is no handle through which the invocation could be
/// re-run, rewritten, or replayed.
///
/// # Why the arguments belong here
///
/// A result alone under-determines the fact it describes. The native Read
/// capability returns file *content*; the *path* exists only in the validated
/// invocation arguments. Without them a consumer would have to re-read
/// canonical history, parse Assistant messages, or keep a duplicate
/// invocation index beside the loop — three ways of building a second,
/// drifting authority for a fact the loop already owns.
///
/// # What is deliberately absent
///
/// - The **model-facing tool name**. Capability recognition is a typed
///   identity question (`tool_id` + `origin`); an MCP or Python tool publicly
///   named `read` must never be classified as the native rustX Read
///   capability, and leaving the name out makes that structural.
/// - The **raw provider payload**. `arguments` is the stripped, schema-
///   validated business argument value: the reserved `__rustx_*` invocation
///   metadata has already been removed by preflight, and no wire-format
///   envelope is exposed.
///
/// [`PreparedInvocation`]: crate::tools::executor::PreparedInvocation
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedToolInvocation {
    /// The canonical registry-resolved tool identity that executed.
    pub tool_id: ToolId,
    /// The canonical registry-resolved typed origin of the tool.
    pub origin: ToolOrigin,
    /// The runtime-resolved execution ownership of this invocation.
    ///
    /// A [`ToolInvocationMode::Background`] invocation reports an **accepted
    /// background dispatch**, not the completion of the detached work. An
    /// observer must not infer an external side effect from it.
    pub mode: ToolInvocationMode,
    /// The stripped business arguments, exactly as validated against the
    /// canonical schema before execution.
    pub arguments: serde_json::Value,
}

/// One immutable finalized tool outcome of a structurally settled batch.
///
/// The observation is a borrow of facts that are already canonical: the
/// result has been normalized, every sibling call slot has settled, and the
/// complete `ToolMessage` batch has been committed in canonical call order.
/// There is no interior mutability and no owning handle, so an observer
/// cannot change the finalized result, produce a second result, rewrite the
/// Assistant `ToolCall`, or dispatch further work.
#[derive(Debug)]
pub struct ToolResultObservation<'a> {
    /// The attempt that executed the batch.
    pub attempt_id: &'a AttemptId,
    /// The owning conversation.
    pub conversation_id: &'a ConversationId,
    /// The primary model turn that issued the batch.
    pub turn: u32,
    /// The canonical position of this call inside its owning tool batch,
    /// counted in original model call order. This is the primary key of the
    /// deferred-context ordering rule and is never derived from completion
    /// timing.
    pub batch_position: usize,
    /// The canonical model-issued call identity.
    pub call_id: &'a ToolCallId,
    /// The canonical registry-resolved tool identity.
    pub tool_id: &'a ToolId,
    /// The canonical registry-resolved typed origin of the tool.
    ///
    /// Together with `tool_id` this is the *only* supported way to recognize
    /// a capability. The model-facing tool name is deliberately absent from
    /// this observation: an MCP or Python tool whose public name happens to
    /// be `read` must never be mistaken for the native rustX Read
    /// capability, and leaving the name out makes that discipline
    /// structural rather than advisory.
    pub origin: &'a ToolOrigin,
    /// The immutable invocation facts of the call, when it reached invocation
    /// resolution.
    ///
    /// `None` means the call never produced a canonical invocation: preflight
    /// rejected it (invalid reserved invocation metadata or a business schema
    /// violation) and its result slot is the deterministic rejection. A
    /// rejected call therefore exposes no invocation arguments, because none
    /// were ever validated — `tool_id` and `origin` above still identify the
    /// resolved capability.
    pub invocation: Option<&'a ObservedToolInvocation>,
    /// The finalized normalized result exactly as committed to canonical
    /// history.
    pub result: &'a ToolExecutionResult,
}

/// The typed immutable tool-result observation seam of one attempt.
///
/// The observer runs once per settled call, in canonical `ToolCall` batch
/// order, **after** the complete owning batch has reached structural
/// settlement. It may return zero or more bounded transient
/// [`UserMessageProposal`]s; the Agent Loop validates them at the transaction
/// boundary, stamps its registered producer reference onto each, and stages
/// them. They become canonical only if the next Context Assembly, the pre-step
/// policy, and the admission boundary all accept them.
///
/// # User context only
///
/// The return type is deliberately not the full
/// [`ContextProposal`](crate::context::ContextProposal) vocabulary. A settled
/// tool batch is a conversational fact, and the only concrete requirement is
/// deferred conversational context, so this seam cannot mutate the Effective
/// System Prompt on the following turn. System sections stay owned by the
/// request-time [`ContextContributor`](crate::context::ContextContributor)
/// path. The restriction is a type, not a runtime check: a deferred system
/// section is unrepresentable.
///
/// # What an observer does not decide
///
/// An observer returns *content*, never *semantics*. It cannot select a
/// [`UserSource`](crate::message::types::UserSource), a
/// [`ContextKind`](crate::message::types::ContextKind), a semantic lane, or a
/// contributor identity: Context Assembly derives those after resolving the
/// producer this observer was registered under against its own registrations.
/// It also may not mutate the result, reject or undo it, create a second
/// `ToolMessage`, dispatch another tool, start a model request, own
/// cancellation, change the terminal outcome, or touch the Ledger/Surface.
pub trait ToolResultObserver: Send + Sync {
    /// Observes one finalized tool outcome and proposes bounded deferred
    /// User context.
    ///
    /// # Errors
    ///
    /// Returns a [`LifecycleError`] when the observer cannot complete its
    /// bounded observation. The already-committed Assistant `ToolCall`
    /// message and its complete canonical `ToolMessage` batch are unaffected;
    /// the Agent Loop discards every proposal of the failed pass and settles
    /// the attempt; the terminal event is published only after its durable
    /// append succeeds.
    fn observe_tool_result<'a>(
        &'a self,
        observation: &'a ToolResultObservation<'a>,
    ) -> BoxFuture<'a, Result<Vec<UserMessageProposal>, LifecycleError>>;
}

/// The identity tool-result observer: no deferred context is ever produced.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoDeferredContext;

impl ToolResultObserver for NoDeferredContext {
    fn observe_tool_result<'a>(
        &'a self,
        _observation: &'a ToolResultObservation<'a>,
    ) -> BoxFuture<'a, Result<Vec<UserMessageProposal>, LifecycleError>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// One tool-result observer bound to the semantic owner it speaks for.
///
/// The producer reference comes from registration, never from the observer's
/// return value, and it is the only ordering key between observers, so
/// registration order is not observable. Binding is not admission: Context
/// Assembly still resolves the reference against its own registrations before
/// any provenance is assigned.
#[derive(Clone)]
pub struct RegisteredToolResultObserver {
    producer: DeferredContextProducer,
    observer: Arc<dyn ToolResultObserver>,
}

impl core::fmt::Debug for RegisteredToolResultObserver {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RegisteredToolResultObserver")
            .field("producer", &self.producer)
            .finish_non_exhaustive()
    }
}

impl RegisteredToolResultObserver {
    /// The semantic owner this observer produces deferred context for.
    #[must_use]
    pub const fn producer(&self) -> &DeferredContextProducer {
        &self.producer
    }

    /// The observer implementation.
    #[must_use]
    pub fn observer(&self) -> Arc<dyn ToolResultObserver> {
        Arc::clone(&self.observer)
    }
}

/// The one required immutable lifecycle configuration of an attempt.
///
/// Every `AgentExecution` is constructed with exactly one of these. The
/// identity configuration ([`AttemptLifecycle::inert`]) preserves the
/// pre-#56 behavior exactly: every batch enters and no deferred context is
/// produced. Because the configuration is required and total, no code path
/// branches on "is a hook attached?", so attaching a seam cannot change
/// ordering, cancellation, or settlement semantics.
#[derive(Clone)]
pub struct AttemptLifecycle {
    pre_step: Arc<dyn PreStepPolicy>,
    pre_tool: Arc<dyn PreToolPolicy>,
    interaction: InteractionBinding,
    tool_results: Vec<RegisteredToolResultObserver>,
}

impl core::fmt::Debug for AttemptLifecycle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AttemptLifecycle")
            .field(
                "tool_result_observers",
                &self
                    .tool_results
                    .iter()
                    .map(RegisteredToolResultObserver::producer)
                    .collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl Default for AttemptLifecycle {
    fn default() -> Self {
        Self::inert()
    }
}

impl AttemptLifecycle {
    /// The identity configuration: enter every step, defer no context.
    #[must_use]
    pub fn inert() -> Self {
        Self {
            pre_step: Arc::new(AlwaysEnter),
            pre_tool: Arc::new(ConfiguredApprovalPolicy::new(ApprovalMode::Policy)),
            interaction: InteractionBinding::Unavailable,
            tool_results: Vec::new(),
        }
    }

    /// Replaces the attempt's single pre-step policy owner.
    #[must_use]
    pub fn with_pre_step_policy(mut self, policy: Arc<dyn PreStepPolicy>) -> Self {
        self.pre_step = policy;
        self
    }

    /// Replaces the attempt's single pre-tool policy owner.
    #[must_use]
    pub fn with_pre_tool_policy(mut self, policy: Arc<dyn PreToolPolicy>) -> Self {
        self.pre_tool = policy;
        self
    }

    /// Applies the runtime's effective approval mode to the built-in
    /// pre-tool policy. Test/custom policies can still replace this seam
    /// explicitly with [`Self::with_pre_tool_policy`].
    pub(crate) fn with_approval_mode(mut self, mode: ApprovalMode) -> Self {
        self.pre_tool = Arc::new(ConfiguredApprovalPolicy::new(mode));
        self
    }

    /// Binds the conversation-owned native interaction coordinator used by an
    /// `Ask` decision. Only the runtime owner can install this production
    /// binding; callers cannot replace it with another rendezvous.
    pub(crate) fn with_native_interaction(
        mut self,
        coordinator: Arc<InteractionCoordinator>,
    ) -> Self {
        self.interaction = InteractionBinding::Native(coordinator);
        self
    }

    /// Installs a test-only waiter seam for deterministic Agent Loop tests.
    /// This is deliberately absent from production builds.
    #[cfg(test)]
    pub(crate) fn with_test_interaction_rendezvous(
        mut self,
        rendezvous: Arc<dyn TestInteractionRendezvous>,
    ) -> Self {
        self.interaction = InteractionBinding::Test(rendezvous);
        self
    }

    /// Requests approval through the attempt's runtime-owned binding.
    pub(crate) async fn request_approval(
        &self,
        attempt_id: AttemptId,
        facts: ApprovalFacts,
        cancellation: ExecutionCancellation,
    ) -> InteractionOutcome {
        self.interaction
            .request_approval(attempt_id, facts, cancellation)
            .await
    }

    /// Binds the one native Questionnaire capability for a foreground invocation.
    /// The returned value is crate-private and carries only a read-only
    /// cancellation view; it never exposes the attempt cancellation owner.
    pub(crate) fn native_questionnaire_requester(
        &self,
        attempt_id: AttemptId,
        cancellation: ExecutionCancellation,
        turn: u32,
    ) -> Option<QuestionnaireRequester> {
        self.interaction
            .native_questionnaire_requester(attempt_id, cancellation, turn)
    }

    /// Binds the observer that speaks for the **native** runtime observation
    /// owner.
    ///
    /// rustX owns this semantic owner, so no registration elsewhere is needed
    /// and its deferred proposals receive native runtime provenance. That
    /// follows from *whose observer this is*, not from the fact that the
    /// proposals were produced after a tool batch.
    ///
    /// # Errors
    ///
    /// Returns a [`LifecycleError`] when the native owner already has an
    /// observer: a semantic owner is single-owner exactly as in Context
    /// Assembly.
    pub fn with_native_tool_result_observer(
        self,
        observer: Arc<dyn ToolResultObserver>,
    ) -> Result<Self, LifecycleError> {
        self.bind_tool_result_observer(DeferredContextProducer::NativeRuntimeObservation, observer)
    }

    /// Binds an observer that speaks for one **certified extension**.
    ///
    /// This is a *behavior binding*, not an admission. The extension must be
    /// registered with the attempt's
    /// [`ContextAssembly`](crate::context::ContextAssembly), which is the one
    /// semantic admission authority; binding an observer here proves nothing
    /// about the extension. If Context Assembly does not know the key when the
    /// deferred proposals are assembled, they are rejected and no extension
    /// provenance is ever assigned.
    ///
    /// For a registered extension the deferred proposals keep that extension's
    /// identity, provenance, and registered attestation, exactly like its
    /// request-time proposals. Post-tool timing never converts them into
    /// native runtime context.
    ///
    /// # Errors
    ///
    /// Returns a [`LifecycleError`] when the extension already has an
    /// observer.
    pub fn with_extension_tool_result_observer(
        self,
        identity: CertifiedExtensionIdentity,
        observer: Arc<dyn ToolResultObserver>,
    ) -> Result<Self, LifecycleError> {
        self.bind_tool_result_observer(
            DeferredContextProducer::CertifiedExtension { identity },
            observer,
        )
    }

    /// Binds one observer to one semantic owner.
    ///
    /// Deliberately private: a public generic binder taking an arbitrary
    /// contributor identity would let a caller name any semantic owner, which
    /// would read like a second registry even though Context Assembly still
    /// has the final say. The two narrow constructors above are the whole
    /// surface.
    fn bind_tool_result_observer(
        mut self,
        producer: DeferredContextProducer,
        observer: Arc<dyn ToolResultObserver>,
    ) -> Result<Self, LifecycleError> {
        if self
            .tool_results
            .iter()
            .any(|registered| registered.producer == producer)
        {
            return Err(LifecycleError::new(format!(
                "deferred-context producer {producer:?} already has a tool-result observer"
            )));
        }
        self.tool_results
            .push(RegisteredToolResultObserver { producer, observer });
        // The bound set is kept in logical producer order, so the deferred
        // ordering key never contains a registration-order term.
        self.tool_results
            .sort_by(|left, right| left.producer.cmp(&right.producer));
        Ok(self)
    }

    /// The attempt's pre-step policy owner.
    #[must_use]
    pub fn pre_step_policy(&self) -> Arc<dyn PreStepPolicy> {
        Arc::clone(&self.pre_step)
    }

    /// The attempt's one pre-tool policy owner.
    #[must_use]
    pub fn pre_tool_policy(&self) -> Arc<dyn PreToolPolicy> {
        Arc::clone(&self.pre_tool)
    }

    /// The attempt's bound deferred-context observers, in logical producer
    /// order.
    #[must_use]
    pub fn tool_result_observers(&self) -> &[RegisteredToolResultObserver] {
        &self.tool_results
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfiguredApprovalPolicy, PreToolDecision, PreToolPolicy, PreToolView};
    use crate::runtime::ApprovalMode;
    use crate::runtime::identity::{AttemptId, ConversationId, ToolCallId, ToolId};
    use crate::tools::types::{ToolApprovalPolicy, ToolInvocationMode, ToolOrigin};

    fn view<'a>(
        conversation_id: &'a ConversationId,
        attempt_id: &'a AttemptId,
        call_id: &'a ToolCallId,
        tool_id: &'a ToolId,
        origin: &'a ToolOrigin,
        arguments: &'a serde_json::Value,
        approval_policy: ToolApprovalPolicy,
    ) -> PreToolView<'a> {
        PreToolView {
            conversation_id,
            attempt_id,
            turn: 1,
            call_id,
            tool_id,
            tool_name: "write",
            origin,
            mode: ToolInvocationMode::Foreground,
            arguments,
            canonical_arguments: arguments,
            approval_policy,
        }
    }

    #[tokio::test]
    async fn approval_mode_changes_only_the_effective_approval_decision() {
        let conversation_id = ConversationId::new("approval-policy-conversation");
        let attempt_id = AttemptId::new("approval-policy-attempt");
        let call_id = ToolCallId::new("approval-policy-call");
        let tool_id = ToolId::new("tool-write");
        let origin = ToolOrigin::Builtin;
        let arguments = serde_json::json!({"path": "same.txt", "content": "same"});

        let never = view(
            &conversation_id,
            &attempt_id,
            &call_id,
            &tool_id,
            &origin,
            &arguments,
            ToolApprovalPolicy::Never,
        );
        assert!(matches!(
            ConfiguredApprovalPolicy::new(ApprovalMode::Policy)
                .evaluate(&never)
                .await
                .expect("policy evaluation"),
            PreToolDecision::Allow
        ));

        let always = view(
            &conversation_id,
            &attempt_id,
            &call_id,
            &tool_id,
            &origin,
            &arguments,
            ToolApprovalPolicy::Always,
        );
        assert!(matches!(
            ConfiguredApprovalPolicy::new(ApprovalMode::Policy)
                .evaluate(&always)
                .await
                .expect("policy evaluation"),
            PreToolDecision::Ask { .. }
        ));
        assert!(matches!(
            ConfiguredApprovalPolicy::new(ApprovalMode::FullAccess)
                .evaluate(&always)
                .await
                .expect("full access evaluation"),
            PreToolDecision::Allow
        ));
        assert_eq!(always.arguments, &arguments);
        assert_eq!(always.approval_policy, ToolApprovalPolicy::Always);
    }
}
