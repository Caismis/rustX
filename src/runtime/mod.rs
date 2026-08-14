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
//! persistent service layer. General runtime supervision, persistent
//! process management, recovery services, and broader capability
//! coordination remain future work. M5 implements only the narrow
//! internal process-supervision capability required for Bash catastrophic
//! supervisor-loss recovery (`crate::runtime::process_supervision`:
//! one-time activation of Linux child-subreaper mode). That is
//! coordination infrastructure, not a generic process manager and not a
//! public runtime API.

pub mod cancellation;
pub mod continuation;
pub mod identity;
pub mod inbound;
/// The rustX-side driver of one long-lived interactive process (MCP stdio
/// servers), owning physical settlement of the interactive supervisor unit.
pub(crate) mod interactive_process;
/// The long-lived interactive supervisor unit (M5-equivalent ownership).
/// The binary entry points are reachable only from the dedicated
/// `interactive-supervisor` bin and from tests via self-exec; they are
/// documented-hidden binary entry points, never runtime API.
#[doc(hidden)]
pub mod interactive_supervisor;
/// The internal owned supervised command runner shared by native Bash and
/// Skill environment materialization: the M5 Bash process-group lifecycle
/// extracted so package-manager work reuses the same rustX-owned
/// supervisor/process-group domain instead of a second independent
/// subprocess hierarchy. Internal coordination only — it is not part of
/// the public runtime API.
pub(crate) mod process_runner;
/// The runtime-level Linux process-supervision capability: the one-time
/// activation of the process-wide child-subreaper primitive used by Bash
/// catastrophic fallback. Internal coordination only — it is not part of
/// the public runtime API.
pub(crate) mod process_supervision;
/// The shared structural ownership core of every rustX-owned supervised
/// process unit (M5 Bash and M7 interactive MCP stdio). Internal
/// coordination only — it is not part of the public runtime API.
pub(crate) mod supervised_unit;
pub mod types;

pub use cancellation::CancellationSignal;
pub use continuation::{
    AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
};
pub use identity::{
    AgentId, AgentVersionId, ArtifactId, AttemptId, CapabilityRevision, ConversationId, EventId,
    McpServerId, MessageId, NodeEnvironmentDigest, PythonEnvironmentDigest, SkillId,
    SkillVersionId, ToolCallId, ToolExecutionId, ToolId, ToolVersionId, TurnId,
};
pub use inbound::{
    ConversationInboundMailbox, InboundBatch, InboundItem, InboundSequence, MailboxError,
};
pub use types::{
    CancellationReason, RuntimeClock, RuntimeError, SystemClock, TokenMeasurement,
    TokenMeasurementSource,
};
