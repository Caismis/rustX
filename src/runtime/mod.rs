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

pub mod cancellation;
pub mod continuation;
pub mod identity;
pub mod inbound;
/// The runtime-level Linux process-supervision capability: the one-time
/// activation of the process-wide child-subreaper primitive used by Bash
/// catastrophic fallback. Internal coordination only — it is not part of
/// the public runtime API.
pub(crate) mod process_supervision;
pub mod types;

pub use cancellation::CancellationSignal;
pub use continuation::{
    AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
};
pub use identity::{
    AgentId, AgentVersionId, ArtifactId, AttemptId, CapabilityRevision, ConversationId, EventId,
    McpServerId, MessageId, SkillId, SkillVersionId, ToolCallId, ToolExecutionId, ToolId,
    ToolVersionId, TurnId,
};
pub use inbound::{
    ConversationInboundMailbox, InboundBatch, InboundItem, InboundSequence, MailboxError,
};
pub use types::{CancellationReason, RuntimeClock, RuntimeError, SystemClock};
