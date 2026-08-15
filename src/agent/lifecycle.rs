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
//! generic cancellation checkpoint
//!     ↓ admission linearization
//! admit_context → Ledger + Surface → RequestSnapshot → provider request
//!
//! Assistant(ToolCall A, ToolCall B) committed
//!     ↓ execute, settle every CallSlot, commit ToolResult A then ToolResult B
//! tool batch is structurally settled
//!     ↓
//! ToolResultObserver (canonical ToolCall order, then identity order)
//!     ↓ validate count + content at the transaction boundary
//!     ↓ stamp the observer's registered producer identity
//! Agent-Loop-owned deferred buffer
//!     ↓ next Context Assembly → lane + provenance from the producer identity
//!     ↓ PreStepPolicy → admission
//! canonical User context, owned by whoever produced it
//! ```
//!
//! # Lifecycle timing is not semantic ownership
//!
//! The left column above is **timing**, owned by the Agent Loop: when a
//! proposal becomes eligible. It is deliberately silent about **who owns the
//! fact**. A deferred proposal is stamped with the trusted contributor
//! identity its observer was *registered* under, and Context Assembly derives
//! the lane, the `UserSource`, and the `ContextKind` from that identity using
//! the same table it applies to that owner's request-time proposals.
//!
//! A certified extension (Issue #58) therefore produces deferred post-tool
//! context while keeping its extension identity, its extension provenance, and
//! its own lane. Nothing is rewritten into native runtime context merely
//! because a tool batch happened to precede it.
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
//!   identity;
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
//! speak for another. Observers are therefore registered under the same
//! [`ContextContributorIdentity`] vocabulary Context Assembly already uses,
//! and the deferred ordering key is
//! `(canonical ToolCall batch position, producer identity, proposal FIFO)`.
//! There is no priority number, no registration-order term, and no new
//! ordering model — only the identity order that already existed.
//!
//! # Seams that are intentionally absent
//!
//! - **`PreToolPolicy` / pre-dispatch `Allow`/`Deny`**: no concrete native
//!   consumer exists. `ToolRegistry::preflight` already owns canonical
//!   identity resolution, reserved-metadata stripping, and business argument
//!   validation, and nothing in the native tool plane needs a second gate.
//! - **`ToolExecutionWrapper` / around-dispatch middleware**: no concrete
//!   native requirement exists that the two seams above cannot express.
//! - **Post-tool result replacement/blocking**: `ToolResultObservation` is
//!   immutable by construction. A finalized result is a canonical fact by the
//!   time an observer sees it.
//! - **`Ask`/human approval** (Issue #64), **subagent lifecycle** (Issue
//!   #60), and **turn-stopping/forced continuation**: each needs a real
//!   native owner first.
//!
//! [`AgentExecutionObserver`](super::AgentExecutionObserver) is a different
//! responsibility and stays a read-only projection observer of committed
//! facts; it is not a policy or a deferred-context producer.

use std::sync::Arc;

use futures_util::future::BoxFuture;

use crate::context::{AcceptedContext, ContextProposal};
use crate::conversation::SurfaceRevision;
use crate::runtime::identity::{
    AttemptId, CertifiedExtensionIdentity, ContextContributorIdentity, ConversationId,
    NativeContextContributor, ToolCallId, ToolId,
};
use crate::tools::types::{ToolExecutionResult, ToolInvocationMode, ToolOrigin};

/// The bounded failure of one lifecycle extension invocation.
///
/// A lifecycle extension reports only a diagnostic. It never selects the
/// attempt's terminal outcome: the Agent Loop maps a pre-step failure to
/// [`RuntimeError::PreStepPolicyFailed`] and an observation failure to
/// [`RuntimeError::ToolResultObservationFailed`], and settles the attempt
/// with exactly one terminal event.
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
/// Agent Status and Skill guidance, certified-extension proposals, and any
/// deferred post-tool observation context staged by the previous tool batch.
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
    /// The Surface revision the proposals were assembled against. No context
    /// has been admitted yet, so this is still the pre-admission revision.
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
    /// A rejection is observed strictly before the admission linearization
    /// point, so no proposed dynamic context is committed, the Surface does
    /// not advance because of the proposals, no `RequestSnapshot` is frozen,
    /// and no provider request begins.
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
/// pending, the evaluation settles and the Agent Loop's own generic
/// pre-admission cancellation checkpoint still decides admission — exactly
/// like a pending `ContextContributor` future in Issue #55.
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

/// The bounded number of deferred proposals one observer may return for one
/// settled call.
///
/// The Agent Loop checks this at the observer transaction boundary, before a
/// single proposal is staged, so an observer can never allocate an unbounded
/// proposal set into the deferred buffer and have it rejected one step later.
pub const MAX_DEFERRED_PROPOSALS_PER_OBSERVATION: usize = 16;

