//! The Runtime Client snapshot read model (Runtime Client Protocol v1).
//!
//! [`RuntimeClientSnapshot`] is the one deterministic external read model
//! of authoritative runtime state. It is a projection, never a second
//! authority:
//!
//! - committed messages mirror the conversation's canonical history;
//! - the attempt view mirrors the current/latest attempt execution;
//! - the in-flight output and foreground tool views carry enough state to
//!   repair every client-visible streaming effect;
//! - background executions mirror the authoritative conversation registry;
//! - the Agent Status view derives from the exact composed status;
//! - inbound diagnostics mirror the authoritative mailbox;
//! - the capability view mirrors the active capability snapshot.
//!
//! The snapshot carries no internal executors, no environment paths, no
//! provider objects, and no synchronization identities.
//!
//! Snapshot semantics are frozen by the snapshot/cursor invariant: a
//! snapshot returned at cursor `C` describes all Runtime Client state
//! through `C`, and a subscription after `C` observes every subsequently
//! published event or fails explicitly with `resync_required`.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use super::event::RuntimeClientOutcome;
use crate::message::types::{ContentBlockIndex, MessageBlock, UserMessageBlock};
use crate::model::session::{AttemptModelView, SessionModelView};
use crate::model::types::ModelUsage;
use crate::runtime::identity::{
    AttemptId, CapabilityRevision, ConversationId, MessageId, SkillId, SkillVersionId, ToolCallId,
    ToolExecutionId, ToolId,
};
use crate::runtime::inbound::InboundSequence;
use crate::runtime::types::TokenMeasurement;
use crate::tools::background::BackgroundLifecycle;
use crate::tools::types::{
    ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionResult, ToolOrigin, ToolProgress,
    ToolReplayPolicy,
};

/// The authoritative Runtime Client snapshot of one conversation runtime.
///
/// Every section is a deterministic projection of one authoritative
/// runtime owner. The shape belongs to Runtime Client Protocol v1: internal
/// snapshot types are projected into these external DTOs, never exposed
/// directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientSnapshot {
    /// The conversation this snapshot belongs to.
    pub conversation_id: ConversationId,
    /// Whether the runtime has accepted shutdown and stopped admitting new
    /// inbound work. This is runtime-owned state, not a client observation.
    pub shutting_down: bool,
    /// The committed canonical conversation messages, in canonical order.
    ///
    /// This is a read model of canonical history: it is repaired from
    /// authoritative commit observations and is never independently
    /// mutable.
    pub messages: Vec<MessageBlock>,
    /// The current/latest attempt view, when any attempt exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<RuntimeClientAttempt>,
    /// The inbound mailbox diagnostics (pending items and the latest
    /// finite drain observation).
    pub inbound: InboundDiagnostics,
    /// All background executions in execution allocation order, including
    /// terminal records retained by the authoritative registry.
    #[serde(default)]
    pub background: Vec<RuntimeClientBackgroundExecution>,
    /// The latest composed Agent Status observation, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AgentStatusView>,
    /// Runtime-owned context-compaction diagnostics. The values describe
    /// committed `RuntimeEvent::CompactionCompleted` facts; they never
    /// replace canonical history or expose summary content.
    #[serde(default)]
    pub context: RuntimeClientContextView,
    /// The active capability projection.
    pub capabilities: CapabilityView,
    /// The redacted session model state: the authoritative *desired*
    /// configuration and its resolution.
    ///
    /// This is deliberately distinct from
    /// [`RuntimeClientAttempt::model`], which is the immutable snapshot an
    /// already-admitted attempt froze. While an attempt on model A runs and
    /// the session has been switched to model B, this section truthfully
    /// shows B and the attempt section truthfully shows A.
    ///
    /// No credential, adapter object, provider HTTP client, or
    /// synchronization identity appears here.
    pub model: SessionModelView,
}

/// The context diagnostics carried by the Runtime Client snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientContextView {
    /// Runtime Client projection statistic: the number of committed
    /// completion events folded into this read model. Checkpoint generation
    /// remains the context-owned identity.
    pub compaction_count: u64,
    /// The latest committed checkpoint metadata, when compaction occurred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_compaction: Option<RuntimeClientCompactionView>,
}

/// Public metadata for one committed context checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientCompactionView {
    /// The monotonically increasing checkpoint generation.
    pub generation: u64,
    /// The pre-compaction input measurement and its provenance.
    pub tokens_before: TokenMeasurement,
    /// The deterministic estimate of the rebuilt projection.
    pub estimated_tokens_after: u64,
}

