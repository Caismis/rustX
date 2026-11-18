//! Provider continuation / reasoning state boundary.
//!
//! Opaque reasoning and continuation state is never flattened into plain text: the
//! canonical layer preserves the provider-specific opaque state a later
//! adapter needs to continue a generation. Provider SDK-specific Rust types
//! are forbidden on this boundary; it holds rustX-owned serializable data
//! only. Model adapters (M2) are responsible for converting between provider
//! SDK state and this boundary.

/// Provider-specific continuation state preserved by the runtime.
///
/// `None` (absence) is the natural representation for protocols that carry no
/// continuation state, such as `OpenAI` Chat Completions, which resend the
pub use rustx_types::runtime::continuation::{
    AcceptedOutputContinuationSource, AnthropicContinuation, OpenAiResponsesContinuation,
    ProviderContinuationState,
};
