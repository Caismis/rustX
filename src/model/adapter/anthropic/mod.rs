//! Anthropic Messages model adapter.
//!
//! The adapter talks to the Anthropic `/v1/messages` streaming API directly
//! over HTTP/SSE. There is no official Anthropic Rust SDK, and the evaluated
//! community SDK has stale typed stop-reason coverage, so rustX owns the wire
//! representation (`wire.rs`), request/error/finish mapping (`mapping.rs`),
//! and the transport (`messages.rs`). No Anthropic SDK type exists anywhere
//! in rustX, and one adapter invocation performs exactly one HTTP request.

pub mod config;
pub(crate) mod mapping;
pub mod messages;
pub(crate) mod wire;

pub use config::AnthropicAdapterConfig;
pub use messages::AnthropicMessagesAdapter;
