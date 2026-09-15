//! Read-only Trace presentation over native durable authorities.
//!
//! No value in this module is history, execution authority, or recovery input.
//! Reads select indexed Journal anchors and join immutable Request Snapshots
//! and canonical Ledger messages by exact identity. The browser never folds
//! Journal events. A read cut bounds all joins, including concurrent settlement.
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::durable::presentation::{FactQuery, FactScope};
use crate::durable::{ConversationStore, ConversationStoreError};
use crate::events::types::{RuntimeEvent as E, RuntimeEventEnvelope};
use crate::message::types::{AssistantContentBlock, MessageBlock};
use crate::model::types::ModelUsage;
use crate::runtime::identity::{
    ArtifactId, AttemptId, MessageId, RequestId, ToolCallId, ToolId, TurnId,
};
use crate::tools::types::ToolExecutionStatus;

pub const TRACE_PAGE_LIMIT: usize = 32;
pub const TRACE_RECORD_LIMIT: usize = 512;
pub const TRACE_TEXT_BYTES: usize = 2048;
pub const TRACE_BLOCK_LIMIT: usize = 8;
const ANCHORS: &[&str] = &[
    "attempt_started",
    "turn_started",
    "model_request_started",
    "assistant_message_committed",
    "tool_execution_started",
    "compaction_started",
    "background_execution_committed",
    "subagent_ownership_committed",
    "workflow_started",
    "interaction_requested",
];

/// Opaque Trace-only exclusive boundary. Valid only in its conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct TraceCursor(String);
impl TraceCursor {
    fn at(sequence: u64) -> Self {
        Self(format!("trace:{sequence}"))
    }
    fn sequence(&self) -> Result<u64, ConversationStoreError> {
        self.0
            .strip_prefix("trace:")
            .and_then(|s| s.parse().ok())
            .filter(|n| i64::try_from(*n).is_ok())
            .ok_or_else(|| ConversationStoreError::InvalidReference("invalid Trace cursor".into()))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracePage {
    pub entries: Vec<TraceEntry>,
    pub next_cursor: Option<TraceCursor>,
}

/// Refresh of a loaded record, resolved by the server at the snapshot cut.
/// No browser lifecycle inference or replacement of canonical payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceLifecycle {
    pub id: String,
    pub state: TraceState,
    pub timing: TraceTiming,
    pub request: Option<TraceRequestOutcome>,
    pub message_id: Option<MessageId>,
    pub artifacts: Vec<TraceArtifact>,
    pub truncated: bool,
}

/// Mutable request outcome only; immutable historical input is not repeated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceRequestOutcome {
    pub failure_kind: Option<crate::model::error::ModelErrorKind>,
    pub usage: Option<ModelUsage>,
}

