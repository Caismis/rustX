//! Construction of one attempt execution: the authority binding between the
//! admitted request, its capability lease, the conversation tool runtime, and
//! the frozen runtime policy, plus the test-only policy overrides installed
//! before the execution runs.

use super::start::RequestMeasurements;
use super::step::LogicalStep;
#[cfg(test)]
use super::test_sync;
use super::{AgentExecution, AgentExecutionRequest, AgentExecutionRuntimePolicy};
use crate::agent::cancellation::AgentCancellation;
use crate::agent::lifecycle::AttemptLifecycle;
use crate::agent::observer::AgentExecutionObserver;
use crate::agent::state::ExecutionStateMachine;
use crate::capabilities::AttemptCapabilityLease;
use crate::context::ContextRuntime;
use crate::durable::{ConversationStore, ConversationStoreError};
#[cfg(test)]
use crate::model::deadline::ModelTimeoutPolicy;
use crate::model::generation::GenerationSafetyPolicy;
use crate::publication::CoalescePolicy;
#[cfg(test)]
use crate::runtime::MonotonicClock;
use crate::runtime::inbound::MailboxError;
use crate::tools::runtime::ConversationToolRuntime;

/// The runtime generation-safety policy of one attempt, resolved once from
/// the attempt's frozen model invocation.
///
/// The *decision* about the bounds belongs to the invocation layer, which
/// already resolves the effective output budget; this loop only freezes the
/// answer, exactly like the timeout policy beside it: one attempt, one
/// policy, and no live model state is read again while the attempt runs.
#[must_use]
fn frozen_generation_policy(request: &AgentExecutionRequest) -> GenerationSafetyPolicy {
    request.model.primary().generation_safety_policy()
}

impl<'a> AgentExecution<'a> {
    /// Creates an attempt execution over the given adapter, the owned attempt
    /// capability lease, the cancellation signal, the mandatory M4 context
    /// runtime, the explicitly admitted runtime execution policy, and the
    /// conversation tool runtime.
    ///
    /// The attempt capability lease is moved into the execution and pins the
    /// immutable capability snapshot (revision, `ToolRegistry` handle, Skill
    /// catalog, environment identities, and the effective `ToolEnvironment`)
    /// for the complete lifetime of this attempt: every model/tool cycle
    /// inside the attempt uses exactly that snapshot and never re-discovers
    /// Skills. Constructor failure drops the owned lease, and successful
    /// execution settlement drops it with the consumed execution. The
    /// execution cannot be constructed without a lease — there is no
    /// capability-free constructor.
    ///
    /// The conversation tool runtime binds the conversation identity, the
    /// canonical inbound mailbox, and the background registry together:
    /// the attempt must belong to the same conversation, otherwise the
    /// execution is rejected structurally. The loop drains exactly
    /// `tool_runtime.mailbox()` at every safe boundary, so background
    /// terminal notifications always enter the mailbox the Agent Loop
    /// drains.
    ///
    /// The runtime policy is required: the composition owner must supply the
    /// same admitted [`AgentExecutionRuntimePolicy`] to the primary execution
    /// boundary that it supplied to the provider-backed summarizer. This
    /// constructor never creates a policy or monotonic clock of its own.
    ///
    /// The context runtime is required: there is exactly one normal
    /// execution model — canonical history is always projected through the
    /// context engine. A pending `FreshInbound` turn offers the optional
    /// Agent Status opportunity; disabled or empty modules simply contribute
    /// no status message. There is no no-context mode.
    ///
    /// The [`AttemptLifecycle`] is required for the same reason: the loop
    /// always evaluates exactly one pre-step policy and always runs exactly
    /// one tool-result observation pass, so no code path branches on whether
    /// a seam is attached. [`AttemptLifecycle::inert`] is the identity
    /// configuration.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::ConversationMismatch`] when the request's
    /// conversation differs from the conversation tool runtime's
    /// conversation (and therefore its canonical mailbox).
    pub(crate) fn new(
        request: AgentExecutionRequest,
        capability: AttemptCapabilityLease,
        cancellation: &'a AgentCancellation,
        runtime_policy: AgentExecutionRuntimePolicy,
        context_runtime: ContextRuntime,
        tool_runtime: &'a ConversationToolRuntime,
        lifecycle: AttemptLifecycle,
    ) -> Result<Self, MailboxError> {
        if tool_runtime.conversation_id() != &request.conversation_id {
            return Err(MailboxError::ConversationMismatch {
                expected: request.conversation_id.clone(),
                actual: tool_runtime.conversation_id().clone(),
            });
        }
        let store = tool_runtime.durable_store();
        Self::new_bound(
            request,
            capability,
            cancellation,
            context_runtime,
            runtime_policy,
            tool_runtime,
            store,
            lifecycle,
        )
    }