/// The external attempt view of the Runtime Client projection.
///
/// The view folds attempt lifecycle, turn progress, in-flight agent
/// output, and foreground tool execution into one structured read model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientAttempt {
    /// The attempt identity.
    pub attempt_id: AttemptId,
    /// The externally meaningful attempt phase.
    pub phase: RuntimeClientAttemptPhase,
    /// The number of completed turns.
    pub turn: u32,
    /// The latest normalized usage of a completed model request, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_usage: Option<ModelUsage>,
    /// The in-flight agent output, when a message is streaming: enough
    /// accumulated state to repair every client-visible streaming effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<InFlightAgentMessage>,
    /// The foreground tool executions of the attempt in call-assembly
    /// order.
    #[serde(default)]
    pub foreground: Vec<ForegroundToolExecution>,
    /// The immutable model snapshot this attempt was admitted with.
    ///
    /// A client never has to infer "which model is this attempt actually
    /// using" from event ordering: the answer is here for the attempt's
    /// whole lifetime, even after the session moved on to another model.
    pub model: Box<AttemptModelView>,
}

/// The externally meaningful phase of one attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientAttemptPhase {
    /// The coordinator admitted the attempt; the loop has not started yet.
    Admitted,
    /// The attempt is executing.
    Running,
    /// The attempt settled; the terminal outcome is final and absorbing.
    Settled {
        /// The platform-level terminal settlement.
        outcome: RuntimeClientOutcome,
    },
}

/// The accumulated in-flight output of one streaming agent message.
///
/// This is the repair state of streaming: a snapshot taken mid-stream
/// carries every accumulated delta through its cursor, so a client
/// repairing after `resync` reconstructs the exact message it would have
/// observed incrementally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InFlightAgentMessage {
    /// The provisional message identity.
    pub message_id: MessageId,
    /// The ordered content blocks assembled so far.
    #[serde(default)]
    pub blocks: Vec<InFlightBlock>,
}

/// One ordered block of an in-flight agent message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InFlightBlock {
    /// Accumulated text of one output block.
    Text {
        /// The canonical block index.
        block_index: ContentBlockIndex,
        /// The accumulated text.
        text: String,
    },
    /// Accumulated reasoning of one output block.
    Reasoning {
        /// The canonical block index.
        block_index: ContentBlockIndex,
        /// The accumulated reasoning text.
        text: String,
    },
    /// Accumulated refusal of one output block.
    Refusal {
        /// The canonical block index.
        block_index: ContentBlockIndex,
        /// The accumulated refusal text.
        text: String,
    },
    /// One tool call being assembled.
    ToolCall {
        /// The canonical block index.
        block_index: ContentBlockIndex,
        /// The tool-call identity.
        call_id: ToolCallId,
        /// The canonical tool identity.
        tool_id: ToolId,
        /// The model-facing tool name.
        name: String,
        /// The accumulated JSON argument fragments.
        arguments: String,
    },
}

/// The foreground tool execution read model of one logical tool call.
///
/// Keyed by the canonical logical tool-call identity, so parallel physical
/// completion timing can never corrupt logical identities or canonical
/// ordering. Native, MCP, and Python foreground executions converge through
/// this one shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForegroundToolExecution {
    /// The logical tool-call identity.
    pub call_id: ToolCallId,
    /// The canonical tool identity.
    pub tool_id: ToolId,
    /// The model-facing tool name at call time.
    pub name: String,
    /// The externally meaningful execution state.
    pub state: ForegroundToolState,
}

/// The externally meaningful state of one foreground tool execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ForegroundToolState {
    /// The call is known and its arguments are assembled; execution has
    /// not started.
    Assembled {
        /// The assembled JSON arguments.
        arguments: String,
    },
    /// The execution is running.
    Running {
        /// The assembled JSON arguments.
        arguments: String,
        /// The latest bounded progress, when any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<ToolProgress>,
    },
    /// The execution settled with its normalized result.
    Settled {
        /// The assembled JSON arguments.
        arguments: String,
        /// The normalized execution result.
        result: ToolExecutionResult,
    },
}

/// The inbound mailbox diagnostics of the Runtime Client projection.
///
/// A read-only view of authoritative mailbox state: it validates the
/// Issue #22 ordering contract and supports reconnect, and it can never
/// drain or mutate the mailbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundDiagnostics {
    /// The currently pending inbound items in runtime-assigned inbound
    /// sequence order.
    #[serde(default)]
    pub pending: Vec<InboundItemView>,
    /// The latest observed finite drain boundary, when any drain occurred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_drain: Option<InboundDrainView>,
}

