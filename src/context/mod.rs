//! The context plane: deterministic finite context assembly, token
//! accounting, compaction planning, Agent Status generation, and the
//! provider-neutral model-context boundary.
//!
//! M7.5 (Issue #54) supersedes the M4 projection-only compaction model. The
//! canonical conversation model lives in [`crate::conversation`]; the
//! context plane is a consumer of it:
//!
//! ```text
//! Message Ledger        immutable committed facts
//!         ↓
//! Conversation Surface  active identity/order @ SurfaceRevision
//!         ↓
//! Context Assembly      bounded proposals + admission-ready context
//!         ↓
//! Context Engine        finite projection + token pressure + compaction
//!                       planning
//!         ↓
//! canonical User(Runtime / CompactionSummary) commit
//! + one complete-message Surface Replace
//! ```
//!
//! Compaction is no longer "a projection that hides history": it commits a
//! genuine canonical conversational fact and rewrites the Surface. Ledger
//! facts are never edited, deleted, or overwritten.
//!
//! No provider SDK or wire type exists in this module: Context Assembly settles
//! trusted semantic context, the engine projects the Surface, and adapters
//! receive the final Effective System Prompt.
//! [`ContextRuntime`] bundles the engine, the summary service, and the
//! attempt-owned Agent Status engine for `AgentExecution`.

pub mod assembly;
pub(crate) mod compaction;
pub mod engine;
pub mod error;
pub mod projection;
pub mod status;
pub mod summarizer;
pub mod tokens;

use std::sync::Arc;

use crate::model::session::AttemptModelSnapshot;

pub use crate::message::types::{AgentStatusGenerationMetadata, AgentStatusModuleId};
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
    CompactionBudgets, CompactionConstraints, CompactionPlan, ContextConfig, ContextEngine,
    EstimateCorrection, SessionContextPolicy,
};
pub use error::{ContextError, ContextErrorKind};
pub use projection::ContextProjection;
#[cfg(test)]
pub(crate) use status::AgentStatusTestSeam;
pub use status::{
    AgentStatus, AgentStatusClock, AgentStatusConfig, AgentStatusEngine, AgentStatusOpportunitySet,
    AgentStatusSection, AgentStatusSectionData, AgentStatusSectionId, AgentStatusSurfaceView,
    BACKGROUND_REMINDER_MESSAGE_INTERVAL, BackgroundStatusConfig, FreshInboundStatusOpportunity,
    GLOBAL_AGENT_STATUS_BYTE_CAP, MAX_BACKGROUND_STATUS_EXECUTIONS,
    MAX_BACKGROUND_STATUS_TEXT_BYTES, SystemClock, TIME_REFRESH_INTERVAL, TimeStatusConfig,
    render_agent_status,
};
pub use summarizer::{ContextSummarizer, ModelBackedSummarizer, SummaryModelInput, SummaryRequest};
pub use tokens::{
    ClosureTokenEstimator, DefaultTokenEstimator, ObservedAnchor, ProviderObservedInput,
    TokenEstimator, bytes_to_tokens, non_conversation_fingerprint, request_identity_fingerprint,
};

/// The context runtime bundle handed to an `AgentExecution`.
///
/// The bundle owns the deterministic engine, the summary service, and the
/// attempt-owned Agent Status engine; `AgentExecution` owns the integration point and
/// the attempt's [`ConversationState`](crate::conversation::ConversationState).
/// There is deliberately no separate summary store: compaction lineage is
/// derived from Conversation Surface history, so no second authority can
/// drift from it.
pub struct ContextRuntime {
    /// The deterministic context engine, configured for this attempt's
    /// model context window.
    pub(crate) engine: ContextEngine,
    /// The provider-neutral summary service.
    pub(crate) summarizer: Arc<dyn ContextSummarizer>,
    /// The attempt-owned closed Agent Status engine. It contains the
    /// compile-time module set and attempt-scoped quarantine state.
    pub(crate) status_engine: AgentStatusEngine,
    /// The one rustX-owned finite context-assembly contract. Extensions only
    /// receive immutable invocation snapshots through this value.
    pub(crate) assembly: ContextAssembly,
    /// The primary/summary output budgets and the summary input limit,
    /// frozen at attempt admission.
    pub(crate) compaction_budgets: CompactionBudgets,
    /// Static request-time System inputs frozen from the admitted resource
    /// generation. Agent Status is composed separately per primary request.
    pub(crate) native_system: NativeContextInput,
    /// Process-local resource generation that supplied `native_system`.
    pub(crate) resource_revision: crate::runtime::RuntimeResourceRevision,
}

