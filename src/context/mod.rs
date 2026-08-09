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

use crate::model::adapter::ModelAdapter;

pub use checkpoint::{
    ContextBoundary, ContextCheckpoint, ContextCheckpointStore, InMemoryCheckpointStore,
    summary_message_id,
};
pub use engine::{CompactionConstraints, CompactionPlan, ContextConfig, ContextEngine};
pub use error::{ContextError, ContextErrorKind};
pub use projection::{CompiledContext, ContextProjection, ProjectionItem, compile_projection};
pub use status::{
    AgentStatus, AgentStatusClock, AgentStatusComposer, AgentStatusCompositionError,
    AgentStatusFact, AgentStatusRenderContext, AgentStatusSection, AgentStatusSectionData,
    AgentStatusSectionId, AgentStatusSectionProvider, SystemClock, render_agent_status,
};
pub use summarizer::{
    ContextSummarizer, ModelBackedSummarizer, SplitTurnSummaryInput, SummaryInputItem,
    SummaryModelConfig, SummaryRequest,
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
pub struct ContextRuntime<'a> {
    /// The deterministic context engine.
    pub engine: ContextEngine,
    /// The provider-neutral summary service.
    pub summarizer: Arc<dyn ContextSummarizer + 'a>,
    /// The checkpoint persistence abstraction.
    pub checkpoint_store: Arc<dyn ContextCheckpointStore>,
    /// The Agent Status composer: the structured status sections and the
    /// deterministic renderer that produces the ephemeral attachment. Agent
    /// Status is mandatory for rustX agents and owned by the context plane.
    pub status_composer: AgentStatusComposer,
}

impl<'a> ContextRuntime<'a> {
    /// Creates a context runtime bundle with the default Agent Status
    /// composer (system clock, mandatory temporal section only).
    #[must_use]
    pub fn new(
        engine: ContextEngine,
        summarizer: Arc<dyn ContextSummarizer + 'a>,
        checkpoint_store: Arc<dyn ContextCheckpointStore>,
    ) -> Self {
        Self {
            engine,
            summarizer,
            checkpoint_store,
            status_composer: AgentStatusComposer::default(),
        }
    }

    /// Creates a context runtime bundle with an explicit Agent Status
    /// composer.
    #[must_use]
    pub fn with_status_composer(
        engine: ContextEngine,
        summarizer: Arc<dyn ContextSummarizer + 'a>,
        checkpoint_store: Arc<dyn ContextCheckpointStore>,
        status_composer: AgentStatusComposer,
    ) -> Self {
        Self {
            engine,
            summarizer,
            checkpoint_store,
            status_composer,
        }
    }

    /// Creates a production runtime bundle backed by the canonical model
    /// adapter: the model-backed summarizer shares the execution's adapter
    /// and model configuration.
    ///
    /// # Errors
    ///
    /// Returns an engine construction error for an impossible context
    /// configuration.
    pub fn model_backed(
        config: ContextConfig,
        estimator: Arc<dyn TokenEstimator>,
        adapter: &'a dyn ModelAdapter,
        summary_config: SummaryModelConfig,
        checkpoint_store: Arc<dyn ContextCheckpointStore>,
    ) -> Result<Self, ContextError> {
        let engine = ContextEngine::new(config, estimator)?;
        let summarizer = Arc::new(ModelBackedSummarizer::new(adapter, summary_config));
        Ok(Self::new(engine, summarizer, checkpoint_store))
    }
}
