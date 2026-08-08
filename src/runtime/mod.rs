//! Runtime-owned identities, shared semantics, coordination contracts, and
//! provider boundaries.
//!
//! This module owns Layer-0 types that are shared across message, tool,
//! model, event, and manifest contracts: strongly typed identifiers,
//! cancellation reasons, runtime errors, and the provider continuation-state
//! boundary. It also owns the narrow conversation inbound mailbox
//! ([`ConversationInboundMailbox`]): a per-conversation in-memory
//! coordination contract that both the agent kernel and future runtime
//! producers can depend on without the tool plane depending upward on
//! `src/agent`. The mailbox is coordination only — it is not canonical
//! history, not the Event Journal, not a scheduler, supervisor, or
//! persistent service layer. Runtime supervision, cancellation propagation,
//! capability guards, recovery, and process ownership are later milestones
//! and are not implemented here.

pub mod continuation;
pub mod identity;
pub mod inbound;
pub mod types;

pub use continuation::{
    AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
};
pub use identity::{
    AgentId, AgentVersionId, ArtifactId, AttemptId, CapabilityRevision, ConversationId, EventId,
    McpServerId, MessageId, SkillId, SkillVersionId, ToolCallId, ToolId, ToolVersionId, TurnId,
};
pub use inbound::{
    ConversationInboundMailbox, InboundBatch, InboundItem, InboundSequence, MailboxError,
};
pub use types::{CancellationReason, RuntimeError};