    /// Completes construction over the full authority obtained from the
    /// conversation tool runtime's single composition binding. This helper
    /// is private so callers cannot pair an arbitrary store with a mailbox.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::ConversationMismatch`] when the request and
    /// tool runtime belong to different conversations, and
    /// [`MailboxError::Durable`] when the supplied authority belongs to a
    /// different conversation, cannot load its current head, or cannot
    /// initialize the standalone fixture history.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one explicit authority-binding boundary
    fn new_bound(
        mut request: AgentExecutionRequest,
        capability: AttemptCapabilityLease,
        cancellation: &'a AgentCancellation,
        mut context_runtime: ContextRuntime,
        runtime_policy: AgentExecutionRuntimePolicy,
        tool_runtime: &'a ConversationToolRuntime,
        store: std::sync::Arc<dyn ConversationStore>,
        lifecycle: AttemptLifecycle,
    ) -> Result<Self, MailboxError> {
        if tool_runtime.conversation_id() != &request.conversation_id {
            return Err(MailboxError::ConversationMismatch {
                expected: request.conversation_id.clone(),
                actual: tool_runtime.conversation_id().clone(),
            });
        }
        if store.conversation_id() != &request.conversation_id {
            return Err(MailboxError::Durable(
                crate::durable::ConversationStoreError::ConversationIdMismatch {
                    stored: store.conversation_id().clone(),
                    requested: request.conversation_id.clone(),
                },
            ));
        }
        context_runtime.engine.set_image_projection(
            tool_runtime.artifacts().clone(),
            request
                .model
                .primary()
                .capabilities()
                .input_modalities
                .contains(&crate::model::catalog::Modality::Image),
        );
        #[cfg(test)]
        {
            context_runtime.monotonic_clock = runtime_policy.monotonic_clock.clone();
            if !context_runtime.summarizer.has_backend() {
                let backend = crate::runtime::auxiliary::executor::AuxiliaryExecutor::for_test(
                    store.clone(),
                    runtime_policy.monotonic_clock.clone(),
                )
                .map_err(|error| {
                    MailboxError::Durable(ConversationStoreError::Storage(error.message))
                })?;
                context_runtime.summarizer.bind_backend(
                    std::sync::Arc::new(backend),
                    context_runtime.resource_revision.get(),
                );
            }
        }
        let snapshot = capability.snapshot();
        let lifecycle = context_runtime
            .native_composition
            .bind(
                &mut context_runtime.assembly,
                snapshot
                    .resolved_profile()
                    .map(|profile| &profile.extensions),
                tool_runtime,
                lifecycle,
            )
            .map_err(|error| {
                MailboxError::Durable(ConversationStoreError::InvalidReference(error.to_string()))
            })?;
        if snapshot.conversation_id() != tool_runtime.conversation_id()
            || snapshot.workspace_root() != tool_runtime.workspace().root()
        {
            return Err(MailboxError::CapabilityOwnershipMismatch {
                capability_conversation: snapshot.conversation_id().clone(),
                runtime_conversation: tool_runtime.conversation_id().clone(),
                capability_workspace: snapshot.workspace_root().to_path_buf(),
                runtime_workspace: tool_runtime.workspace().root().to_path_buf(),
            });
        }
        let conversation = core::mem::take(&mut request.conversation);
        // Standalone fixtures may provide a fresh store rather than
        // constructing a ConversationRuntime first; initialize it once from
        // the supplied current facts (normal construction already did).
        if !store
            .is_initialized()
            .map_err(|error| MailboxError::Durable(error.clone()))?
        {
            store
                .initialize(&conversation.ledger().audit_records())
                .map_err(MailboxError::Durable)?;
        }
        let admitted_tools = std::sync::Arc::new(
            capability
                .snapshot()
                .tool_registry()
                .for_model(request.model.primary().capabilities()),
        );
        Ok(Self {
            admitted_tools,
            conversation,
            generation_policy: frozen_generation_policy(&request),
            request,
            capability,
            cancellation,
            tool_runtime,
            store,
            state: ExecutionStateMachine::new(),
            durable_failure: None,
            durable_failure_kind: None,
            pending_continuation: None,
            continuation_owner: None,
            continuation_source: None,
            pending_fresh_inbound: None,
            goal_creation_authorization: std::sync::Mutex::new(None),
            schedule_authorization: std::sync::Mutex::new(None),
            pending_post_tool_batch: None,
            context_runtime,
            subagent_context: runtime_policy.subagent_context,
            workflow_output: runtime_policy.workflow_output,
            workflow_output_feedback: None,
            lifecycle,
            logical_step: LogicalStep::begin(None),
            deferred_context: Vec::new(),
            context_generation_serial: 0,
            measurements: RequestMeasurements::default(),
            last_request_id: None,
            generation_timing: None,
            publication_policy: CoalescePolicy::default(),
            monotonic_clock: runtime_policy.monotonic_clock,
            model_timeout_policy: runtime_policy.model_timeout_policy,
            tool_deadline_policy: runtime_policy.tool_deadline_policy,
            publication: None,
            publication_settlement_failed: false,
            observer: None,
            #[cfg(test)]
            pauses: test_sync::ExecutionPauses::default(),
            turn: 0,
            terminal_emitted: false,
        })
    }

