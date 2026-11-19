//! Finite read evidence captured at original native J/C/request boundaries.
//! These headers are neither event authority nor execution input.
use crate::observation::trace::{
    TraceArtifact, TraceFieldSource, TraceLocation, TracePreview, TraceToolCall, TraceToolOutcome,
};
use rustx_types::events::types::RuntimeEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceFactKind {
    AttemptStarted,
    AttemptCompleted,
    AttemptFailed,
    AttemptCancelled,
    AttemptTimedOut,
    AttemptLimitExceeded,
    TurnStarted,
    TurnCompleted,
    InboundTurnAdopted,
    ModelRequestStarted,
    ModelRequestCompleted,
    ModelRequestFailed,
    AssistantMessageCommitted,
    ToolExecutionStarted,
    ToolExecutionCompleted,
    ToolExecutionFailed,
    ToolMessageCommitted,
    CompactionStarted,
    CompactionCompleted,
    CompactionFailed,
    BackgroundExecutionCommitted,
    BackgroundTerminalPublished,
    SubagentOwnershipCommitted,
    SubagentTerminalPublished,
    SubagentTerminalSettled,
    WorkflowStarted,
    WorkflowCompleted,
    WorkflowFailed,
    WorkflowCancelled,
    InteractionRequested,
    InteractionSettled,
    ConfigurationApplied,
    ToolCallRequested,
}
#[derive(Debug, Clone)]
pub enum TraceFactScope {
    All,
    /// Primary Agent-owned model-request facts only.
    Agent,
    Owner {
        anchor_sequence: u64,
    },
}
#[derive(Debug, Clone)]
pub struct TraceFactQuery {
    pub scope: TraceFactScope,
    pub kinds: Vec<TraceFactKind>,
    pub before: Option<u64>,
    pub after: u64,
    pub ascending: bool,
    pub limit: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceFactHeader {
    pub sequence: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub kind: TraceFactKind,
    /// Explicit request owner when this fact is itself a model-request fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<super::observation::RequestOwnerKind>,
    pub owner_anchor: Option<u64>,
    pub location: TraceLocation,
    pub fields: Vec<TraceFieldSource>,
    pub fact: TraceFactData,
    pub bootstrap: Option<TraceBootstrapFact>,
    /// Same-C presentation recapture after original raw retention. Own J facts remain independent.
    pub canonical_fields_ready: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TraceBootstrapFact {
    Turn {
        number: u32,
    },
    AttemptCancelled {
        reason: rustx_types::runtime::types::CancellationReason,
    },
    AttemptLimited {
        limit: rustx_types::events::types::AttemptLimit,
    },
    CompactionFailed {
        detail: TracePreview,
    },
    CompactionCompleted {
        reading: Box<crate::observation::OccupancyObservation>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TraceFactData {
    AttemptStart,
    StepStart,
    Adoption {
        total: u64,
        first_ledger_positions: Vec<u64>,
        classification: TraceAdoptionClass,
        unanimous_agent: Option<rustx_types::runtime::identity::AgentId>,
    },
    RequestStart {
        start_sequence: u64,
        retry_number: Option<u32>,
        request_id: Option<rustx_types::runtime::identity::RequestId>,
        assistant_message_id: Option<rustx_types::runtime::identity::MessageId>,
    },
    AssistantCommit {
        ledger_position: u64,
    },
    ToolStart {
        occurrence: Option<rustx_types::events::types::ToolObservationOccurrence>,
        call_id: Option<rustx_types::runtime::identity::ToolCallId>,
        tool_id: Option<rustx_types::runtime::identity::ToolId>,
    },
    CompactionStart {
        native_id: Option<String>,
    },
    BackgroundStart {
        native_id: Option<String>,
        originating_tool_call_id: Option<rustx_types::runtime::identity::ToolCallId>,
    },
    SubagentStart {
        activation_id: Option<rustx_types::runtime::identity::SubagentId>,
        agent_id: rustx_types::runtime::identity::AgentId,
        origin: Option<rustx_types::runtime::subagent::AgentActivationOrigin>,
        agent_preview: TracePreview,
    },
    WorkflowStart {
        native_id: Option<String>,
        originating_tool_call_id: Option<rustx_types::runtime::identity::ToolCallId>,
    },
    InteractionStart {
        native_id: Option<String>,
    },
    /// A Session configuration commit; complete and instantaneous.
    Configuration {
        change: rustx_types::runtime::configuration_commit::ConfigurationChange,
    },
    CanonicalToolCommit {
        occurrence: Option<rustx_types::events::types::ToolObservationOccurrence>,
        ledger_position: u64,
    },
    Terminal(TraceTerminalHeader),
    ToolRequest {
        occurrence: Option<rustx_types::events::types::ToolObservationOccurrence>,
        call_id: Option<rustx_types::runtime::identity::ToolCallId>,
        tool_id: Option<rustx_types::runtime::identity::ToolId>,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TraceAdoptionClass {
    Human,
    Context,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TraceTerminalClass {
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Limited,
    Denied,
    OutcomeUnknown,
    Interrupted,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceTerminalHeader {
    pub class: TraceTerminalClass,
    pub tool_outcome: Option<TraceToolOutcome>,
    pub detail: Option<TracePreview>,
    pub failure_kind: Option<rustx_types::model::error::ModelErrorKind>,
    pub usage: Option<rustx_types::model::types::ModelUsage>,
    pub generation: Option<rustx_types::model::generation_evidence::GenerationEvidence>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestReadSummary {
    pub request_id: Option<rustx_types::runtime::identity::RequestId>,
    pub attempt_id: Option<rustx_types::runtime::identity::AttemptId>,
    pub step_id: Option<rustx_types::runtime::identity::TurnId>,
    pub assistant_message_id: Option<rustx_types::runtime::identity::MessageId>,
    pub retry_number: u32,
    pub model: Option<String>,
    pub model_preview: TracePreview,
    pub context_window_tokens: u64,
    pub prompt_preview: TracePreview,
    pub prompt_digest: [u8; 32],
    pub tools_digest: [u8; 32],
    pub prompt_json_bytes: u64,
    pub tools_json_bytes: u64,
}
#[derive(Debug, Clone)]
pub struct TraceRequestPredecessorHeader {
    pub start_sequence: u64,
    pub request_id: Option<rustx_types::runtime::identity::RequestId>,
    pub source: Option<TraceFieldSource>,
    /// None only if the actual original snapshot is absent, never for a layout hole.
    pub summary: Option<RequestReadSummary>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceCanonicalSummary {
    pub source: Option<rustx_types::message::types::UserSource>,
    pub ordinary_input: bool,
    pub context_kind: Option<crate::observation::trace::TraceContextKind>,
    pub locator: rustx_types::observation::content::CanonicalContentLocator,
    pub identity: Option<rustx_types::runtime::identity::MessageId>,
    pub preview: Option<TracePreview>,
    pub calls: Vec<TraceToolCall>,
    pub attachments: Vec<TraceArtifact>,
    pub tool_outcome: Option<TraceToolOutcome>,
    pub tool_detail: Option<TracePreview>,
    pub truncated: bool,
    pub fields: Vec<TraceFieldSource>,
    pub collections: Vec<crate::observation::cut::CollectionFrontier>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceToolProposalSummary {
    pub occurrence: rustx_types::events::types::ToolObservationOccurrence,
    pub call_id: Option<rustx_types::runtime::identity::ToolCallId>,
    pub tool_id: Option<rustx_types::runtime::identity::ToolId>,
    pub name: Option<String>,
    pub arguments: Option<TracePreview>,
    pub fields: Vec<TraceFieldSource>,
}
#[derive(Debug, Clone)]
pub struct TraceRequestContextPage {
    pub total: u64,
    pub items: Vec<crate::observation::trace::TraceContextPresentation>,
    pub next_offset: Option<u64>,
    pub domain: String,
}

/// Original adoption ordinal order, joined only to bounded same-cut canonical heads.
#[derive(Debug, Clone)]
pub struct TraceAdoptionPage {
    pub total: u64,
    pub items: Vec<TraceCanonicalSummary>,
    pub next_offset: Option<u64>,
}

pub fn trace_fact_requires_publication(event: &rustx_types::events::types::RuntimeEvent) -> bool {
    match event {
        RuntimeEvent::AttemptStarted { .. }
        | RuntimeEvent::AttemptCompleted { .. }
        | RuntimeEvent::AttemptCancelled { .. }
        | RuntimeEvent::AttemptTimedOut { .. }
        | RuntimeEvent::AttemptLimitExceeded { .. }
        | RuntimeEvent::AttemptFailed { .. }
        | RuntimeEvent::TurnStarted
        | RuntimeEvent::TurnCompleted
        | RuntimeEvent::ContextContributionEmitted { .. }
        | RuntimeEvent::ModelRetryScheduled { .. }
        // A Session configuration commit is its own between-Attempt record.
        | RuntimeEvent::ConfigurationApplied { .. }
        // Adoption is the canonical linearization point at which inbound
        // became a turn this conversation owes an answer for. Trace projects
        // it as the ledger's User record, so its receipt must be published
        // before a later owner may advance the represented prefix past it.
        | RuntimeEvent::InboundTurnAdopted { .. }
        | RuntimeEvent::AssistantMessageCommitted { .. }
        | RuntimeEvent::ToolCallRequested { .. }
        | RuntimeEvent::ToolExecutionStarted { .. }
        | RuntimeEvent::ToolExecutionCompleted { .. }
        | RuntimeEvent::ToolExecutionFailed { .. }
        | RuntimeEvent::ToolMessageCommitted { .. }
        | RuntimeEvent::CompactionStarted { .. }
        | RuntimeEvent::CompactionCompleted { .. }
        | RuntimeEvent::CompactionFailed { .. }
        | RuntimeEvent::BackgroundExecutionCommitted { .. }
        | RuntimeEvent::BackgroundTerminalPublished { .. }
        | RuntimeEvent::AgentActivationAdmission { .. }
        | RuntimeEvent::SubagentOwnershipCommitted { .. }
        | RuntimeEvent::SubagentTerminalPublished { .. }
        | RuntimeEvent::SubagentTerminalSettled { .. }
        | RuntimeEvent::SubagentPhysicalSettlementProven { .. }
        | RuntimeEvent::WorkflowStarted { .. }
        | RuntimeEvent::WorkflowCompleted { .. }
        | RuntimeEvent::WorkflowFailed { .. }
        | RuntimeEvent::WorkflowCancelled { .. }
        | RuntimeEvent::InteractionRequested { .. }
        | RuntimeEvent::InteractionSettled { .. }
        | RuntimeEvent::Goal { .. }
        | RuntimeEvent::ModelRequestStarted {
            owner: crate::model::snapshot::ModelRequestOwner::Agent { .. },
            ..
        }
        | RuntimeEvent::ModelRequestCompleted {
            owner: crate::model::snapshot::ModelRequestOwner::Agent { .. },
            ..
        }
        | RuntimeEvent::ModelRequestFailed {
            owner: crate::model::snapshot::ModelRequestOwner::Agent { .. },
            ..
        } => true,
        RuntimeEvent::ModelRequestStarted { .. }
        | RuntimeEvent::ModelRequestCompleted { .. }
        | RuntimeEvent::ModelRequestFailed { .. }
        | RuntimeEvent::AuxiliaryCallStarted { .. }
        | RuntimeEvent::AuxiliaryCallSettled { .. }
        | RuntimeEvent::AuxiliaryRetryScheduled { .. }
        | RuntimeEvent::NativeToolInvocation { .. }
        | RuntimeEvent::ToolExecutionProgress { .. }
        | RuntimeEvent::ToolExecutionDeadlineFired { .. }
        | RuntimeEvent::ToolExecutionCancellationRequested { .. }
        | RuntimeEvent::ToolExecutionSettlementObserved { .. }
        | RuntimeEvent::ToolExecutionSettlementControlFailed { .. }
        | RuntimeEvent::WorkflowBlockStarted { .. }
        | RuntimeEvent::WorkflowLoopIterationAdmitted { .. }
        | RuntimeEvent::WorkflowLoopIterationSettled { .. }
        | RuntimeEvent::WorkflowLoopExited { .. }
        | RuntimeEvent::WorkflowBlockSettled { .. }
        | RuntimeEvent::WorkflowNodeStarted { .. }
        | RuntimeEvent::WorkflowNodeInputRecorded { .. }
        | RuntimeEvent::WorkflowNodeOutputRecorded { .. }
        | RuntimeEvent::WorkflowNodeWaiting { .. }
        | RuntimeEvent::WorkflowNodeSettled { .. }
        | RuntimeEvent::WorkflowAgentAdmitted { .. }
        | RuntimeEvent::WorkflowAgentOutputCommitted { .. }
        | RuntimeEvent::WorkflowBranchSelected { .. }
        => false,
    }
}
