//! Model execution adapters (M2).
//!
//! This is the model-plane implementation layer. It converts between the
//! provider-neutral runtime contracts ([`ModelRequest`] in, [`ModelStreamItem`] stream
//! out) and provider protocols. Provider SDK and wire types terminate inside
//! the `openai` and `anthropic` submodules; nothing here exposes a provider
//! type through a public interface.
//!
//! Runtime policy owns retries: one adapter invocation performs exactly one
//! provider request attempt and returns a normalized [`ModelError`] instead
//! of retrying.
//!
//! [`ModelError`]: crate::model::error::ModelError
//! [`ModelEvent`]: crate::model::event::ModelEvent
//! [`ModelRequest`]: crate::model::types::ModelRequest

pub mod anthropic;
pub mod block_index;
pub(crate) mod correlation;
pub mod openai;
#[cfg(feature = "issue-387-profile")]
pub(crate) mod profile;
pub(crate) mod proposal;
pub mod traits;
pub mod validation;

pub use traits::{
    ModelAdapter, ModelStream, ModelStreamItem, ModelStreamProgress, model_stream_of_failure,
};

#[cfg(feature = "issue-387-profile")]
pub(crate) type ProviderRequest = profile::Payload;
#[cfg(not(feature = "issue-387-profile"))]
pub(crate) type ProviderRequest = serde_json::Value;

/// Preserves the exact final provider value; profiling observes its real serializer.
pub(crate) fn provider_request(value: serde_json::Value) -> ProviderRequest {
    #[cfg(feature = "issue-387-profile")]
    {
        profile::Payload::from(value)
    }
    #[cfg(not(feature = "issue-387-profile"))]
    {
        value
    }
}