impl ContextRuntime {
    /// Creates the production context runtime of one admitted attempt.
    ///
    /// The engine's context window comes from the attempt's **immutable
    /// model snapshot**, never from a window captured at process start, and
    /// the summarizer is derived from that same snapshot's frozen summary
    /// policy. The summary invocation's own window additionally bounds how
    /// large a selected Surface span may be, so compaction can never build
    /// an impossible summary-model request.
    ///
    /// # Errors
    ///
    /// Returns an engine construction error when the derived configuration
    /// leaves no positive effective input budget, for the primary model or
    /// for the summary model.
    pub fn for_attempt(
        policy: SessionContextPolicy,
        estimator: Arc<dyn TokenEstimator>,
        status_engine: AgentStatusEngine,
        model: &AttemptModelSnapshot,
    ) -> Result<Self, ContextError> {
        Self::for_attempt_with_assembly(
            policy,
            estimator,
            status_engine,
            ContextAssembly::new(),
            model,
        )
    }

    /// Creates a production context runtime with the supplied attempt-owned
    /// Agent Status engine.
    ///
    /// # Errors
    ///
    /// Returns a context configuration error when the primary or summary
    /// model cannot produce a valid context budget.
    pub fn for_attempt_with_assembly(
        policy: SessionContextPolicy,
        estimator: Arc<dyn TokenEstimator>,
        status_engine: AgentStatusEngine,
        assembly: ContextAssembly,
        model: &AttemptModelSnapshot,
    ) -> Result<Self, ContextError> {
        if policy.summary_output_cap == Some(0) {
            return Err(ContextError::new(
                ContextErrorKind::InvalidConfiguration,
                "summary_output_cap must be positive when present",
            ));
        }
        let engine = ContextEngine::new(
            policy.config_for_window(model.primary().context_window()),
            estimator,
        )?;
        let summary = match policy.summary_output_cap {
            Some(cap) => model.summary_invocation().with_output_cap(cap),
            None => model.summary_invocation().clone(),
        };
        // The summary request is a bounded one-off: no tools, no Agent
        // Status, no Skill catalog, no continuation. Its input bound is
        // derived with the same discipline as any other request of this
        // runtime — window minus reserve minus output budget — and never as
        // "the whole window". A summary input budget that spans almost the
        // entire window lets compaction assemble a request *larger* than the
        // one that just overflowed, so the recovery from a context overflow
        // overflows again. The reserve is what keeps the derived budget
        // honest against token-estimation error.
        let summary_input_limit = summary
            .context_window()
            .checked_sub(policy.reserve_tokens)
            .and_then(|remaining| remaining.checked_sub(u64::from(summary.max_output_tokens())))
            .filter(|limit| *limit > 0)
            .ok_or_else(|| {
                ContextError::new(
                    ContextErrorKind::InvalidConfiguration,
                    format!(
                        "the summary model context window {} must exceed reserve_tokens {} plus \
                         its output budget {}",
                        summary.context_window(),
                        policy.reserve_tokens,
                        summary.max_output_tokens()
                    ),
                )
            })?;
        let compaction_budgets = CompactionBudgets::new(
            model.primary().max_output_tokens(),
            summary.max_output_tokens(),
            summary_input_limit,
        );
        Ok(Self {
            engine,
            summarizer: Arc::new(ModelBackedSummarizer::new(summary)),
            status_engine,
            assembly,
            compaction_budgets,
            native_system: NativeContextInput::default(),
            resource_revision: crate::runtime::RuntimeResourceRevision::default(),
        })
    }

    /// Freezes one admitted resource generation into this attempt bundle.
    #[must_use]
    pub fn with_runtime_resources(
        mut self,
        resources: &crate::runtime::RuntimeResourceSnapshot,
    ) -> Self {
        self.native_system.workspace_instructions =
            resources.project_instructions().map(str::to_owned);
        self.native_system.skill_guidance = resources.skill_catalog().map(str::to_owned);
        self.native_system.agent_profile = resources.agent_profile().map(str::to_owned);
        self.resource_revision = resources.revision();
        self
    }

    /// The in-crate deterministic summarizer seam.
    ///
    /// It exists only under `cfg(test)` and is `pub(crate)`, so it is not
    /// part of the published API: [`ContextRuntime::for_attempt`] is the one
    /// constructor a consumer of this library can call, and it derives the
    /// summarizer from the attempt's frozen model snapshot.
    #[cfg(test)]
    pub(crate) fn with_scripted_summarizer(
        engine: ContextEngine,
        summarizer: Arc<dyn ContextSummarizer>,
        status_engine: AgentStatusEngine,
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
        status_engine: AgentStatusEngine,
        assembly: ContextAssembly,
        compaction_budgets: CompactionBudgets,
    ) -> Self {
        Self {
            engine,
            summarizer,
            status_engine,
            assembly,
            compaction_budgets,
            native_system: NativeContextInput::default(),
            resource_revision: crate::runtime::RuntimeResourceRevision::default(),
        }
    }
}