/// Server-resolved grouping. Native `TurnId` is the logical model step within
/// an Attempt; actual requests never allocate a new step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceLocation {
    pub attempt_id: Option<AttemptId>,
    pub step_id: Option<TurnId>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    Attempt,
    Step,
    Request,
    Assistant,
    Tool,
    Compaction,
    Background,
    Subagent,
    Workflow,
    Interaction,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceState {
    Incomplete,
    Running,
    Pending,
    Cancelling,
    Settling,
    Waiting,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Limited,
    Denied,
    OutcomeUnknown,
    Interrupted,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceText {
    pub text: String,
    pub truncated: bool,
    pub redacted: bool,
}
impl TraceText {
    fn visible(text: &str) -> Self {
        let mut end = text.len().min(TRACE_TEXT_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            text: text[..end].into(),
            truncated: end < text.len(),
            redacted: false,
        }
    }
    fn withheld() -> Self {
        Self {
            text: String::new(),
            truncated: false,
            redacted: true,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceTiming {
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceRequest {
    pub request_id: RequestId,
    pub retry_number: u32,
    pub assistant_message_id: MessageId,
    /// Previous actual request failure, proven through the native retry ordinal.
    pub previous_failure_kind: Option<crate::model::error::ModelErrorKind>,
    pub model: TraceText,
    pub max_output_tokens: u32,
    pub reasoning_enabled: bool,
    /// Exact request input is internal. These sections are explicitly withheld,
    /// never replaced by today's configuration or reconstructed in the browser.
    pub effective_system_prompt: TraceText,
    pub context_input: TraceText,
    pub tool_schema: TraceText,
    pub failure_kind: Option<crate::model::error::ModelErrorKind>,
    pub usage: Option<ModelUsage>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceTool {
    pub call_id: ToolCallId,
    pub tool_id: ToolId,
    pub arguments: TraceText,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceEntry {
    pub id: String,
    /// Trace order, only for ordering this read model; not an event cursor.
    pub position: TraceCursor,
    pub location: TraceLocation,
    pub kind: TraceKind,
    pub state: TraceState,
    pub timing: TraceTiming,
    pub request: Option<TraceRequest>,
    pub tool: Option<TraceTool>,
    /// Accepted canonical `ToolCalls`, in canonical block order. Not start evidence.
    pub calls: Vec<TraceTool>,
    /// Exact native detached execution / child / Workflow run / interaction ID.
    pub native_id: Option<String>,
    /// Canonical output only. Publication without acceptance is not copied here.
    pub message_id: Option<MessageId>,
    pub output: Vec<TraceText>,
    pub reasoning: Vec<TraceText>,
    pub artifacts: Vec<TraceArtifact>,
    pub truncated: bool,
}

/// Safe reference to the existing native artifact carrier, never a storage path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceArtifact {
    pub artifact_id: ArtifactId,
    pub image: bool,
}

/// Stateless, read-only presentation owner. Dropping it changes no native state.
pub struct TraceProjection<'a> {
    store: &'a dyn ConversationStore,
    through: u64,
}
impl<'a> TraceProjection<'a> {
    /// Materialize the exact prefix captured by the native observation cut.
    pub(crate) fn through(store: &'a dyn ConversationStore, through: u64) -> Self {
        Self { store, through }
    }

    /// Capture one durable read cut. All subsequent joins exclude newer facts.
    /// # Errors
    /// Returns the durable read error without interpreting it as absent history.
    pub fn new(store: &'a dyn ConversationStore) -> Result<Self, ConversationStoreError> {
        Ok(Self {
            store,
            through: store.presentation_frontier()?,
        })
    }
    fn facts(
        &self,
        scope: FactScope,
        kinds: &[&'static str],
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<RuntimeEventEnvelope>, ConversationStoreError> {
        self.store.read_presentation_events(&FactQuery {
            scope,
            kinds: kinds.to_vec(),
            before,
            after: 0,
            ascending: false,
            through: self.through,
            limit,
        })
    }
    /// Newest or next-older finite page. Reads never touch live observation state.
    /// # Errors
    /// Rejects invalid cursors/limits and propagates failed authoritative reads.
    pub fn page(
        &self,
        before: Option<&TraceCursor>,
        limit: usize,
    ) -> Result<TracePage, ConversationStoreError> {
        if limit == 0 || limit > TRACE_PAGE_LIMIT {
            return Err(ConversationStoreError::InvalidReference(
                "Trace limit must be 1..=32".into(),
            ));
        }
        let mut anchors = self.facts(
            FactScope::All,
            ANCHORS,
            before.map(TraceCursor::sequence).transpose()?,
            limit + 1,
        )?;
        let more = anchors.len() > limit;
        anchors.truncate(limit);
        anchors.reverse();
        let mut next_cursor = more.then(|| TraceCursor::at(anchors[0].sequence));
        let mut entries: Vec<TraceEntry> = anchors
            .iter()
            .map(|anchor| self.entry(anchor))
            .collect::<Result<_, _>>()?;
        // Bound encoded bytes, including JSON escaping, not just character count.
        for entry in &mut entries {
            bound_entry(entry);
        }
        while entries.len() > 1
            && serde_json::to_vec(&entries)
                .map_err(|_| {
                    ConversationStoreError::InvalidReference("Trace encoding failed".into())
                })?
                .len()
                > 128 * 1024
        {
            entries.remove(0);
            next_cursor = Some(entries[0].position.clone());
        }
        Ok(TracePage {
            entries,
            next_cursor,
        })
    }
    pub(crate) fn refresh(
        &self,
        records: &[TraceCursor],
        snapshot: &super::snapshot::RuntimeClientSnapshot,
    ) -> Result<Vec<TraceLifecycle>, ConversationStoreError> {
        if records.len() > TRACE_RECORD_LIMIT {
            return Err(ConversationStoreError::InvalidReference(
                "too many Trace records".into(),
            ));
        }
        records
            .iter()
            .map(|cursor| {
                let sequence = cursor.sequence()?;
                let anchor = self
                    .store
                    .read_presentation_events(&FactQuery {
                        scope: FactScope::All,
                        kinds: ANCHORS.to_vec(),
                        before: Some(sequence.saturating_add(1)),
                        after: sequence.saturating_sub(1),
                        ascending: false,
                        through: self.through,
                        limit: 1,
                    })?
                    .pop();
                let Some(anchor) = anchor.filter(|event| event.sequence == sequence) else {
                    return Ok(None);
                };
                let mut entry = self.entry(&anchor)?;
                repair_entries(std::slice::from_mut(&mut entry), snapshot);
                bound_entry(&mut entry);
                let mut update = TraceLifecycle {
                    id: entry.id,
                    state: entry.state,
                    timing: entry.timing,
                    request: entry.request.map(|request| TraceRequestOutcome {
                        failure_kind: request.failure_kind,
                        usage: request.usage,
                    }),
                    message_id: entry.message_id,
                    artifacts: entry.artifacts,
                    truncated: entry.truncated,
                };
                // Leave room for the ordinary snapshot in the 1 MiB transport.
                // Oversized optional references are visibly partial, never an
                // excuse to omit the lifecycle of a loaded active record.
                if serde_json::to_vec(&update)
                    .expect("typed Trace update")
                    .len()
                    > 1024
                {
                    update.artifacts.clear();
                    update.message_id = None;
                    update.truncated = true;
                }
                Ok(Some(update))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|updates| updates.into_iter().flatten().collect())
    }

    fn ending(
        &self,
        scope: FactScope,
        kinds: &[&'static str],
    ) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
        Ok(self.facts(scope, kinds, None, 1)?.pop())
    }
    #[allow(clippy::too_many_lines)] // Explicit allowlist; no internal serialization fallback.
    fn entry(&self, anchor: &RuntimeEventEnvelope) -> Result<TraceEntry, ConversationStoreError> {
        let mut entry = TraceEntry {
            id: format!("trace:{}", anchor.sequence),
            position: TraceCursor::at(anchor.sequence),
            location: TraceLocation {
                attempt_id: anchor.attempt_id.clone(),
                step_id: anchor.turn_id.clone(),
            },
            kind: TraceKind::Step,
            state: TraceState::Incomplete,
            timing: TraceTiming {
                started_at: anchor.timestamp,
                ended_at: None,
                duration_ms: None,
            },
            request: None,
            tool: None,
            calls: vec![],
            native_id: None,
            message_id: None,
            output: vec![],
            reasoning: vec![],
            artifacts: vec![],
            truncated: false,
        };
        let ending = match &anchor.event {
            E::AttemptStarted { attempt_id } => {
                entry.kind = TraceKind::Attempt;
                self.ending(
                    FactScope::Attempt(attempt_id.clone()),
                    &[
                        "attempt_completed",
                        "attempt_failed",
                        "attempt_cancelled",
                        "attempt_timed_out",
                        "attempt_limit_exceeded",
                    ],
                )?
            }
            E::TurnStarted => {
                if let (Some(attempt), Some(turn)) = (&anchor.attempt_id, &anchor.turn_id) {
                    self.ending(
                        FactScope::Step(attempt.clone(), turn.clone()),
                        &["turn_completed"],
                    )?
                } else {
                    None
                }
            }
            E::ModelRequestStarted { request_id, .. } => {
                entry.kind = TraceKind::Request;
                let frozen = self.store.load_request_snapshot(request_id)?;
                entry.location = TraceLocation {
                    attempt_id: Some(frozen.identity.attempt_id.clone()),
                    step_id: Some(frozen.identity.turn.clone()),
                };
                let end = self.ending(
                    FactScope::Request(request_id.to_string()),
                    &["model_request_completed", "model_request_failed"],
                )?;
                let (failure_kind, usage) = match end.as_ref().map(|e| &e.event) {
                    Some(E::ModelRequestCompleted { usage, .. }) => (None, usage.clone()),
                    Some(E::ModelRequestFailed { error, usage, .. }) => {
                        (Some(error.kind.clone()), usage.clone())
                    }
                    _ => (None, None),
                };
                let previous_failure_kind = if frozen.identity.retry_number > 0 {
                    let mut identity = frozen.identity.clone();
                    identity.retry_number -= 1;
                    self.ending(
                        FactScope::Request(identity.request_id().to_string()),
                        &["model_request_failed"],
                    )?
                    .and_then(|event| match event.event {
                        E::ModelRequestFailed { error, .. } => Some(error.kind),
                        _ => None,
                    })
                } else {
                    None
                };
                entry.request = Some(TraceRequest {
                    previous_failure_kind,
                    assistant_message_id: frozen.provisional_message_id,
                    request_id: request_id.clone(),
                    retry_number: frozen.identity.retry_number,
                    model: TraceText::visible(&frozen.invocation.model),
                    max_output_tokens: frozen.invocation.max_output_tokens,
                    reasoning_enabled: frozen.reasoning_enabled,
                    effective_system_prompt: TraceText::withheld(),
                    context_input: TraceText::withheld(),
                    tool_schema: TraceText::withheld(),
                    failure_kind,
                    usage,
                });
                end
            }
            E::AssistantMessageCommitted { message_id } => {
                entry.kind = TraceKind::Assistant;
                entry.state = TraceState::Completed;
                entry.message_id = Some(message_id.clone());
                for message in self.store.load_messages(std::slice::from_ref(message_id))? {
                    if let MessageBlock::Assistant(message) = message {
                        entry.truncated = message.content.len() > TRACE_BLOCK_LIMIT;
                        for block in message.content.iter().take(TRACE_BLOCK_LIMIT) {
                            match block {
                                AssistantContentBlock::Text(block) => {
                                    entry.output.push(TraceText::visible(&block.text));
                                }
                                AssistantContentBlock::Refusal(block) => {
                                    entry.output.push(TraceText::visible(&block.text));
                                }
                                AssistantContentBlock::Reasoning(block) => {
                                    if let Some(text) = &block.text {
                                        entry.reasoning.push(TraceText::visible(text));
                                    }
                                }
                                AssistantContentBlock::Image(image) => {
                                    entry.artifacts.push(TraceArtifact {
                                        artifact_id: image.artifact_id.clone(),
                                        image: true,
                                    });
                                }
                                AssistantContentBlock::ToolCall(call) => {
                                    entry.calls.push(TraceTool {
                                        call_id: call.id.clone(),
                                        tool_id: call.tool_id.clone(),
                                        arguments: TraceText::withheld(),
                                    });
                                }
                            }
                        }
                    }
                }
                None
            }
            E::ToolExecutionStarted {
                tool_call_id,
                tool_id,
            } => {
                entry.kind = TraceKind::Tool;
                entry.tool = Some(TraceTool {
                    call_id: tool_call_id.clone(),
                    tool_id: tool_id.clone(),
                    arguments: TraceText::withheld(),
                });
                let end = self.ending(
                    FactScope::ToolCall {
                        call_id: tool_call_id.to_string(),
                        attempt: anchor.attempt_id.clone(),
                        turn: anchor.turn_id.clone(),
                    },
                    &["tool_execution_completed", "tool_execution_failed"],
                )?;
                // Both call and Tool IDs must match. Never pair names or positions.
                let end = end.filter(|e| match &e.event {
                    E::ToolExecutionCompleted { tool_id: id, .. }
                    | E::ToolExecutionFailed { tool_id: id, .. } => id == tool_id,
                    _ => false,
                });
                if let Some(commit) = self.ending(
                    FactScope::ToolCall {
                        call_id: tool_call_id.to_string(),
                        attempt: anchor.attempt_id.clone(),
                        turn: anchor.turn_id.clone(),
                    },
                    &["tool_message_committed"],
                )? && let E::ToolMessageCommitted { message_id, .. } = commit.event
                {
                    for message in self
                        .store
                        .load_messages(std::slice::from_ref(&message_id))?
                    {
                        if let MessageBlock::Tool(message) = message
                            && message.tool_call_id == *tool_call_id
                            && message.tool_id == *tool_id
                        {
                            entry.message_id = Some(message_id.clone());
                            entry.truncated = message.result.content.len() > TRACE_BLOCK_LIMIT;
                            for content in message.result.content.iter().take(TRACE_BLOCK_LIMIT) {
                                match content {
                                    crate::tools::types::ToolResultContent::Image(image) => {
                                        entry.artifacts.push(TraceArtifact {
                                            artifact_id: image.artifact_id.clone(),
                                            image: true,
                                        });
                                    }
                                    crate::tools::types::ToolResultContent::File(file) => {
                                        entry.artifacts.push(TraceArtifact {
                                            artifact_id: file.artifact_id.clone(),
                                            image: false,
                                        });
                                    }
                                    // Arbitrary Tool output can include executor secrets/paths.
                                    _ => entry.output.push(TraceText::withheld()),
                                }
                            }
                        }
                    }
                }
                end
            }
            E::CompactionStarted => {
                entry.kind = TraceKind::Compaction;
                // Compaction has no native operation ID. The exact next
                // start/terminal boundary is selected in durable order below.
                self.store
                    .read_presentation_events(&FactQuery {
                        scope: FactScope::All,
                        kinds: vec![
                            "compaction_started",
                            "compaction_completed",
                            "compaction_failed",
                        ],
                        before: None,
                        after: anchor.sequence,
                        ascending: true,
                        through: self.through,
                        limit: 1,
                    })?
                    .pop()
                    .filter(|event| !matches!(event.event, E::CompactionStarted))
            }
            E::BackgroundExecutionCommitted { execution_id, .. } => {
                entry.kind = TraceKind::Background;
                entry.native_id = Some(execution_id.to_string());
                self.ending(
                    FactScope::Execution(execution_id.to_string()),
                    &["background_terminal_published"],
                )?
            }
            E::SubagentOwnershipCommitted { subagent_id, .. } => {
                entry.kind = TraceKind::Subagent;
                entry.native_id = Some(subagent_id.to_string());
                self.ending(
                    FactScope::Subagent(subagent_id.to_string()),
                    &["subagent_terminal_published", "subagent_terminal_settled"],
                )?
            }
            E::WorkflowStarted { run_id, .. } => {
                entry.kind = TraceKind::Workflow;
                let id = serde_json::to_string(run_id).map_err(|_| {
                    ConversationStoreError::InvalidReference("invalid Workflow identity".into())
                })?;
                entry.native_id = Some(id.clone());
                self.ending(
                    FactScope::Workflow(id),
                    &[
                        "workflow_completed",
                        "workflow_failed",
                        "workflow_cancelled",
                    ],
                )?
            }
            E::InteractionRequested { interaction_id, .. } => {
                entry.kind = TraceKind::Interaction;
                entry.native_id = Some(interaction_id.to_string());
                self.ending(
                    FactScope::Interaction(interaction_id.to_string()),
                    &["interaction_settled"],
                )?
            }
            _ => unreachable!("allowlisted anchors only"),
        };
        if let Some(end) = ending.filter(|end| end.sequence > anchor.sequence) {
            entry.state = terminal(&end.event);
            entry.timing.ended_at = Some(end.timestamp);
            entry.timing.duration_ms =
                u64::try_from((end.timestamp - anchor.timestamp).num_milliseconds()).ok();
        }
        Ok(entry)
    }
}
fn terminal(event: &E) -> TraceState {
    match event {
        E::AttemptCompleted { .. }
        | E::TurnCompleted
        | E::ModelRequestCompleted { .. }
        | E::CompactionCompleted { .. }
        | E::WorkflowCompleted { .. } => TraceState::Completed,
        E::InteractionSettled { settlement, .. } => {
            use crate::events::interaction::InteractionSettlement as S;
            match settlement {
                S::Approved | S::Reviewed { .. } | S::QuestionnaireSubmitted { .. } => {
                    TraceState::Completed
                }
                S::Denied { .. } | S::QuestionnaireDeclined => TraceState::Denied,
                S::Cancelled { .. } => TraceState::Cancelled,
                S::DeadlineExpired { .. } => TraceState::TimedOut,
                S::ReviewInvalidated => TraceState::Interrupted,
            }
        }
        E::AttemptCancelled { .. } | E::WorkflowCancelled { .. } => TraceState::Cancelled,
        E::AttemptTimedOut { .. } => TraceState::TimedOut,
        E::AttemptLimitExceeded { .. } => TraceState::Limited,
        E::ModelRequestFailed { error, .. } => match error.kind {
            crate::model::error::ModelErrorKind::Cancelled => TraceState::Cancelled,
            crate::model::error::ModelErrorKind::Timeout => TraceState::TimedOut,
            _ => TraceState::Failed,
        },
        E::AttemptFailed { .. } | E::CompactionFailed { .. } | E::ToolExecutionFailed { .. } => {
            TraceState::Failed
        }
        E::ToolExecutionCompleted { result, .. } => tool_state(&result.status),
        E::WorkflowFailed { status, .. } => tool_state(status),
        E::SubagentTerminalPublished { state, .. } | E::SubagentTerminalSettled { state, .. } => {
            use crate::events::types::SubagentTerminalState as S;
            match state {
                S::Succeeded => TraceState::Completed,
                S::Failed => TraceState::Failed,
                S::Cancelled => TraceState::Cancelled,
                S::Interrupted => TraceState::Interrupted,
            }
        }
        E::BackgroundTerminalPublished { state, .. } => {
            use crate::events::types::BackgroundTerminalState as S;
            match state {
                S::Succeeded => TraceState::Completed,
                S::Failed => TraceState::Failed,
                S::Cancelled => TraceState::Cancelled,
                S::Denied => TraceState::Denied,
                S::TimedOut => TraceState::TimedOut,
                S::OutcomeUnknown => TraceState::OutcomeUnknown,
            }
        }
        _ => TraceState::Incomplete,
    }
}
fn tool_state(state: &ToolExecutionStatus) -> TraceState {
    match state {
        ToolExecutionStatus::Success => TraceState::Completed,
        ToolExecutionStatus::Failed { .. } => TraceState::Failed,
        ToolExecutionStatus::Denied { .. } => TraceState::Denied,
        ToolExecutionStatus::Cancelled { .. } => TraceState::Cancelled,
        ToolExecutionStatus::TimedOut => TraceState::TimedOut,
        ToolExecutionStatus::OutcomeUnknown { .. } => TraceState::OutcomeUnknown,
    }
}

/// Current runtime proof may label an otherwise unresolved durable start as
/// running. Terminal Journal facts always win; no elapsed duration is invented.
pub(crate) fn repair_live(snapshot: &mut super::snapshot::RuntimeClientSnapshot) {
    let mut page = std::mem::take(&mut snapshot.trace);
    repair_entries(&mut page.entries, snapshot);
    snapshot.trace = page;
}
pub(crate) fn repair_entries(
    entries: &mut [TraceEntry],
    snapshot: &super::snapshot::RuntimeClientSnapshot,
) {
    use super::snapshot::{ForegroundToolState, RuntimeClientAttemptPhase};
    use crate::runtime::subagent::SubagentState as S;
    use crate::runtime::workflow::read_model::WorkflowState as W;
    use crate::tools::background::BackgroundLifecycle as B;
    for entry in entries {
        if entry.state != TraceState::Incomplete {
            continue;
        }
        // Only lifecycle labels cross this boundary. No registry payloads,
        // workspace paths, executor environments, or guessed timing do.
        let live = match entry.kind {
            TraceKind::Background => snapshot
                .background
                .iter()
                .find(|current| entry.native_id.as_deref() == Some(current.execution_id.as_str()))
                .and_then(|current| match current.state {
                    B::Starting => Some(TraceState::Pending),
                    B::Running => Some(TraceState::Running),
                    B::Cancelling => Some(TraceState::Cancelling),
                    B::PublishingTerminal => Some(TraceState::Settling),
                    _ => None,
                }),
            TraceKind::Subagent => snapshot
                .subagents
                .iter()
                .find(|current| entry.native_id.as_deref() == Some(current.subagent_id.as_str()))
                .and_then(|current| match current.state {
                    S::Running => Some(TraceState::Running),
                    S::Cancelling => Some(TraceState::Cancelling),
                    S::PublishingTerminal => Some(TraceState::Settling),
                    _ => None,
                }),
            TraceKind::Workflow => snapshot
                .workflows
                .runs
                .iter()
                .find(|current| {
                    serde_json::to_string(&current.id).ok().as_ref() == entry.native_id.as_ref()
                })
                .and_then(|current| match current.state {
                    W::Pending => Some(TraceState::Pending),
                    W::Running => Some(TraceState::Running),
                    W::Waiting { .. } => Some(TraceState::Waiting),
                    W::Draining => Some(TraceState::Settling),
                    W::Settled { .. } => None,
                }),
            TraceKind::Interaction => snapshot
                .pending_interactions
                .iter()
                .any(|current| {
                    current.request.conversation_id == snapshot.conversation_id
                        && entry.native_id.as_deref() == Some(current.request.id.as_str())
                })
                .then_some(TraceState::Waiting),
            _ => None,
        };
        if let Some(state) = live {
            entry.state = state;
            continue;
        }
        let Some(attempt) = &snapshot.attempt else {
            continue;
        };
        if !matches!(attempt.phase, RuntimeClientAttemptPhase::Running)
            || entry.location.attempt_id.as_ref() != Some(&attempt.attempt_id)
        {
            continue;
        }
        match entry.kind {
            TraceKind::Attempt => entry.state = TraceState::Running,
            TraceKind::Request => {
                if let (Some(request), Some(in_flight)) = (&entry.request, &attempt.in_flight)
                    && request.assistant_message_id == in_flight.message_id
                {
                    entry.state = TraceState::Running;
                }
            }
            TraceKind::Tool => {
                if let Some(tool) = &entry.tool
                    && attempt.foreground.iter().any(|current| {
                        current.call_id == tool.call_id
                            && current.tool_id == tool.tool_id
                            && matches!(current.state, ForegroundToolState::Running { .. })
                    })
                {
                    entry.state = TraceState::Running;
                }
            }
            _ => {}
        }
    }
}

fn bound_entry(entry: &mut TraceEntry) {
    // Do not truncate a native identity into another identity. Omit an
    // oversized identity explicitly and retain the stable Trace record ID.
    if entry
        .location
        .attempt_id
        .as_ref()
        .is_some_and(|id| id.as_str().len() > 512)
    {
        entry.location.attempt_id = None;
        entry.truncated = true;
    }
    if entry
        .location
        .step_id
        .as_ref()
        .is_some_and(|id| id.as_str().len() > 512)
    {
        entry.location.step_id = None;
        entry.truncated = true;
    }
    if entry.native_id.as_ref().is_some_and(|id| id.len() > 512) {
        entry.native_id = None;
        entry.truncated = true;
    }
    if entry
        .message_id
        .as_ref()
        .is_some_and(|id| id.as_str().len() > 512)
    {
        entry.message_id = None;
        entry.truncated = true;
    }
    if entry.request.as_ref().is_some_and(|request| {
        request.request_id.as_str().len() > 512 || request.assistant_message_id.as_str().len() > 512
    }) {
        entry.request = None;
        entry.truncated = true;
    }
    if entry
        .tool
        .as_ref()
        .is_some_and(|tool| tool.call_id.as_str().len() > 512 || tool.tool_id.as_str().len() > 512)
    {
        entry.tool = None;
        entry.truncated = true;
    }
    let identities = entry.artifacts.len() + entry.calls.len();
    entry
        .artifacts
        .retain(|reference| reference.artifact_id.as_str().len() <= 512);
    entry
        .calls
        .retain(|call| call.call_id.as_str().len() <= 512 && call.tool_id.as_str().len() <= 512);
    entry.truncated |= identities != entry.artifacts.len() + entry.calls.len();
    if serde_json::to_vec(&entry).is_ok_and(|bytes| bytes.len() > 32 * 1024) {
        entry.output = vec![TraceText::withheld()];
        entry.reasoning.clear();
        entry.calls.clear();
        entry.truncated = true;
    }
    // JSON escaping can expand even bounded identity/model strings sixfold.
    if serde_json::to_vec(&entry).is_ok_and(|bytes| bytes.len() > 32 * 1024) {
        entry.request = None;
        entry.tool = None;
        entry.artifacts.clear();
    }
}

#[cfg(test)]
mod tests;
