//! Finite read evidence captured at original native J/C/request boundaries.
//! These headers are neither event authority nor execution input.
use crate::runtime_client::trace::{
    TraceArtifact, TraceFieldSource, TraceLocation, TracePreview, TraceToolCall, TraceToolOutcome,
};
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
}
#[derive(Debug, Clone)]
pub enum TraceFactScope {
    All,
    Owner { anchor_sequence: u64 },
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
        reason: crate::runtime::types::CancellationReason,
    },
    AttemptLimited {
        limit: crate::events::types::AttemptLimit,
    },
    CompactionFailed {
        detail: TracePreview,
    },
    CompactionCompleted {
        reading: Box<crate::durable::observation::OccupancyObservation>,
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
        unanimous_agent: Option<crate::runtime::identity::AgentId>,
    },
    RequestStart {
        start_sequence: u64,
        retry_number: Option<u32>,
        request_id: Option<crate::runtime::identity::RequestId>,
        assistant_message_id: Option<crate::runtime::identity::MessageId>,
    },
    AssistantCommit {
        ledger_position: u64,
    },
    ToolStart {
        occurrence: Option<crate::events::types::ToolObservationOccurrence>,
        call_id: Option<crate::runtime::identity::ToolCallId>,
        tool_id: Option<crate::runtime::identity::ToolId>,
    },
    CompactionStart,
    BackgroundStart {
        native_id: Option<String>,
        originating_tool_call_id: Option<crate::runtime::identity::ToolCallId>,
    },
    SubagentStart {
        activation_id: Option<crate::runtime::identity::SubagentId>,
        agent_id: crate::runtime::identity::AgentId,
        origin: Option<crate::runtime::subagent::AgentActivationOrigin>,
        agent_preview: TracePreview,
    },
    WorkflowStart {
        native_id: Option<String>,
        originating_tool_call_id: Option<crate::runtime::identity::ToolCallId>,
    },
    InteractionStart {
        native_id: Option<String>,
    },
    CanonicalToolCommit {
        occurrence: Option<crate::events::types::ToolObservationOccurrence>,
        ledger_position: u64,
    },
    Terminal(TraceTerminalHeader),
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
    pub failure_kind: Option<crate::model::error::ModelErrorKind>,
    pub usage: Option<crate::model::types::ModelUsage>,
    pub generation: Option<crate::model::generation_evidence::GenerationEvidence>,
    pub summary_ledger_position: Option<u64>,
    pub summary_message_id: Option<crate::runtime::identity::MessageId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestReadSummary {
    pub request_id: Option<crate::runtime::identity::RequestId>,
    pub attempt_id: Option<crate::runtime::identity::AttemptId>,
    pub step_id: Option<crate::runtime::identity::TurnId>,
    pub assistant_message_id: Option<crate::runtime::identity::MessageId>,
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
    pub request_id: Option<crate::runtime::identity::RequestId>,
    pub source: Option<TraceFieldSource>,
    /// None only if the actual original snapshot is absent, never for a layout hole.
    pub summary: Option<RequestReadSummary>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceCanonicalSummary {
    pub source: Option<crate::message::types::UserSource>,
    pub ordinary_input: bool,
    pub context_kind: Option<crate::runtime_client::trace::TraceContextKind>,
    pub locator: crate::runtime_client::content::CanonicalContentLocator,
    pub identity: Option<crate::runtime::identity::MessageId>,
    pub preview: Option<TracePreview>,
    pub calls: Vec<TraceToolCall>,
    pub attachments: Vec<TraceArtifact>,
    pub tool_outcome: Option<TraceToolOutcome>,
    pub tool_detail: Option<TracePreview>,
    pub truncated: bool,
    pub fields: Vec<TraceFieldSource>,
    pub collections: Vec<crate::runtime_client::checkpoint::CollectionFrontier>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceToolProposalSummary {
    pub occurrence: crate::events::types::ToolObservationOccurrence,
    pub call_id: Option<crate::runtime::identity::ToolCallId>,
    pub tool_id: Option<crate::runtime::identity::ToolId>,
    pub name: Option<String>,
    pub arguments: Option<TracePreview>,
    pub fields: Vec<TraceFieldSource>,
}
#[derive(Debug, Clone)]
pub struct TraceRequestContextPage {
    pub total: u64,
    pub items: Vec<crate::runtime_client::trace::TraceContextPresentation>,
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
