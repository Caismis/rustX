//! The Runtime Client snapshot read model (Runtime Client Protocol v4).
//!
//! [`RuntimeClientSnapshot`] is the one deterministic external read model
//! of authoritative runtime state. It is a projection, never a second
//! authority:
//!
//! - committed messages mirror the currently projected canonical Surface;
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
use crate::conversation::SurfaceRevision;
use crate::events::interaction::{InteractionSettlement, InteractionSubject};
use crate::events::types::RuntimeEventEnvelope;
use crate::message::types::{ContentBlockIndex, MessageBlock, UserMessageBlock};
use crate::model::session::{AttemptModelView, SessionModelView};
use crate::model::types::ModelUsage;
use crate::publication::PublicationAudit;
use crate::runtime::identity::{
    AttemptId, CapabilityRevision, ConversationId, EventId, InteractionId, MessageId,
    RuntimeResourceRevision, SkillId, SkillVersionId, ToolCallId, ToolExecutionId, ToolId, TurnId,
};
use crate::runtime::inbound::InboundSequence;
use crate::runtime::interaction::InteractionRequest;
use crate::runtime::types::{ApprovalMode, TokenMeasurement};
use crate::tools::background::BackgroundLifecycle;
use crate::tools::todo::{TodoSnapshot, TodoStatus};
use crate::tools::types::{
    ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionResult, ToolOrigin, ToolProgress,
    ToolReplayPolicy,
};

/// The client-visible durable-authority failure state of a conversation
/// runtime (Issue #63).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDurabilityFailure {
    /// The operation that failed persistently.
    pub operation: String,
    /// The human-readable failure diagnostic.
    pub diagnostic: String,
}

