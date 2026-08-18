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
//! persistent service layer. M9c composes the concrete conversation-owned
//! drain/quiescence boundaries; it is not a generic scheduler or plugin
//! lifecycle system. M5 implements only the narrow internal
//! process-supervision capability required for Bash catastrophic
//! supervisor-loss recovery (`crate::runtime::process_supervision`): Linux
//! uses one-time child-subreaper activation; macOS uses its direct
//! process-group path without an equivalent orphan-adoption primitive. That
//! is coordination infrastructure, not a generic process manager and not a
//! public runtime API.

pub mod cancellation;
pub mod continuation;
/// The conversation runtime coordinator (Issue #61): the semantic owner of
/// conversation coordination — session model state, attempt admission, the
/// current-attempt slot, between-attempt current Surface state, durable
/// request-history reads, the shared lifecycle/drain authority, and
/// settlement handoff through quiescence.
pub mod conversation_runtime;
pub mod identity;
pub mod inbound;
/// The native provider-independent human interaction rendezvous and pending
/// interaction projection facts (Issue #64).
pub mod interaction;
/// The rustX-side driver of one long-lived interactive process (MCP stdio
/// servers), owning physical settlement of the interactive supervisor unit.
pub(crate) mod interactive_process;
/// The long-lived interactive supervisor unit (M5-equivalent ownership).
/// The binary entry points are reachable only from the dedicated
/// `interactive-supervisor` bin and from tests via self-exec; they are
/// documented-hidden binary entry points, never runtime API.
#[doc(hidden)]
#[cfg(unix)]
pub mod interactive_supervisor;
/// Non-Unix interactive supervisor entry points are intentionally unavailable
/// because the runtime's process-unit implementation uses Unix process
/// groups and Unix-domain control sockets.
#[doc(hidden)]
#[cfg(not(unix))]
pub mod interactive_supervisor {
    /// The control socket environment variable used by the Unix supervisor.
    pub const RUSTX_CONTROL_ENV: &str = "RUSTX_INTERACTIVE_CONTROL";

    /// Non-Unix platforms cannot provide the supervisor's process-group
    /// ownership proof.
    pub fn run_outer(_arguments: &[String]) -> i32 {
        eprintln!("interactive supervisor requires Unix process supervision");
        1
    }

    /// Non-Unix platforms cannot provide the supervisor's process-group
    /// ownership proof.
    pub fn run_inner(_arguments: &[String]) -> i32 {
        eprintln!("interactive supervisor requires Unix process supervision");
        1
    }
}
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
/// The runtime-level Linux/macOS process-supervision prerequisite used by
/// Bash catastrophic fallback. Internal coordination only — it is not part
/// of the public runtime API.
pub(crate) mod process_supervision;
/// The platform adapter for the `waitid` primitive. Linux uses `nix`'s
/// implementation; macOS uses its libc implementation because `nix` does not
/// currently expose `waitid` on Apple targets.
#[cfg(unix)]
pub(crate) mod process_wait;
/// Durable startup recovery (Issue #12, M9a): the reconstruct → classify →
/// reconcile → resume pipeline the conversation runtime owns. Recovery policy
/// lives here, above the durable store and below nothing: no client, no
/// provider adapter, no mailbox, and no background producer participates.
pub mod recovery;
/// The runtime-owned provider-neutral request read handle (M8 / Issue #11):
/// immutable request snapshots are persisted at request start and resolved on
/// demand from the native durable conversation authority. Not client
/// projection state.
pub mod request_history;
/// The shared structural ownership core of every rustX-owned supervised
/// process unit (M5 Bash and M7 interactive MCP stdio). Internal
/// coordination only — it is not part of the public runtime API.
pub(crate) mod supervised_unit;
pub mod types;

pub use cancellation::{CancellationCause, CancellationSignal, ExecutionCancellation};
pub use continuation::{
    AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
};
pub use conversation_runtime::{
    CancelAttemptError, ConversationContextConfig, ConversationRuntime, ConversationRuntimeError,
    InboundAdmission, InboundAdmissionError, ModelUpdateError, RuntimeConversationConfig,
};
pub use identity::{
    AgentId, AgentVersionId, ArtifactId, AttemptId, CapabilityRevision, ConversationId, EventId,
    InteractionId, McpServerId, MessageId, NodeEnvironmentDigest, PythonEnvironmentDigest,
    RequestId, SkillId, SkillVersionId, ToolCallId, ToolExecutionId, ToolId, ToolVersionId, TurnId,
};
pub use inbound::{
    ConversationInboundMailbox, InboundBatch, InboundItem, InboundSequence, MailboxError,
};
pub use interaction::{
    ApprovalDecision, ApprovalFacts, InteractionCoordinator, InteractionError, InteractionKind,
    InteractionObserver, InteractionOutcome, InteractionRendezvous, InteractionRequest,
    InteractionResponse, InteractionTicket, UnavailableInteraction,
};
pub use recovery::{
    AttemptRecoveryClass, BackgroundEvidence, BackgroundRecoveryClass, KnownModelOutcome,
    RecoveryError, RecoveryEvidence, RecoveryPlan, RecoveryReconciliation, RecoveryReport,
    RequestOutcome, ResumeDisposition,
};
pub use request_history::{RequestHistory, RequestHistoryError};
pub use types::{
    CancellationReason, ConversationLifecycle, ConversationLifecycleState, RuntimeClock,
    RuntimeError, SystemClock, TokenMeasurement, TokenMeasurementSource,
};
