//! Finite Trace value vocabulary and the immutable request source encoding at
//! the native request commit. Reads use indexed byte ranges; the Runtime
//! Client's Trace reader pages these values and never reconstructs them.

pub use types::TraceContributionPresentation;
pub mod bounds;
pub mod content;
pub mod frozen;
pub mod generation;
pub mod request;
pub mod types;

pub use bounds::identity_fits;
pub use bounds::{
    TRACE_DETAIL_BYTES, TRACE_PAGE_BYTES, TRACE_RECORD_BYTES, TRACE_SUMMARY_CONTEXT,
    TRACE_SUMMARY_CONTEXT_BYTES, TraceJson, TracePreview, TraceText,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use frozen::request_detail as frozen_request_detail;
pub use frozen::request_outcome as frozen_request_outcome;
pub use types::{
    TraceArtifact, TraceContentBlock, TraceContextKind, TraceContextPresentation,
    TraceContextSource, TraceCursor, TraceDetail, TraceFieldKind, TraceFieldSource,
    TraceGeneration, TraceGenerationTimeline, TraceInputProvenance, TraceKind, TraceLifecycle,
    TraceLocation, TraceManagedOutput, TraceMessageDetail, TraceMessageRole, TracePage,
    TraceRecord, TraceRequestDetail, TraceRequestFailure, TraceRequestMessage, TraceRequestOption,
    TraceRequestOutcome, TraceRequestPredecessor, TraceRequestSummary, TraceState,
    TraceSystemPromptPresentation, TraceSystemPromptState, TraceTiming, TraceToolCall,
    TraceToolCatalogState, TraceToolDefinition, TraceToolDetail, TraceToolLifecycle,
    TraceToolOutcome, TraceToolOutcomeUpdate, TraceToolResult, TraceToolSource, TraceToolSummary,
    TraceToolTruncation,
};