/// The authoritative Runtime Client snapshot of one conversation runtime.
///
/// Every section is a deterministic projection of one authoritative
/// runtime owner. The shape belongs to Runtime Client Protocol v4: internal
/// snapshot types are projected into these external DTOs, never exposed
/// directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientSnapshot {
    /// The conversation this snapshot belongs to.
    pub conversation_id: ConversationId,
    /// Whether runtime drain has begun and new inbound admission is closed.
    /// The correlated shutdown response resolves only after quiescence.
    pub shutting_down: bool,
    /// The authoritative mode used by the current attempt boundary.
    pub effective_approval_mode: ApprovalMode,
    /// The latest requested mode when it is waiting for an attempt to settle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_approval_mode: Option<ApprovalMode>,
    /// The runtime control-plane revision of the mode state.
    #[serde(default)]
    pub approval_mode_revision: u64,
    /// The runtime's durable-authority failure, when it has entered the
    /// explicit degraded state. While set, no new durable admission/execution
    /// work may begin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability_failure: Option<RuntimeDurabilityFailure>,
    /// The client projection of canonical Message Ledger observations.
    ///
    /// It is repaired from the native durable authority at bootstrap and
    /// from committed observations while live; it is never independently
    /// mutable or recovery input. A restarted projection contains the
    /// current Surface working set, while historical Ledger pages remain
    /// available through `ConversationStore` APIs.
    pub messages: Vec<MessageBlock>,
    /// The bounded newest page of the derived durable transcript. This is
    /// distinct from `messages`: the latter is the current Surface working
    /// set, while this page remains readable after compaction retires Surface
    /// messages. Older pages are fetched through `transcript_page_get`.
    pub transcript: RuntimeClientTranscriptPage,
    /// The current/latest attempt view, when any attempt exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<RuntimeClientAttempt>,
    /// The inbound mailbox diagnostics (pending items and the latest
    /// finite drain observation).
    pub inbound: InboundDiagnostics,
    /// Live process-owned native interaction requests. This is projection
    /// state, never durable recovery input or client-owned truth.
    #[serde(default)]
    pub pending_interactions: Vec<InteractionRequest>,
    /// All background executions in execution allocation order, including
    /// terminal records retained by the authoritative registry.
    #[serde(default)]
    pub background: Vec<RuntimeClientBackgroundExecution>,
    /// All subagent children in subagent ordinal order, including terminal
    /// records retained by the authoritative registry (Issue #60).
    #[serde(default)]
    pub subagents: Vec<RuntimeClientSubagent>,
    /// The latest composed Agent Status observation, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AgentStatusView>,
    /// Runtime-owned context-compaction diagnostics. The values describe
    /// committed `RuntimeEvent::CompactionCompleted` facts by identity;
    /// they are never a second Conversation Surface authority and never
    /// carry summary content (the committed summary itself appears in
    /// `messages`, like every other canonical Ledger fact).
    #[serde(default)]
    pub context: RuntimeClientContextView,
    /// The active capability projection.
    pub capabilities: CapabilityView,
    /// The active runtime resource generation: the project context files
    /// the runtime actually loaded, and whether an agent profile is frozen
    /// into the generation.
    ///
    /// This is deliberately separate from [`CapabilityView`]: a
    /// resource-only reload advances the resource revision while the
    /// capability revision stays put. Nothing here is conversation content
    /// — a project context file is request input the runtime assembles into
    /// the Effective System Prompt, never a canonical Ledger message.
    #[serde(default)]
    pub resources: RuntimeClientResourcesView,
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
    /// The conversation's task list, as of the newest committed `todo`
    /// result.
    ///
    /// This is a **projection of canonical history, not a second
    /// authority**: the runtime derives it from exactly the tool results the
    /// Ledger holds, the same fact the runtime's own
    /// [`ConversationTodoList`] is rebuilt from, so the two can never
    /// disagree.
    ///
    /// It is carried here rather than left for a client to scan out of the
    /// transcript because a client holds only a bounded newest page of that
    /// transcript. A conversation that committed a page or more of messages
    /// after its last `todo` result would otherwise attach with no list at
    /// all, and would appear to have none until the reader happened to page
    /// far enough back — while the runtime, reading the whole Ledger, still
    /// had one.
    ///
    /// A conversation that never called `todo` carries the empty list.
    ///
    /// [`ConversationTodoList`]: crate::tools::todo::ConversationTodoList
    #[serde(default)]
    pub todos: TodoSnapshot,
}

/// One bounded newest-or-older page of derived transcript history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientTranscriptPage {
    /// Items in chronological order within this page.
    #[serde(default)]
    pub entries: Vec<RuntimeClientTranscriptEntry>,
    /// The exclusive cursor for the next older page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<RuntimeClientTranscriptCursor>,
}

/// One derived transcript item and its stable durable cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeClientTranscriptEntry {
    /// The durable transcript position, not the Runtime Client event cursor.
    pub cursor: RuntimeClientTranscriptCursor,
    /// The typed item resolved from a canonical durable owner.
    pub item: RuntimeClientTranscriptItem,
}

/// One live-published requested interaction audit projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeClientTranscriptInteractionRequested {
    /// Durable Event Journal event identity.
    pub event_id: EventId,
    /// Event timestamp.
    pub timestamp: DateTime<Utc>,
    /// Owning attempt.
    pub attempt_id: AttemptId,
    /// Owning turn.
    pub turn_id: TurnId,
    /// Interaction identity.
    pub interaction_id: InteractionId,
    /// Bounded durable subject.
    pub subject: InteractionSubject,
}

/// One live-published settled interaction audit projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeClientTranscriptInteractionSettled {
    /// Durable Event Journal event identity.
    pub event_id: EventId,
    /// Event timestamp.
    pub timestamp: DateTime<Utc>,
    /// Owning attempt.
    pub attempt_id: AttemptId,
    /// Owning turn.
    pub turn_id: TurnId,
    /// Interaction identity.
    pub interaction_id: InteractionId,
    /// Bounded durable settlement.
    pub settlement: InteractionSettlement,
}

