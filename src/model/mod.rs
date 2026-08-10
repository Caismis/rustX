//! Canonical model requests, model events, usage, errors, and provider adapters.
//!
//! M1 implements the canonical model contracts: `ModelRequest`, `ModelEvent`,
//! `ModelUsage`, normalized finish reasons, normalized errors, and the model
//! protocol enum. M2 implements the provider adapters for `OpenAI` Chat
//! Completions, `OpenAI` Responses, and `Anthropic` Messages in
//! `crate::model::adapter`, speaking the runtime-owned `ModelAdapter`
//! interface. The provider continuation-state boundary lives in
//! `crate::runtime::continuation`.

pub mod adapter;
pub mod error;
pub mod event;
pub mod finish;
pub mod types;

pub use adapter::anthropic::{AnthropicAdapterConfig, AnthropicMessagesAdapter};
pub use adapter::openai::{
    OpenAiAdapterConfig, OpenAiChatCompletionsAdapter, OpenAiResponsesAdapter, ResponsesStorageMode,
};
pub use adapter::{ModelAdapter, ModelEventStream, model_event_stream_of_failure};
pub use error::{ModelError, ModelErrorKind};
pub use event::ModelEvent;
pub use finish::ModelFinishReason;
pub use types::{ModelProtocol, ModelRequest, ModelUsage, ReasoningEffort, UsageDetails};

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