    /// Attaches the live observation seam.
    ///
    /// The observer is read-only: it observes emitted runtime facts,
    /// committed canonical messages, and composed Agent Status at their
    /// commit linearization points, and it never influences execution. It
    /// must be attached before [`AgentExecution::run`] to observe the whole
    /// attempt; attaching is optional, and the durable Event Journal remains
    /// available for historical reads regardless.
    pub fn observe(&mut self, observer: &'a dyn AgentExecutionObserver) {
        self.observer = Some(observer);
    }

    /// Installs the deterministic publication policy and clock of one attempt
    /// (Issue #108 regressions).
    ///
    /// Production always uses the default bounded policy and the monotonic
    /// system clock. A test installs an explicit byte threshold and a manually
    /// advanced clock so a coalescing or latency regression is decided by the
    /// policy alone and never by wall-clock timing.
    #[cfg(test)]
    pub(crate) fn install_publication_policy(
        &mut self,
        policy: CoalescePolicy,
        clock: std::sync::Arc<dyn MonotonicClock>,
    ) {
        self.publication_policy = policy;
        self.monotonic_clock = clock;
    }

    /// Installs the deterministic request deadline policy used by this
    /// execution's actual primary requests.
    #[cfg(test)]
    pub(crate) fn install_model_timeout_policy(&mut self, policy: ModelTimeoutPolicy) {
        self.model_timeout_policy = policy;
    }

    /// Installs the resolved generation-safety policy of one attempt.
    ///
    /// Production resolves the policy at the model-invocation layer. A test
    /// installs an explicit one so a budget regression is decided by the
    /// policy alone, without streaming a quarter of a megabyte through the
    /// publication and durability planes, and so a policy whose two bounds
    /// are unrelated can be exercised directly.
    #[cfg(test)]
    pub(crate) fn install_generation_policy(&mut self, policy: GenerationSafetyPolicy) {
        self.generation_policy = policy;
    }
}

#[cfg(test)]
mod tests;