/// The typed immutable tool-result observation seam of one attempt.
///
/// The observer runs once per settled call, in canonical `ToolCall` batch
/// order, **after** the complete owning batch has reached structural
/// settlement. It may return zero or more bounded transient
/// [`ContextProposal`]s; the Agent Loop validates them at the transaction
/// boundary, stamps its registered producer identity onto each, and stages
/// them. They become canonical only if the next Context Assembly, the pre-step
/// policy, and the admission boundary all accept them.
///
/// # What an observer does not decide
///
/// An observer returns *content*, never *semantics*. It cannot select a
/// [`UserSource`](crate::message::types::UserSource), a
/// [`ContextKind`](crate::message::types::ContextKind), a semantic lane, or a
/// contributor identity: those are derived by Context Assembly from the
/// trusted identity this observer was registered under. It also may not mutate
/// the result, reject or undo it, create a second `ToolMessage`, dispatch
/// another tool, start a model request, own cancellation, change the terminal
/// outcome, or touch the Ledger/Surface.
pub trait ToolResultObserver: Send + Sync {
    /// Observes one finalized tool outcome and proposes bounded deferred
    /// context.
    ///
    /// # Errors
    ///
    /// Returns a [`LifecycleError`] when the observer cannot complete its
    /// bounded observation. The already-committed Assistant `ToolCall`
    /// message and its complete canonical `ToolMessage` batch are unaffected;
    /// the Agent Loop discards every proposal of the failed pass and settles
    /// the attempt with exactly one terminal event.
    fn observe_tool_result<'a>(
        &'a self,
        observation: &'a ToolResultObservation<'a>,
    ) -> BoxFuture<'a, Result<Vec<ContextProposal>, LifecycleError>>;
}

/// The identity tool-result observer: no deferred context is ever produced.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoDeferredContext;

impl ToolResultObserver for NoDeferredContext {
    fn observe_tool_result<'a>(
        &'a self,
        _observation: &'a ToolResultObservation<'a>,
    ) -> BoxFuture<'a, Result<Vec<ContextProposal>, LifecycleError>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// One tool-result observer bound to the trusted identity it speaks for.
///
/// The identity comes from registration, never from the observer's return
/// value, and it is the only ordering key between observers. Registration
/// order is therefore not observable.
#[derive(Clone)]
pub struct RegisteredToolResultObserver {
    identity: ContextContributorIdentity,
    observer: Arc<dyn ToolResultObserver>,
}

impl core::fmt::Debug for RegisteredToolResultObserver {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RegisteredToolResultObserver")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl RegisteredToolResultObserver {
    /// The trusted semantic owner this observer produces deferred context for.
    #[must_use]
    pub const fn identity(&self) -> &ContextContributorIdentity {
        &self.identity
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
                    .map(RegisteredToolResultObserver::identity)
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
            tool_results: Vec::new(),
        }
    }

    /// Replaces the attempt's single pre-step policy owner.
    #[must_use]
    pub fn with_pre_step_policy(mut self, policy: Arc<dyn PreStepPolicy>) -> Self {
        self.pre_step = policy;
        self
    }

    /// Registers the observer that speaks for the **native** runtime
    /// observation owner.
    ///
    /// Its deferred proposals receive native runtime provenance because of
    /// *this registration*, not because they were produced after a tool batch.
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
        self.with_tool_result_observer(
            ContextContributorIdentity::Native(NativeContextContributor::ToolResultObservation),
            observer,
        )
    }

    /// Registers an observer that speaks for one **certified extension**.
    ///
    /// Its deferred proposals keep that extension's identity and provenance
    /// through Context Assembly, exactly like the same extension's
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
        self.with_tool_result_observer(
            ContextContributorIdentity::CertifiedExtension(identity),
            observer,
        )
    }

    /// Registers one deferred-context producer under a trusted identity.
    ///
    /// # Errors
    ///
    /// Returns a [`LifecycleError`] when the identity is already registered,
    /// or when it names a semantic owner that publishes no context at all.
    pub fn with_tool_result_observer(
        mut self,
        identity: ContextContributorIdentity,
        observer: Arc<dyn ToolResultObserver>,
    ) -> Result<Self, LifecycleError> {
        if self
            .tool_results
            .iter()
            .any(|registered| registered.identity == identity)
        {
            return Err(LifecycleError::new(format!(
                "deferred-context producer {identity:?} already has a tool-result observer"
            )));
        }
        self.tool_results
            .push(RegisteredToolResultObserver { identity, observer });
        // The registered set is kept in logical identity order, so the
        // deferred ordering key never contains a registration-order term.
        self.tool_results
            .sort_by(|left, right| left.identity.cmp(&right.identity));
        Ok(self)
    }

    /// The attempt's pre-step policy owner.
    #[must_use]
    pub fn pre_step_policy(&self) -> Arc<dyn PreStepPolicy> {
        Arc::clone(&self.pre_step)
    }

    /// The attempt's registered deferred-context producers, in logical
    /// identity order.
    #[must_use]
    pub fn tool_result_observers(&self) -> &[RegisteredToolResultObserver] {
        &self.tool_results
    }
}
