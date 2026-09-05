//! Provider-neutral model requests, canonical model events, stream progress,
//! usage, errors, and provider adapters.
//!
//! M1 implements the provider-neutral model contracts: `ModelRequest`, `ModelEvent`,
//! `ModelUsage`, normalized finish reasons, normalized errors, and the model
//! protocol enum. M2 implements the provider adapters for `OpenAI` Chat
//! Completions, `OpenAI` Responses, and `Anthropic` Messages in
//! `crate::model::adapter`, speaking the runtime-owned `ModelAdapter`
//! interface. The provider continuation-state boundary lives in
//! `crate::runtime::continuation`.

pub mod adapter;
pub mod catalog;
pub mod deadline;
pub mod error;
pub mod event;
pub mod finish;
pub mod frozen;
pub mod generation;
pub mod input;
pub mod invocation;
pub mod session;
pub mod snapshot;
pub mod types;

pub use adapter::anthropic::{AnthropicAdapterConfig, AnthropicMessagesAdapter};
pub use adapter::openai::{
    OpenAiAdapterConfig, OpenAiChatCompletionsAdapter, OpenAiResponsesAdapter,
};
pub use adapter::{
    ModelAdapter, ModelStream, ModelStreamItem, ModelStreamProgress, model_stream_of_failure,
};
pub use catalog::{
    ChatMaxTokensField, ChatReasoningReplay, ChatStreamUsage, ChatToolProtocol, CredentialSource,
    CredentialSourceView, Modality, ModelCapabilities, ModelCatalog, ModelCatalogError,
    ModelCatalogView, ModelCompat, ModelDefinition, ModelId, ModelRef, ProviderId,
    ReasoningProfile, ReasoningProfileId, ResolvedModelCatalog, ResponsesStorageMode,
};
pub use deadline::{
    DEFAULT_RESPONSE_START_TIMEOUT, DEFAULT_STREAM_IDLE_TIMEOUT, ModelDeadlinePhase, ModelProgress,
    ModelRequestDeadline, ModelTimeoutPhase, ModelTimeoutPolicy,
};
pub use error::{
    ContextOverflowReport, MAX_MALFORMED_TOOL_PROPOSAL_MESSAGE_BYTES, MalformedToolProposalSource,
    ModelError, ModelErrorKind, ModelRetryDisposition,
};
pub use event::ModelEvent;
pub use finish::ModelFinishReason;
pub use frozen::{
    FrozenModelInvocation, FrozenModelSpec, FrozenProviderBinding, FrozenSummaryModel,
};
pub use generation::{
    DEGENERATION_CANDIDATE_PERIODS, DEGENERATION_MAX_COMPARISONS_PER_SCAN,
    DEGENERATION_MAX_PERIOD_BYTES, DEGENERATION_MIN_PERIOD_BYTES, DEGENERATION_MIN_REPETITIONS,
    DEGENERATION_MIN_SPAN_BYTES, DEGENERATION_SCAN_STRIDE_BYTES, GenerationBudgetKind,
    GenerationChannel, GenerationFailure, GenerationGuard, GenerationSafetyPolicy,
};
pub use input::{
    CarryoverBlockKind, CarryoverDetailLevel, CarryoverOmissionCounts, ModelInputMessage,
    RenderedCarryoverRecord, RenderedCarryoverText, RenderedCarryoverToolCall,
    RenderedUnresolvedOutputCarryover, RequestOnlyInsertionAnchor, RequestOnlyModelContext,
    UnresolvedOutputSettlement, canonical_input,
};
pub use invocation::{
    DEFAULT_RUNTIME_REASONING_BYTE_SHARE_DENOMINATOR,
    DEFAULT_RUNTIME_REASONING_BYTE_SHARE_NUMERATOR, ModelBindingRegistry, ModelInvocationConfig,
    ModelInvocationError, ModelInvocationView, ModelSelection,
    RUNTIME_GENERATED_BYTES_PER_OUTPUT_TOKEN, RUNTIME_MIN_GENERATED_BYTES, RequestParams,
    ResolvedModelInvocation, runtime_fallback_generation_safety_policy,
};
pub use session::{
    AttemptModelSnapshot, AttemptModelView, AttemptSummaryModel, SessionModelConfig,
    SessionModelState, SessionModelView, SummaryModelPolicy, SummaryModelView,
};
pub use snapshot::{
    AgentStatusStart, RequestIdentity, RequestReconstructionError, RequestSnapshot,
};
pub use types::{ModelProtocol, ModelRequest, ModelUsage, UsageDetails};

/// Test-only model protocol value for in-crate agent-loop unit tests.
///
/// The agent kernel must stay free of provider-protocol literals (enforced
/// by the M3 `agent_modules_contain_no_provider_branching` guard, which
/// scans `src/agent`), so scripted adapters outside `src/agent` name the
/// concrete protocol here instead.
#[cfg(test)]
#[must_use]
pub(crate) fn chat_protocol() -> ModelProtocol {
    ModelProtocol::OpenAiChatCompletions
}
