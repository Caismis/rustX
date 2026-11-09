//! The context plane: bounded read-only resources, typed historical projections,
//! deterministic compilation, token accounting and compaction planning.
//!
//! Immutable Canonical History supplies committed facts. Durable checkpoint
//! activations select Original and Derived context. Compilation includes only
//! eligible Tail strictly after the checkpoint base. The Agent Loop adds current
//! core-owned input and freezes exact semantic model requests before dispatch.
//! Default Summary writes a Derived checkpoint item and never a canonical User.
//!
//! Context Assembly remains the sole admission path for dynamic contributors.
//! `ContextRuntime` bundles the engine, summary service and admitted Agent Status
//! contributor; execution owns timeout policy, cancellation and monotonic clocks.

pub mod assembly;
pub mod contribution;

pub mod checkpoint;
pub mod chunks;
pub(crate) mod compaction;
pub mod compaction_budget;
pub(crate) mod compaction_metadata;
pub mod engine;
pub mod error;
mod images;
pub mod occupancy;
pub(crate) mod policy;
pub mod projection;
pub mod resources;
pub mod status;
pub mod summarizer;
pub(crate) mod summary_content;
mod system_prompt;
pub mod tokens;

use std::sync::Arc;

use crate::model::deadline::ModelTimeoutPolicy;
use crate::model::session::AttemptModelSnapshot;
use crate::runtime::monotonic::MonotonicClock;

pub use crate::message::types::{
    AgentStatusGenerationMetadata, AgentStatusMetadataError, AgentStatusModuleId,
};
pub use assembly::{
    AcceptedContext, AcceptedSystemSection, AcceptedUserContext, CONTEXT_COMPATIBILITY_ABI_VERSION,
    ContextAssembly, ContextAssemblyError, ContextCompatibilityManifest, ContextContributor,
    ContextGeneration, ContextProposal, ContextProposalKind, ContributorGeneration,
    ContributorInputSnapshot, DeferredContextProducer, DeferredContextProposal,
    MAX_DEFERRED_CONTEXT_PROPOSALS, MAX_PROPOSALS_PER_CONTRIBUTOR, NativeContextInput,
    SystemSectionLane, UserContextLane, UserMessageProposal, render_effective_system_prompt,
    validate_user_message_proposal,
};
pub use engine::{
    CompactionBudgets, CompactionCarryover, CompactionConstraints, ContextConfig, ContextEngine,
    EstimateCorrection,
};
pub use error::{ContextError, ContextErrorKind};
pub use projection::ContextProjection;
#[cfg(test)]
pub(crate) use status::AgentStatusTestSeam;
pub use status::{
    AgentStatus, AgentStatusClock, AgentStatusConfig, AgentStatusEngine, AgentStatusSection,
    AgentStatusSectionData, AgentStatusSectionId, AgentStatusSurfaceView,
    AgentStatusSurfaceViewError, BACKGROUND_REMINDER_MESSAGE_INTERVAL, BackgroundStatusConfig,
    ContributionOpportunities, FreshInboundOpportunity, GLOBAL_AGENT_STATUS_BYTE_CAP,
    MAX_BACKGROUND_STATUS_EXECUTIONS, MAX_BACKGROUND_STATUS_TEXT_BYTES, PostToolBatchOpportunity,
    SystemClock, TIME_REFRESH_INTERVAL, TODO_STATUS_EMISSION_KEY,
    TODO_STATUS_REMINDER_PROGRESS_INTERVAL, TimeStatusConfig, effective_status_timezone,
    render_agent_status,
};
pub use summarizer::{ContextSummarizer, ModelBackedSummarizer, SummaryModelInput, SummaryRequest};
pub(crate) use system_prompt::DEFAULT_SYSTEM_PROMPT;
pub use tokens::{
    ClosureTokenEstimator, DefaultTokenEstimator, ObservedAnchor, ProviderObservedInput,
    TokenEstimator, bytes_to_tokens, model_input_fingerprint, non_conversation_fingerprint,
    request_identity_fingerprint,
};

/// The context runtime bundle handed to an `AgentExecution`.
///
/// The bundle owns the deterministic engine, the summary service, and the
/// frozen native capability binding; `AgentExecution` owns the integration point and
/// the attempt's [`ConversationState`](crate::conversation::ConversationState).
/// Independently durable typed checkpoint content and activations supply
/// historical projection authority; this bundle never owns canonical insertion.
pub struct ContextRuntime {
    /// Provider-facing primary model frozen with the context capacity.
    pub(crate) primary_model: String,
    /// The deterministic context engine, configured for this attempt's
    /// model context window.
    pub(crate) engine: ContextEngine,
    /// The provider-neutral summary service.
    pub(crate) summarizer: Arc<dyn ContextSummarizer>,
    /// Frozen composition bindings installed at Attempt admission, never a
    /// separate per-step preparation or commit pipeline.
    pub(crate) native_composition: crate::extensions::NativeContextComposition,
    /// The one rustX-owned finite context-assembly contract. Extensions only
    /// receive immutable invocation snapshots through this value.
    pub(crate) assembly: ContextAssembly,
    /// The primary/summary output budgets and the summary input limit,
    /// frozen at attempt admission.
    pub(crate) compaction_budgets: CompactionBudgets,
    pub(crate) maximum_summary_output: u32,
    /// Static request-time System inputs frozen from the admitted resource
    /// generation. Dynamic domain content enters through registered contributors.
    pub(crate) native_system: NativeContextInput,
    /// Process-local resource generation that supplied `native_system`.
    pub(crate) resource_revision: crate::runtime::RuntimeResourceRevision,
    pub(crate) monotonic_clock: Arc<dyn MonotonicClock>,
}

