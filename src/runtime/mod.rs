//! Runtime-owned identities, shared semantics, and provider boundaries.
//!
//! This module owns Layer-0 types that are shared across message, tool,
//! model, event, and manifest contracts: strongly typed identifiers,
//! cancellation reasons, runtime errors, and the provider continuation-state
//! boundary. Runtime supervision, cancellation propagation, capability
//! guards, recovery, and process ownership are later milestones and are not
//! implemented here.

pub mod continuation;
pub mod identity;
pub mod types;

pub use continuation::{
    AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
};
pub use identity::{
    AgentId, AgentVersionId, ArtifactId, AttemptId, CapabilityRevision, ConversationId, EventId,
    McpServerId, MessageId, SkillId, SkillVersionId, ToolCallId, ToolId, ToolVersionId, TurnId,
};
pub use types::{CancellationReason, RuntimeError};
