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
//! ToolResultObserver (canonical ToolCall order)
//!     ↓ bounded transient proposals
//! Agent-Loop-owned deferred buffer
//!     ↓ next Context Assembly → PreStepPolicy → admission
//! canonical User(PostToolObservation) context
//! ```
//!
//! # Authority
//!
//! Neither seam receives mutable runtime state. Concretely, a lifecycle
//! extension can never:
//!
//! - mutate arbitrary Agent Loop state;
//! - append to the Message Ledger or advance the Conversation Surface;
//! - allocate a `MessageId` or commit a canonical message;
//! - mutate a `ToolCallId`, `ToolId`, tool name, or tool arguments;
//! - mutate a finalized `ToolExecutionResult` or produce a second one;
//! - own, replace, or observe-and-convert the attempt cancellation signal;
//! - issue, retry, or suppress a provider request;
//! - decide the attempt's terminal outcome.
//!
//! Both seams are values on one **required** immutable [`AttemptLifecycle`]
//! configuration. [`AttemptLifecycle::inert`] is the identity configuration:
//! its policy always enters and its observer always produces no deferred
//! context. There is no optional/`Option`-shaped branch whose mere presence
//! changes ordering or cancellation semantics, and there is no registry,
//! chain, middleware, or wrapper.
//!
//! # Why one composed owner per phase
//!
//! Each phase has exactly one owner per attempt, not a chain. A chain would
//! need a second deterministic ordering model (on top of the #55 contributor
//! lane/identity order) purely to sequence hook implementations, and rustX
//! has no native consumer that requires several independent policies or
//! observers to compose. Composition, when a consumer eventually needs it,
//! belongs inside that consumer's own single implementation, where it can be
//! ordered by its own domain identities. This keeps the deferred-context
//! ordering rule reducible to `(canonical ToolCall batch position, proposal
//! FIFO)` with no observer-identity term.
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

use crate::context::{AcceptedContext, UserMessageProposal};
use crate::conversation::SurfaceRevision;
use crate::runtime::identity::{AttemptId, ConversationId, ToolCallId, ToolId};
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
    /// The runtime-resolved execution ownership of the invocation, when the
    /// call reached invocation resolution.
    ///
    /// `None` means the call never produced a canonical invocation: preflight
    /// rejected it (invalid reserved invocation metadata or a business schema
    /// violation) and its result slot is the deterministic rejection.
    ///
    /// A [`ToolInvocationMode::Background`] observation reports an **accepted
    /// background dispatch**, not the completion of the detached work. An
    /// observer must not infer an external side effect from it.
    pub mode: Option<ToolInvocationMode>,
    /// The finalized normalized result exactly as committed to canonical
    /// history.
    pub result: &'a ToolExecutionResult,
}

/// The typed immutable tool-result observation seam of one attempt.
///
/// The observer runs once per settled call, in canonical `ToolCall` batch
/// order, **after** the complete owning batch has reached structural
/// settlement. It may return zero or more bounded transient
/// [`UserMessageProposal`]s; the Agent Loop stages them and they become
/// canonical only if the next Context Assembly, the pre-step policy, and the
/// admission boundary all accept them.
///
/// An observer may not mutate the result, reject or undo it, create a second
/// `ToolMessage`, dispatch another tool, start a model request, own
/// cancellation, change the terminal outcome, or touch the Ledger/Surface.
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
    tool_results: Arc<dyn ToolResultObserver>,
}

impl core::fmt::Debug for AttemptLifecycle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AttemptLifecycle").finish_non_exhaustive()
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
            tool_results: Arc::new(NoDeferredContext),
        }
    }

    /// Replaces the attempt's single pre-step policy owner.
    #[must_use]
    pub fn with_pre_step_policy(mut self, policy: Arc<dyn PreStepPolicy>) -> Self {
        self.pre_step = policy;
        self
    }

    /// Replaces the attempt's single tool-result observer owner.
    #[must_use]
    pub fn with_tool_result_observer(mut self, observer: Arc<dyn ToolResultObserver>) -> Self {
        self.tool_results = observer;
        self
    }

    /// The attempt's pre-step policy owner.
    #[must_use]
    pub fn pre_step_policy(&self) -> Arc<dyn PreStepPolicy> {
        Arc::clone(&self.pre_step)
    }

    /// The attempt's tool-result observer owner.
    #[must_use]
    pub fn tool_result_observer(&self) -> Arc<dyn ToolResultObserver> {
        Arc::clone(&self.tool_results)
    }
}
