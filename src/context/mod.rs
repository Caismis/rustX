//! The M4 context plane: deterministic context assembly, token accounting,
//! compaction, checkpoints, and provider-context compilation.
//!
//! The core invariant is:
//!
//! ```text
//! Canonical history is durable truth.
//! Context is a deterministic projection of that truth.
//! Compaction changes the projection, never canonical history.
//! ```
//!
//! No provider SDK or wire type exists in this module: the engine projects
//! canonical context, and adapters decide how that canonical context is
//! encoded on the wire. [`ContextRuntime`] bundles the engine, the summary
//! service, and the checkpoint store for `AgentExecution`.

pub mod checkpoint;
pub mod engine;
pub mod error;
pub mod projection;
pub mod structure;
pub mod summarizer;
pub mod tokens;

use std::sync::Arc;

use crate::model::adapter::ModelAdapter;

pub use checkpoint::{
    ContextBoundary, ContextCheckpoint, ContextCheckpointStore, InMemoryCheckpointStore,
    summary_message_id,
};
pub use engine::{CompactionPlan, ContextConfig, ContextEngine};
pub use error::{ContextError, ContextErrorKind};
pub use projection::{ContextProjection, ProjectionItem, compile_projection};
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
/// The bundle owns the deterministic engine, the summary service, and the
/// checkpoint store; `AgentExecution` owns the integration point
/// (`with_context_runtime`). The summary service and checkpoint store are
/// shared (cheaply clonable) so one store can be reused across attempts of
/// one conversation.
pub struct ContextRuntime<'a> {
    /// The deterministic context engine.
    pub engine: ContextEngine,
    /// The provider-neutral summary service.
    pub summarizer: Arc<dyn ContextSummarizer + 'a>,
    /// The checkpoint persistence abstraction.
    pub checkpoint_store: Arc<dyn ContextCheckpointStore>,
}

impl<'a> ContextRuntime<'a> {
    /// Creates a context runtime bundle.
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