/// The explicit Runtime Client transcript vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientTranscriptItem {
    /// A user, Assistant, or Tool message body from Pending Inbound or Ledger.
    Message {
        /// The canonical or durably accepted message.
        message: MessageBlock,
    },
    /// A noncanonical Assistant publication audit.
    PublicationAudit {
        /// The bounded immutable publication audit.
        audit: PublicationAudit,
    },
    /// A historical interaction request audit. It is never a live waiter.
    InteractionRequested {
        /// Durable Event Journal event identity.
        event_id: EventId,
        /// Event timestamp.
        timestamp: DateTime<Utc>,
        /// Owning attempt.
        attempt_id: AttemptId,
        /// Owning turn.
        turn_id: TurnId,
        /// Interaction identity.
        interaction_id: InteractionId,
        /// Bounded durable subject.
        subject: InteractionSubject,
    },
    /// A historical interaction settlement audit. It is never actionable.
    InteractionSettled {
        /// Durable Event Journal event identity.
        event_id: EventId,
        /// Event timestamp.
        timestamp: DateTime<Utc>,
        /// Owning attempt.
        attempt_id: AttemptId,
        /// Owning turn.
        turn_id: TurnId,
        /// Interaction identity.
        interaction_id: InteractionId,
        /// Bounded durable settlement.
        settlement: InteractionSettlement,
    },
}

/// The cursor domain of durable transcript paging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeClientTranscriptCursor(u64);

impl RuntimeClientTranscriptCursor {
    /// Creates a transcript cursor from its wire value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the wire value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<crate::durable::TranscriptCursor> for RuntimeClientTranscriptCursor {
    fn from(cursor: crate::durable::TranscriptCursor) -> Self {
        Self::new(cursor.get())
    }
}

impl From<RuntimeClientTranscriptCursor> for crate::durable::TranscriptCursor {
    fn from(cursor: RuntimeClientTranscriptCursor) -> Self {
        Self::new(cursor.get())
    }
}

