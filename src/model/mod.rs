//! Canonical model requests, model events, usage, errors, and provider adapters.
//!
//! M1 implements the canonical model contracts: `ModelRequest`, `ModelEvent`,
//! `ModelUsage`, normalized finish reasons, normalized errors, and the model
//! protocol enum. Provider adapters for `OpenAI` Chat Completions, `OpenAI`
//! Responses, and `Anthropic` Messages are milestone M2 and are not
//! implemented here. The provider continuation-state boundary lives in
//! `crate::runtime::continuation`.

pub mod adapter;
pub mod error;
pub mod event;
pub mod finish;
pub mod types;

pub use adapter::{
    model_event_stream_of_failure, ModelAdapter, ModelCancellation, ModelEventStream,
};
pub use error::{ModelError, ModelErrorKind};
pub use event::ModelEvent;
pub use finish::ModelFinishReason;
pub use types::{ModelProtocol, ModelRequest, ModelUsage, ReasoningEffort, UsageDetails};
