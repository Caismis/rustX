//! Finite Trace value vocabulary and the immutable request source encoding at
//! the native request commit. Reads use indexed byte ranges; the Runtime
//! Client's Trace reader pages these values and never reconstructs them.

pub(crate) mod bounds;
pub(crate) mod content;
pub(crate) mod frozen;
pub(crate) mod generation;
pub(crate) mod request;
pub(crate) mod types;

pub(crate) use bounds::identity_fits;
pub use bounds::{
    TRACE_DETAIL_BYTES, TRACE_PAGE_BYTES, TRACE_RECORD_BYTES, TRACE_SUMMARY_CONTEXT,
    TRACE_SUMMARY_CONTEXT_BYTES, TraceJson, TracePreview, TraceText,
};
pub(crate) use frozen::{
    request_detail as frozen_request_detail, request_outcome as frozen_request_outcome,
};
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
