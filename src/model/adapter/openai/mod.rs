//! `OpenAI` model adapters (Chat Completions and Responses).
//!
//! All `async-openai` SDK types terminate inside this module: the public
//! surface is the canonical [`ModelAdapter`] trait plus rustX-owned
//! configuration. Automatic retry is bypassed by construction through the
//! no-retry transport in `client.rs`; one adapter invocation performs exactly
//! one provider request attempt.
//!
//! [`ModelAdapter`]: crate::model::adapter::traits::ModelAdapter

pub mod chat_completions;
mod client;
pub mod config;
pub(crate) mod mapping;
pub(crate) mod qwen_xml;
pub mod responses;

pub use chat_completions::OpenAiChatCompletionsAdapter;
pub use config::OpenAiAdapterConfig;
pub use responses::OpenAiResponsesAdapter;