/// Converts one durable transcript page into the explicit Runtime Client
/// read model. Invalid durable interaction shapes are rejected rather than
/// fabricated into a display item.
pub(crate) fn transcript_page_view(
    page: crate::durable::TranscriptPage,
) -> Result<RuntimeClientTranscriptPage, String> {
    let entries = page
        .entries
        .into_iter()
        .map(|entry| {
            let item = match entry.item {
                crate::durable::TranscriptItem::Message { message } => {
                    RuntimeClientTranscriptItem::Message { message }
                }
                crate::durable::TranscriptItem::PublicationAudit { audit } => {
                    RuntimeClientTranscriptItem::PublicationAudit { audit }
                }
                crate::durable::TranscriptItem::InteractionRequested { event } => {
                    let RuntimeEventEnvelope {
                        event_id,
                        timestamp,
                        attempt_id: Some(attempt_id),
                        turn_id: Some(turn_id),
                        event:
                            crate::events::types::RuntimeEvent::InteractionRequested {
                                interaction_id,
                                subject,
                            },
                        ..
                    } = event
                    else {
                        return Err(
                            "durable transcript requested interaction has an invalid envelope"
                                .to_owned(),
                        );
                    };
                    RuntimeClientTranscriptItem::InteractionRequested {
                        event_id,
                        timestamp,
                        attempt_id,
                        turn_id,
                        interaction_id,
                        subject,
                    }
                }
                crate::durable::TranscriptItem::InteractionSettled { event } => {
                    let RuntimeEventEnvelope {
                        event_id,
                        timestamp,
                        attempt_id: Some(attempt_id),
                        turn_id: Some(turn_id),
                        event:
                            crate::events::types::RuntimeEvent::InteractionSettled {
                                interaction_id,
                                settlement,
                            },
                        ..
                    } = event
                    else {
                        return Err(
                            "durable transcript settled interaction has an invalid envelope"
                                .to_owned(),
                        );
                    };
                    RuntimeClientTranscriptItem::InteractionSettled {
                        event_id,
                        timestamp,
                        attempt_id,
                        turn_id,
                        interaction_id,
                        settlement,
                    }
                }
            };
            Ok(RuntimeClientTranscriptEntry {
                cursor: entry.cursor.into(),
                item,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(RuntimeClientTranscriptPage {
        entries,
        next_cursor: page.next_cursor.map(Into::into),
    })
}

/// Converts one live requested interaction audit envelope into its client
/// transcript view.
pub(crate) fn interaction_requested_view(
    event: RuntimeEventEnvelope,
) -> Result<RuntimeClientTranscriptInteractionRequested, String> {
    let RuntimeEventEnvelope {
        event_id,
        timestamp,
        attempt_id: Some(attempt_id),
        turn_id: Some(turn_id),
        event:
            crate::events::types::RuntimeEvent::InteractionRequested {
                interaction_id,
                subject,
            },
        ..
    } = event
    else {
        return Err("interaction requested audit has an invalid envelope".to_owned());
    };
    Ok(RuntimeClientTranscriptInteractionRequested {
        event_id,
        timestamp,
        attempt_id,
        turn_id,
        interaction_id,
        subject,
    })
}

/// Converts one live settled interaction audit envelope into its client
/// transcript view.
pub(crate) fn interaction_settled_view(
    event: RuntimeEventEnvelope,
) -> Result<RuntimeClientTranscriptInteractionSettled, String> {
    let RuntimeEventEnvelope {
        event_id,
        timestamp,
        attempt_id: Some(attempt_id),
        turn_id: Some(turn_id),
        event:
            crate::events::types::RuntimeEvent::InteractionSettled {
                interaction_id,
                settlement,
            },
        ..
    } = event
    else {
        return Err("interaction settled audit has an invalid envelope".to_owned());
    };
    Ok(RuntimeClientTranscriptInteractionSettled {
        event_id,
        timestamp,
        attempt_id,
        turn_id,
        interaction_id,
        settlement,
    })
}

/// The context diagnostics carried by the Runtime Client snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientContextView {
    /// Whether the runtime currently owns a context-compaction operation.
    /// This is live operation state, not inferred from token usage.
    pub compaction_in_progress: bool,
    /// Runtime Client projection statistic: the number of committed
    /// compaction completions folded into this read model. The compaction
    /// generation remains the conversation-owned identity.
    pub compaction_count: u64,
    /// The latest committed compaction metadata, when compaction occurred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_compaction: Option<RuntimeClientCompactionView>,
}

/// Public metadata for one committed compaction.
///
/// Every field is derived from already-committed conversation state. The
/// view names the canonical summary message by identity; its content is an
/// ordinary Ledger fact in [`RuntimeClientSnapshot::messages`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientCompactionView {
    /// The compaction generation maintained in the current Conversation
    /// Surface head.
    pub generation: u64,
    /// The identity of the committed canonical compaction summary message.
    pub summary_message_id: MessageId,
    /// The Conversation Surface revision established by the rewrite.
    pub surface_revision: SurfaceRevision,
    /// The pre-compaction input measurement and its provenance.
    pub tokens_before: TokenMeasurement,
    /// The deterministic estimate of the rebuilt request context.
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
    /// The in-flight Assistant output, when a message is streaming: enough
    /// accumulated state to repair every client-visible streaming effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<InFlightAssistantMessage>,
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

/// The accumulated in-flight output of one streaming Assistant message.
///
/// This is the repair state of streaming: a snapshot taken mid-stream
/// carries every accumulated delta through its cursor, so a client
/// repairing after `resync` reconstructs the exact message it would have
/// observed incrementally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InFlightAssistantMessage {
    /// The provisional message identity.
    pub message_id: MessageId,
    /// The ordered content blocks assembled so far.
    #[serde(default)]
    pub blocks: Vec<InFlightBlock>,
}

/// One ordered block of an in-flight Assistant message.
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
/// container shape belongs to Runtime Client Protocol v4 while the
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

/// The Runtime Client view of one subagent child (Issue #60).
///
/// A read-model materialization of the authoritative registry snapshot:
/// every field is derived, and the durable ownership/terminal events —
/// never this view — are the recovery authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientSubagent {
    /// The conversation-owned subagent identity.
    pub subagent_id: crate::runtime::identity::SubagentId,
    /// The child agent identity (the provenance its answer carries).
    pub child_agent_id: crate::runtime::identity::AgentId,
    /// The child's own durable conversation identity.
    pub child_conversation_id: ConversationId,
    /// The frozen profile identity.
    pub profile: String,
    /// The authoritative lifecycle state.
    pub state: crate::runtime::subagent::SubagentState,
    /// The bounded terminal detail (result content, failure diagnostic, or
    /// cancellation detail), once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
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
    /// The canonical Agent Status User message described by this view.
    pub status_message_id: MessageId,
    /// The delivery opportunities that made this generation eligible.
    pub opportunities: AgentStatusOpportunityView,
    /// The ordered structured sections.
    pub sections: Vec<RuntimeClientStatusSection>,
    /// The canonical rendered representation, derived from the same
    /// composition as the sections.
    pub rendered: String,
}

/// One structured Agent Status section of the external view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientStatusSection {
    /// The typed Time payload.
    Temporal {
        /// The runtime clock value sampled at composition time.
        current_time: DateTime<Utc>,
        /// The Time status timezone, when configured.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<Tz>,
    },
    /// The runtime-owned background-execution section.
    BackgroundExecutions {
        /// The active background executions in allocation order.
        executions: Vec<RuntimeClientBackgroundExecution>,
        /// Active executions omitted by the module-local bound.
        omitted_count: usize,
    },
    /// The bounded conversation-owned Todo reminder.
    Todo {
        /// The first committed in-progress task, when any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current: Option<RuntimeClientTodoStatusTask>,
        /// Remaining committed active tasks in creation order.
        #[serde(default)]
        tasks: Vec<RuntimeClientTodoStatusTask>,
        /// Number of committed active tasks.
        active_count: usize,
        /// Number of active tasks blocked by active dependencies.
        blocked_count: usize,
        /// Number of committed completed tasks.
        completed_count: usize,
        /// Number of committed deleted tasks.
        deleted_count: usize,
        /// Number of active tasks omitted from the bounded view.
        omitted_count: usize,
    },
}