/// One pending inbound item of the diagnostics view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundItemView {
    /// The mailbox-assigned inbound sequence.
    pub sequence: InboundSequence,
    /// The canonical inbound message.
    pub message: UserMessageBlock,
}

/// The latest observed finite drain boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboundDrainView {
    /// The highest selected inbound sequence.
    pub watermark: InboundSequence,
    /// The number of drained items.
    pub count: usize,
}

/// The external background execution read model.
///
/// Projected from the authoritative [`ConversationBackgroundRegistry`]
/// ([`crate::tools::background::ConversationBackgroundRegistry`]); the
/// container shape belongs to Runtime Client Protocol v1 while the
/// lifecycle, progress, and result leaf types are stable runtime-owned
/// value contracts. No internal task handles or process ids ever appear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientBackgroundExecution {
    /// The detached runtime execution identity.
    pub execution_id: ToolExecutionId,
    /// The canonical tool identity.
    pub tool_id: ToolId,
    /// The model-facing tool name.
    pub tool_name: String,
    /// The authoritative lifecycle state.
    pub state: BackgroundLifecycle,
    /// The latest bounded progress, when any was reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<ToolProgress>,
    /// The bounded terminal result, when terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ToolExecutionResult>,
}

/// The structured Agent Status view of one composition.
///
/// Derived from the exact composed status the model path consumed: the
/// structured sections and the canonical rendered representation originate
/// from the same composition, so a client never parses the rendered text
/// to recover structure and never triggers a second composition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStatusView {
    /// The attempt that composed the status.
    pub attempt_id: AttemptId,
    /// The turn number of the request preparation.
    pub turn: u32,
    /// The canonical inbound message the status targets.
    pub target_message_id: MessageId,
    /// The ordered structured sections.
    #[serde(default)]
    pub sections: Vec<RuntimeClientStatusSection>,
    /// The canonical rendered representation, derived from the same
    /// composition as the sections.
    pub rendered: String,
}

/// One structured Agent Status section of the external view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientStatusSection {
    /// The mandatory temporal facts.
    Temporal {
        /// The runtime clock value sampled at composition time.
        current_time: DateTime<Utc>,
        /// The conversation timezone, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<Tz>,
        /// The persisted timestamp of the final message of the fresh
        /// inbound turn.
        inbound_message_time: DateTime<Utc>,
    },
    /// The runtime-owned background-execution section.
    BackgroundExecutions {
        /// The active background executions in allocation order.
        executions: Vec<RuntimeClientBackgroundExecution>,
    },
    /// An extension section's ordered structured facts.
    Facts {
        /// The ordered facts.
        facts: Vec<RuntimeClientStatusFact>,
    },
}

/// One structured fact of an extension status section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientStatusFact {
    /// The fact label.
    pub label: String,
    /// The fact value.
    pub value: String,
}

/// The deterministic active capability projection.
///
/// Projected from the active [`CapabilitySnapshot`]
/// ([`crate::capabilities::CapabilitySnapshot`]): the revision, the
/// deterministic tool catalog, and the deterministic Skill catalog. No
/// executors, environment paths, package-manager state, or private
/// dependency internals appear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityView {
    /// The active monotonic capability revision.
    pub revision: CapabilityRevision,
    /// The deterministic tool catalog in registry order.
    #[serde(default)]
    pub tools: Vec<RuntimeClientTool>,
    /// The deterministic Skill catalog ordered by Skill name.
    #[serde(default)]
    pub skills: Vec<RuntimeClientSkill>,
}

/// One external tool catalog entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientTool {
    /// The canonical tool identity.
    pub id: ToolId,
    /// The stable model-facing tool name.
    pub name: String,
    /// The human-readable description.
    pub description: String,
    /// The canonical JSON Schema of accepted arguments.
    pub input_schema: serde_json::Value,
    /// Who owns an invocation: attempt (foreground) or conversation
    /// (background).
    pub execution_policy: ToolExecutionPolicy,
    /// How calls within one batch are scheduled.
    pub concurrency_policy: ToolConcurrencyPolicy,
    /// The replay policy.
    pub replay_policy: ToolReplayPolicy,
    /// Where the tool comes from.
    pub origin: ToolOrigin,
}

/// One external Skill catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientSkill {
    /// The validated standard Skill identity.
    pub id: SkillId,
    /// The immutable Skill version identity.
    pub version_id: SkillVersionId,
    /// The validated standard Skill name.
    pub name: String,
    /// The validated standard Skill description.
    pub description: String,
}

impl RuntimeClientSnapshot {
    /// The conversation identity the snapshot belongs to.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }
}
