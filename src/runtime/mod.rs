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
/// The conversation runtime coordinator (Issue #61): the semantic owner of
/// conversation coordination — session model state, attempt admission, the
/// current-attempt slot, between-attempt canonical state, request history,
/// the shutdown gate, and settlement handoff.
pub mod conversation_runtime;
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
/// The runtime-owned semantic observation contract (Issue #61): the
/// observation vocabulary and the leaf pending-observation queue. Runtime
/// Client projection types never appear here; the Runtime Client adapter
/// translates these shapes, and it owns the only fold of the stream.
pub(crate) mod observation;
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
/// The runtime-owned retained provider-neutral request facts (Issue #61):
/// immutable settled request snapshots, appended by the conversation
/// runtime coordinator at attempt settlement. Not client projection state.
pub mod request_history;
/// The shared structural ownership core of every rustX-owned supervised
/// process unit (M5 Bash and M7 interactive MCP stdio). Internal
/// coordination only — it is not part of the public runtime API.
pub(crate) mod supervised_unit;
pub mod types;

pub use cancellation::CancellationSignal;
pub use continuation::{
    AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
};
pub use conversation_runtime::{
    CancelAttemptError, ConversationContextConfig, ConversationRuntime, ConversationRuntimeError,
    InboundAdmission, InboundAdmissionError, ModelUpdateError, RuntimeConversationConfig,
};
pub use identity::{
    AgentId, AgentVersionId, ArtifactId, AttemptId, CapabilityRevision, ConversationId, EventId,
    McpServerId, MessageId, NodeEnvironmentDigest, PythonEnvironmentDigest, SkillId,
    SkillVersionId, ToolCallId, ToolExecutionId, ToolId, ToolVersionId, TurnId,
};
pub use inbound::{
    ConversationInboundMailbox, InboundBatch, InboundItem, InboundSequence, MailboxError,
};
pub use request_history::{RequestHistory, RequestHistoryError};
pub use types::{
    CancellationReason, RuntimeClock, RuntimeError, SystemClock, TokenMeasurement,
    TokenMeasurementSource,
};
