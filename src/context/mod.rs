//! The context plane: deterministic finite context assembly, token
//! accounting, compaction planning, Agent Status composition, and the
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
//! No provider SDK or wire type exists in this module: the engine projects
//! canonical context, the Agent Status composer produces structured status
//! sections and a deterministic renderer produces the attachment text, and
//! adapters decide how that canonical context is encoded on the wire.
//! [`ContextRuntime`] bundles the engine, the summary service, and the Agent
//! Status composer for `AgentExecution`.

pub mod engine;
pub mod error;
pub mod projection;
pub mod status;
pub mod summarizer;
pub mod tokens;

use std::sync::Arc;

use crate::model::session::AttemptModelSnapshot;

pub use engine::{
    CompactionBudgets, CompactionConstraints, CompactionPlan, ContextConfig, ContextEngine,
    SessionContextPolicy,
};
pub use error::{ContextError, ContextErrorKind};
pub use projection::ContextProjection;
pub use status::{
    AgentStatus, AgentStatusClock, AgentStatusComposer, AgentStatusCompositionError,
    AgentStatusFact, AgentStatusRenderContext, AgentStatusSection, AgentStatusSectionData,
    AgentStatusSectionId, AgentStatusSectionProvider, SystemClock, render_agent_status,
};
pub use summarizer::{ContextSummarizer, ModelBackedSummarizer, SummaryRequest};
pub use tokens::{
    ClosureTokenEstimator, DefaultTokenEstimator, ProviderObservedInput, TokenEstimator,
    bytes_to_tokens,
};

/// The context runtime bundle handed to an `AgentExecution`.
///
/// The bundle owns the deterministic engine, the summary service, and the
/// Agent Status composer; `AgentExecution` owns the integration point and
/// the attempt's [`ConversationState`](crate::conversation::ConversationState).
/// There is deliberately no checkpoint store: compaction lineage is derived
/// from Conversation Surface history, so no second store can drift from it.
pub struct ContextRuntime {
    /// The deterministic context engine, configured for this attempt's
    /// model context window.
    pub(crate) engine: ContextEngine,
    /// The provider-neutral summary service.
    pub(crate) summarizer: Arc<dyn ContextSummarizer>,
    /// The Agent Status composer: the structured status sections and the
    /// deterministic renderer that produces the ephemeral attachment. Agent
    /// Status is mandatory for rustX agents and owned by the context plane.
    pub(crate) status_composer: AgentStatusComposer,
    /// The primary/summary output budgets and the summary input limit,
    /// frozen at attempt admission.
    pub(crate) compaction_budgets: CompactionBudgets,
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
        status_composer: AgentStatusComposer,
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
        // therefore the summary model's own window minus its output budget;
        // the session's conversational safety reserve belongs to the primary
        // loop, not to this single request.
        let summary_input_limit = summary
            .context_window()
            .checked_sub(u64::from(summary.max_output_tokens()))
            .filter(|limit| *limit > 0)
            .ok_or_else(|| {
                ContextError::new(
                    ContextErrorKind::InvalidConfiguration,
                    format!(
                        "the summary model context window {} must exceed its output budget {}",
                        summary.context_window(),
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
            status_composer,
            compaction_budgets,
        })
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
        status_composer: AgentStatusComposer,
        compaction_budgets: CompactionBudgets,
    ) -> Self {
        Self {
            engine,
            summarizer,
            status_composer,
            compaction_budgets,
        }
    }
}
