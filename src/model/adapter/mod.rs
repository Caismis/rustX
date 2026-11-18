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
pub use rustx_types::model::adapter::block_index;
pub(crate) mod cache;
pub(crate) mod correlation;
pub mod openai;
#[cfg(feature = "issue-387-profile")]
pub(crate) mod profile;
pub(crate) mod proposal;
pub(crate) use rustx_types::model::adapter::retry;
pub mod structured;
pub mod traits;
pub(crate) use rustx_types::model::adapter::transport;
pub mod validation;

pub use traits::{
    ModelAdapter, ModelStream, ModelStreamItem, ModelStreamProgress, model_stream_of_failure,
};

pub(crate) use rustx_types::model::adapter::response_budget;