/// One bounded Todo task in the Agent Status client view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientTodoStatusTask {
    /// The conversation-owned task id.
    pub id: u64,
    /// The bounded task subject.
    pub subject: String,
    /// The bounded in-progress label, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    /// The committed lifecycle status.
    pub status: TodoStatus,
    /// Whether an active dependency still blocks this task.
    pub blocked: bool,
}

/// The external view of the Agent Status opportunity set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStatusOpportunityView {
    /// The `FreshInbound` opportunity that produced this status, when one is
    /// present. Future delivery opportunities can be added alongside it
    /// without making this member structurally mandatory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_inbound: Option<FreshInboundStatusOpportunityView>,
    /// The complete settled tool batch that made this existing primary step
    /// eligible, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_tool_batch: Option<PostToolBatchStatusOpportunityView>,
}

/// The external view of one `FreshInbound` status opportunity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshInboundStatusOpportunityView {
    /// The inbound message that made status generation eligible.
    pub target_message_id: MessageId,
}

/// Minimal external representation of the batch-level `PostToolBatch`
/// opportunity. The marker has no durable or scheduling metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PostToolBatchStatusOpportunityView {}

/// The deterministic capability projection.
///
/// Projected from the active [`CapabilitySnapshot`]
/// ([`crate::capabilities::CapabilitySnapshot`]) plus the
/// coordinator-owned availability state (Issue #81): the revision, the
/// active Tool catalog, the complete available Tool catalog, the deterministic
/// model-visible Skill catalog, and the typed per-source availability. No
/// executors, environment paths, package-manager state, or private dependency
/// internals appear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityView {
    /// The active monotonic capability revision.
    pub revision: CapabilityRevision,
    /// The deterministic active Tool catalog in registry order. Model
    /// requests and execution use exactly this set.
    #[serde(default)]
    pub tools: Vec<RuntimeClientTool>,
    /// The complete available Tool catalog, including inactive Tools. The
    /// active set above is an explicit subset; availability never implies
    /// model activation.
    #[serde(default)]
    pub available_tools: Vec<RuntimeClientTool>,
    /// The deterministic model-visible Skill catalog ordered by Skill name.
    /// Skills hidden by `disable-model-invocation` remain runtime-owned but
    /// are omitted here. Every entry includes the canonical absolute host
    /// path of its `SKILL.md`.
    #[serde(default)]
    pub skills: Vec<RuntimeClientSkill>,
    /// The typed availability of every evaluated optional capability
    /// source, in deterministic source-identity order (Issue #81).
    #[serde(default)]
    pub sources: Vec<CapabilitySourceView>,
}

