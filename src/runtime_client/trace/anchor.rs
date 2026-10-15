//! Finite own lifecycle evidence from original native J and captured C headers.
use super::TraceProjection;
use super::bounds::TracePreview;
use super::types::{
    TraceArtifact, TraceCursor, TraceFieldSource, TraceGeneration, TraceKind, TraceLocation,
    TraceState, TraceTiming, TraceToolCall, TraceToolSummary,
};
use crate::durable::ConversationStoreError;
use crate::durable::observation::{
    TraceFactData as D, TraceFactHeader, TraceFactKind as K, TraceTerminalClass,
};

pub(super) struct AnchorFacts {
    pub id: String,
    pub position: TraceCursor,
    pub location: TraceLocation,
    pub kind: TraceKind,
    pub state: TraceState,
    pub timing: TraceTiming,
    pub preview: Option<TracePreview>,
    pub calls: Vec<TraceToolCall>,
    pub native_id: Option<String>,
    pub agent_id: Option<crate::runtime::identity::AgentId>,
    pub activation_id: Option<crate::runtime::identity::SubagentId>,
    pub activation_origin: Option<crate::runtime::subagent::AgentActivationOrigin>,
    pub originating_tool_call_id: Option<crate::runtime::identity::ToolCallId>,
    pub message_id: Option<crate::runtime::identity::MessageId>,
    pub attachments: Vec<TraceArtifact>,
    pub tool: Option<TraceToolSummary>,
    pub request: Option<AnchorRequest>,
    pub has_detail: bool,
    pub truncated: bool,
    pub fields: Vec<TraceFieldSource>,
    pub collections: Vec<crate::runtime_client::checkpoint::CollectionFrontier>,
}
pub(super) struct AnchorRequest {
    pub start_sequence: u64,
    pub retry_number: Option<u32>,
    pub request_id: Option<crate::runtime::identity::RequestId>,
    pub assistant_message_id: Option<crate::runtime::identity::MessageId>,
    pub failure_kind: Option<crate::model::error::ModelErrorKind>,
    pub usage: Option<crate::model::types::ModelUsage>,
    pub generation: Option<TraceGeneration>,
}
impl AnchorFacts {
    fn from_header(header: &TraceFactHeader) -> Self {
        Self {
            id: format!("trace:{}", header.sequence),
            position: TraceCursor::at(header.sequence),
            location: header.location.clone(),
            kind: TraceKind::Step,
            state: TraceState::Incomplete,
            timing: TraceTiming {
                started_at: header.timestamp,
                ended_at: None,
                duration_ms: None,
            },
            preview: None,
            calls: Vec::new(),
            native_id: None,
            agent_id: None,
            activation_id: None,
            activation_origin: None,
            originating_tool_call_id: None,
            message_id: None,
            attachments: Vec::new(),
            tool: None,
            request: None,
            has_detail: false,
            truncated: false,
            fields: header.fields.clone(),
            collections: Vec::new(),
        }
    }
    pub(super) fn canonical(
        &mut self,
        summary: crate::durable::observation::TraceCanonicalSummary,
    ) {
        self.preview = summary.preview;
        self.message_id = summary.identity;
        self.calls = summary.calls;
        self.attachments = summary.attachments;
        self.fields.extend(summary.fields);
        self.collections.extend(summary.collections);
        self.truncated |= summary.truncated;
    }
    fn terminal(&mut self, ending: Option<TraceFactHeader>, anchor: u64) {
        let Some(end) = ending.filter(|end| end.sequence > anchor) else {
            return;
        };
        let D::Terminal(terminal) = end.fact else {
            return;
        };
        self.state = terminal_state(terminal.class);
        self.timing.ended_at = Some(end.timestamp);
        self.timing.duration_ms =
            u64::try_from((end.timestamp - self.timing.started_at).num_milliseconds()).ok();
        self.fields.extend(end.fields);
        if let Some(request) = &mut self.request {
            request.failure_kind = terminal.failure_kind;
            request.generation = terminal.generation.map(|evidence| {
                super::record::generation_metrics(evidence, terminal.usage.as_ref())
            });
            request.usage = terminal.usage;
        }
        if let Some(tool) = &mut self.tool {
            // Actual J outcome remains positive before its canonical result exists.
            if tool.outcome.is_none() {
                tool.outcome = terminal.tool_outcome;
            }
            if tool.detail.is_none() {
                tool.detail = terminal.detail;
            }
        }
    }
}
pub(super) fn terminal_state(class: TraceTerminalClass) -> TraceState {
    match class {
        TraceTerminalClass::Completed => TraceState::Completed,
        TraceTerminalClass::Failed => TraceState::Failed,
        TraceTerminalClass::Cancelled => TraceState::Cancelled,
        TraceTerminalClass::TimedOut => TraceState::TimedOut,
        TraceTerminalClass::Limited => TraceState::Limited,
        TraceTerminalClass::Denied => TraceState::Denied,
        TraceTerminalClass::OutcomeUnknown => TraceState::OutcomeUnknown,
        TraceTerminalClass::Interrupted => TraceState::Interrupted,
    }
}
impl TraceProjection<'_> {
    pub(super) fn anchor_facts(
        &self,
        anchor: &TraceFactHeader,
    ) -> Result<AnchorFacts, ConversationStoreError> {
        let mut facts = AnchorFacts::from_header(anchor);
        let endings = self.anchor_start(&mut facts, anchor)?;
        let ending = if endings.is_empty() {
            None
        } else {
            self.ending(anchor.sequence, &endings)?
        };
        if let Some(end) = &ending
            && let D::Terminal(terminal) = &end.fact
            && terminal.summary_message_id.is_some()
        {
            facts.message_id.clone_from(&terminal.summary_message_id);
        }
        facts.terminal(ending, anchor.sequence);
        Ok(facts)
    }
    fn anchor_start(
        &self,
        facts: &mut AnchorFacts,
        anchor: &TraceFactHeader,
    ) -> Result<Vec<K>, ConversationStoreError> {
        let endings = match &anchor.fact {
            D::AttemptStart => {
                facts.kind = TraceKind::Attempt;
                vec![
                    K::AttemptCompleted,
                    K::AttemptFailed,
                    K::AttemptCancelled,
                    K::AttemptTimedOut,
                    K::AttemptLimitExceeded,
                ]
            }
            D::StepStart => vec![K::TurnCompleted],
            D::Adoption {
                total,
                classification,
                unanimous_agent,
                ..
            } => {
                facts.kind = match classification {
                    crate::durable::observation::TraceAdoptionClass::Human => TraceKind::User,
                    crate::durable::observation::TraceAdoptionClass::Context => TraceKind::Context,
                };
                facts.state = TraceState::Completed;
                facts.has_detail = *total > 0;
                facts.truncated = *total > 8;
                facts.agent_id.clone_from(unanimous_agent);
                if facts.location.attempt_id.is_none()
                    && facts.location.attempt_id_source.is_none()
                    && let Some(next) = self.next_attempt(anchor.sequence)?
                {
                    facts.location = next.location;
                }
                Vec::new()
            }
            request @ D::RequestStart { .. } => request_start(facts, request),
            D::AssistantCommit { ledger_position } => {
                facts.kind = TraceKind::Assistant;
                facts.state = TraceState::Completed;
                facts.has_detail = true;
                facts.canonical(
                    self.store
                        .observation_trace_canonical_summary(self.cut()?, *ledger_position)?,
                );
                Vec::new()
            }
            D::ToolStart {
                occurrence,
                call_id,
                tool_id,
            } => {
                self.tool_start(facts, occurrence.as_ref(), call_id.clone(), tool_id.clone())?;
                vec![K::ToolExecutionCompleted, K::ToolExecutionFailed]
            }
            D::CompactionStart => {
                facts.kind = TraceKind::Compaction;
                facts.has_detail = true;
                vec![K::CompactionCompleted, K::CompactionFailed]
            }
            D::BackgroundStart {
                native_id,
                originating_tool_call_id,
            } => {
                facts.kind = TraceKind::Background;
                facts.native_id.clone_from(native_id);
                facts
                    .originating_tool_call_id
                    .clone_from(originating_tool_call_id);
                vec![K::BackgroundTerminalPublished]
            }
            subagent @ D::SubagentStart { .. } => subagent_start(facts, subagent),
            D::WorkflowStart {
                native_id,
                originating_tool_call_id,
            } => {
                facts.kind = TraceKind::Workflow;
                facts.native_id.clone_from(native_id);
                facts
                    .originating_tool_call_id
                    .clone_from(originating_tool_call_id);
                vec![
                    K::WorkflowCompleted,
                    K::WorkflowFailed,
                    K::WorkflowCancelled,
                ]
            }
            D::InteractionStart { native_id } => {
                facts.kind = TraceKind::Interaction;
                facts.native_id.clone_from(native_id);
                vec![K::InteractionSettled]
            }
            D::CanonicalToolCommit { .. } | D::Terminal(_) => {
                return Err(ConversationStoreError::InvalidReference(
                    "native fact is not a Trace anchor".into(),
                ));
            }
        };
        Ok(endings)
    }
    fn tool_start(
        &self,
        facts: &mut AnchorFacts,
        occurrence: Option<&crate::events::types::ToolObservationOccurrence>,
        call_id: Option<crate::runtime::identity::ToolCallId>,
        tool_id: Option<crate::runtime::identity::ToolId>,
    ) -> Result<(), ConversationStoreError> {
        facts.kind = TraceKind::Tool;
        facts.has_detail = true;
        let mut tool = TraceToolSummary {
            call_id,
            tool_id,
            occurrence: occurrence.map(public_occurrence),
            fields: Vec::new(),
            name: None,
            arguments: None,
            started: true,
            outcome: None,
            detail: None,
        };
        if let Some(occurrence) = occurrence
            && let Some(summary) = self
                .store
                .observation_trace_tool_result(self.cut()?, occurrence)?
        {
            tool.outcome = summary.tool_outcome;
            tool.detail.clone_from(&summary.tool_detail);
            facts.canonical(summary);
        }
        facts.tool = Some(tool);
        Ok(())
    }
}
fn request_start(facts: &mut AnchorFacts, data: &D) -> Vec<K> {
    let D::RequestStart {
        start_sequence,
        retry_number,
        request_id,
        assistant_message_id,
    } = data
    else {
        unreachable!("native Request start classification")
    };
    facts.kind = TraceKind::Request;
    facts.has_detail = true;
    facts.request = Some(AnchorRequest {
        start_sequence: *start_sequence,
        retry_number: *retry_number,
        request_id: request_id.clone(),
        assistant_message_id: assistant_message_id.clone(),
        failure_kind: None,
        usage: None,
        generation: None,
    });
    vec![K::ModelRequestCompleted, K::ModelRequestFailed]
}
fn subagent_start(facts: &mut AnchorFacts, data: &D) -> Vec<K> {
    let D::SubagentStart {
        activation_id,
        agent_id,
        origin,
        agent_preview,
    } = data
    else {
        unreachable!("native Subagent start classification")
    };
    facts.kind = TraceKind::Subagent;
    facts.native_id = activation_id.as_ref().map(ToString::to_string);
    facts.agent_id = Some(agent_id.clone());
    facts.activation_id.clone_from(activation_id);
    facts.activation_origin.clone_from(origin);
    facts.originating_tool_call_id = origin
        .as_ref()
        .and_then(|origin| origin.tool_call_id().cloned());
    facts.preview = Some(agent_preview.clone());
    vec![K::SubagentTerminalPublished, K::SubagentTerminalSettled]
}

pub(super) fn public_occurrence(
    occurrence: &crate::events::types::ToolObservationOccurrence,
) -> crate::runtime_client::snapshot::RuntimeClientToolOccurrence {
    crate::runtime_client::snapshot::RuntimeClientToolOccurrence {
        locator: crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition {
            position: occurrence.ledger_position,
        },
        block_index: occurrence.block_index,
    }
}
