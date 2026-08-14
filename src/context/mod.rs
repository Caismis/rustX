//! The M4 context plane: deterministic context assembly, token accounting,
//! compaction, checkpoints, Agent Status composition, and provider-context
//! compilation.
//!
//! The core invariant is:
//!
//! ```text
//! Canonical history is durable truth.
//! Context is a deterministic projection of that truth.
//! Compaction changes the projection, never canonical history.
//! Agent Status is an ephemeral projection of runtime facts, never history.
//! ```
//!
//! No provider SDK or wire type exists in this module: the engine projects
//! canonical context, the Agent Status composer produces structured status
//! sections and a deterministic renderer produces the attachment text, and
//! adapters decide how that canonical context is encoded on the wire.
//! [`ContextRuntime`] bundles the engine, the summary service, the Agent
//! Status composer, and the checkpoint store for `AgentExecution`.

pub mod checkpoint;
pub mod engine;
pub mod error;
pub mod projection;
pub mod status;
pub mod structure;
pub mod summarizer;
pub mod tokens;

use std::sync::Arc;

use crate::model::session::AttemptModelSnapshot;

pub use checkpoint::{
    ContextBoundary, ContextCheckpoint, ContextCheckpointStore, InMemoryCheckpointStore,
    summary_message_id,
};
pub use engine::{
    CompactionBudgets, CompactionConstraints, CompactionPlan, ContextConfig, ContextEngine,
    SessionContextPolicy,
};
pub use error::{ContextError, ContextErrorKind};
pub use projection::{CompiledContext, ContextProjection, ProjectionItem, compile_projection};
pub use status::{
    AgentStatus, AgentStatusClock, AgentStatusComposer, AgentStatusCompositionError,
    AgentStatusFact, AgentStatusRenderContext, AgentStatusSection, AgentStatusSectionData,
    AgentStatusSectionId, AgentStatusSectionProvider, SystemClock, render_agent_status,
};
pub use summarizer::{
    ContextSummarizer, ModelBackedSummarizer, SplitTurnSummaryInput, SummaryInputItem,
    SummaryRequest,
};
pub use tokens::{
    ClosureTokenEstimator, DefaultTokenEstimator, ProviderObservedInput, TokenEstimator,
    TokenMeasurement, TokenMeasurementSource, bytes_to_tokens,
};

/// The M4 context runtime bundle handed to an `AgentExecution`.
///
/// The bundle owns the deterministic engine, the summary service, the Agent
/// Status composer, and the checkpoint store; `AgentExecution` owns the
/// integration point. The summary service and checkpoint store are shared
/// (cheaply clonable) so one store can be reused across attempts of one
/// conversation.
pub struct ContextRuntime {
    /// The deterministic context engine, configured for this attempt's
    /// model context window.
    pub(crate) engine: ContextEngine,
    /// The provider-neutral summary service.
    pub(crate) summarizer: Arc<dyn ContextSummarizer>,
    /// The checkpoint persistence abstraction.
    pub(crate) checkpoint_store: Arc<dyn ContextCheckpointStore>,
    /// The Agent Status composer: the structured status sections and the
    /// deterministic renderer that produces the ephemeral attachment. Agent
    /// Status is mandatory for rustX agents and owned by the context plane.
    pub(crate) status_composer: AgentStatusComposer,
    /// The primary and summary output budgets frozen at attempt admission.
    pub(crate) compaction_budgets: CompactionBudgets,
}

impl ContextRuntime {
    /// Creates the production context runtime of one admitted attempt.
    ///
    /// The engine's context window comes from the attempt's **immutable
    /// model snapshot**, never from a window captured at process start, and
    /// the summarizer is derived from that same snapshot's frozen summary
    /// policy. There is deliberately no production path that supplies an
    /// unrelated summarizer beside the attempt's model.
    ///
    /// # Errors
    ///
    /// Returns an engine construction error when the derived configuration
    /// leaves no positive effective input budget.
    pub fn for_attempt(
        policy: SessionContextPolicy,
        estimator: Arc<dyn TokenEstimator>,
        checkpoint_store: Arc<dyn ContextCheckpointStore>,
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
        let compaction_budgets = CompactionBudgets::new(
            model.primary().max_output_tokens(),
            summary.max_output_tokens(),
        );
        Ok(Self {
            engine,
            summarizer: Arc::new(ModelBackedSummarizer::new(summary)),
            checkpoint_store,
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
        checkpoint_store: Arc<dyn ContextCheckpointStore>,
        status_composer: AgentStatusComposer,
        compaction_budgets: CompactionBudgets,
    ) -> Self {
        Self {
            engine,
            summarizer,
            checkpoint_store,
            status_composer,
            compaction_budgets,
        }
    }
}