/// The client-visible identity of one optional capability source (Issue
/// #81).
///
/// Native tools never appear here: their construction is part of the core
/// runtime and remains fatal at composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CapabilitySourceDescriptor {
    /// The custom Python tool plane.
    Python,
    /// One configured MCP server.
    Mcp {
        /// The authoritative server identity.
        server_id: crate::runtime::identity::McpServerId,
    },
}

/// The client-visible availability of one optional capability source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CapabilitySourceStateView {
    /// The source initialized; its capabilities are usable.
    Ready,
    /// The source is unavailable; `reason` is the bounded diagnostic.
    Unavailable {
        /// The bounded failure diagnostic.
        reason: String,
    },
}

/// One optional capability source's availability projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitySourceView {
    /// The stable source identity.
    pub source: CapabilitySourceDescriptor,
    /// The authoritative availability state.
    pub state: CapabilitySourceStateView,
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
    /// Whether execution requires a native approval interaction.
    pub approval_policy: crate::tools::types::ToolApprovalPolicy,
    /// The replay policy.
    pub replay_policy: ToolReplayPolicy,
    /// Where the tool comes from.
    pub origin: ToolOrigin,
}

/// The client-visible projection of one immutable runtime resource
/// generation.
///
/// It carries identity and provenance, never content: a client that wants
/// the text of a project context file reads the file, exactly as it would
/// read a `SKILL.md`. Publishing the bytes here would create a second copy
/// of request input inside a conversation projection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientResourcesView {
    /// The monotonic process-local resource generation.
    #[serde(default)]
    pub revision: RuntimeResourceRevision,
    /// The runtime-loaded project instruction files, root-most to
    /// workspace, in the exact order the runtime concatenated them.
    #[serde(default)]
    pub context_files: Vec<RuntimeClientContextFile>,
    /// Whether an immutable agent profile/persona is frozen into this
    /// generation. The persona text itself is runtime-owned request input.
    #[serde(default)]
    pub agent_profile: bool,
}

/// One runtime-loaded project instruction file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientContextFile {
    /// The canonical absolute host path the runtime read.
    pub path: String,
    /// The exact byte length of the loaded content.
    pub bytes: u64,
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
    /// The canonical absolute host path of the package's `SKILL.md`.
    ///
    /// This is a real filesystem path, not a runtime-owned virtual locator:
    /// a Skill package is an ordinary host directory, and the same path
    /// serves Read, Bash, Grep, and Glob alike.
    pub location: String,
}

impl RuntimeClientSnapshot {
    /// The conversation identity the snapshot belongs to.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }
}