impl ContextRuntime {
    pub(crate) fn compaction_budget(
        &self,
        operation_id: crate::runtime::identity::CompactionOperationId,
    ) -> compaction_budget::CompactionBudget {
        compaction_budget::CompactionBudget::new(operation_id, self.monotonic_clock.clone())
    }
    /// Test-only convenience wrapper for the explicit production-capable
    /// assembly constructor. It requires the same admitted policy and clock
    /// that the sibling `AgentExecution` receives; it never creates either.
    #[cfg(test)]
    pub(crate) fn for_attempt(
        estimator: Arc<dyn TokenEstimator>,
        status_engine: Option<AgentStatusEngine>,
        model: &AttemptModelSnapshot,
        model_timeout_policy: ModelTimeoutPolicy,
        monotonic_clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, ContextError> {
        Self::for_attempt_with_assembly(
            estimator,
            status_engine,
            ContextAssembly::new(),
            model,
            model_timeout_policy,
            monotonic_clock,
        )
    }

    /// Creates a context runtime with the supplied attempt-owned Agent Status
    /// engine, context assembly, timeout policy, and runtime monotonic clock.
    ///
    /// # Errors
    ///
    /// Returns a context configuration error when the primary or summary
    /// model cannot produce a valid context budget.
    pub(crate) fn for_attempt_with_assembly(
        estimator: Arc<dyn TokenEstimator>,
        status_engine: Option<AgentStatusEngine>,
        assembly: ContextAssembly,
        model: &AttemptModelSnapshot,
        model_timeout_policy: ModelTimeoutPolicy,
        monotonic_clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, ContextError> {
        if !model_timeout_policy.is_positive() {
            return Err(ContextError::new(
                ContextErrorKind::InvalidConfiguration,
                "model request timeout policy values must be positive",
            ));
        }
        let policy = policy::CompactionPolicy::for_models(model)?;
        let engine = ContextEngine::new(policy.primary, estimator)?;
        let summary = model.summary_invocation().automatic_summary_base();
        let compaction_budgets = policy.budgets;
        Ok(Self {
            primary_model: model.primary().invocation_config().model,
            engine,
            summarizer: Arc::new(ModelBackedSummarizer::new(
                summary,
                model_timeout_policy,
                monotonic_clock.clone(),
            )),
            native_composition: crate::extensions::NativeContextComposition::new(status_engine),
            assembly,
            compaction_budgets,
            maximum_summary_output: policy.maximum_summary_output,
            native_system: NativeContextInput::default(),
            resource_revision: crate::runtime::RuntimeResourceRevision::default(),
            monotonic_clock,
        })
    }

    /// Freezes one admitted resource generation into this attempt bundle.
    #[must_use]
    pub fn with_runtime_resources(
        mut self,
        resources: &crate::runtime::RuntimeResourceSnapshot,
    ) -> Self {
        self.native_system = resources.native_system_input();
        self.resource_revision = resources.revision();
        self
    }

    /// The in-crate deterministic summarizer seam.
    ///
    /// It exists only under `cfg(test)` and is `pub(crate)`, so it is not
    /// part of the published API. Production-capable construction is owned
    /// by `ConversationRuntime`, which supplies the explicit admitted policy
    /// and shared monotonic clock to
    /// [`ContextRuntime::for_attempt_with_assembly`].
    #[cfg(test)]
    pub(crate) fn with_scripted_summarizer(
        engine: ContextEngine,
        summarizer: Arc<dyn ContextSummarizer>,
        status_engine: Option<AgentStatusEngine>,
        compaction_budgets: CompactionBudgets,
    ) -> Self {
        Self::with_scripted_summarizer_and_assembly(
            engine,
            summarizer,
            status_engine,
            ContextAssembly::new(),
            compaction_budgets,
        )
    }

    /// Test-only constructor with an explicit attempt-owned status engine.
    #[cfg(test)]
    pub(crate) fn with_scripted_summarizer_and_assembly(
        engine: ContextEngine,
        summarizer: Arc<dyn ContextSummarizer>,
        status_engine: Option<AgentStatusEngine>,
        assembly: ContextAssembly,
        compaction_budgets: CompactionBudgets,
    ) -> Self {
        Self {
            primary_model: "scripted".to_owned(),
            engine,
            summarizer,
            native_composition: crate::extensions::NativeContextComposition::new(status_engine),
            assembly,
            compaction_budgets,
            maximum_summary_output: compaction_budgets.summary_output_budget,
            native_system: NativeContextInput::default(),
            resource_revision: crate::runtime::RuntimeResourceRevision::default(),
            monotonic_clock: Arc::new(crate::runtime::monotonic::SystemMonotonicClock::new()),
        }
    }
}

pub(crate) mod planning;
