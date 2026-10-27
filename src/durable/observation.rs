//! Finite display descriptors indexed by their native immutable source commit.
//! These indexes are derived read layouts, never canonical/model-context authority.
use crate::conversation::SurfaceRevision;
use crate::message::types::MessageBlock;
use crate::runtime::identity::MessageId;
use crate::runtime_client::checkpoint::{CollectionFrontier, ContentBinding};
use crate::runtime_client::snapshot::RuntimeClientTranscriptEntry;
use serde::{Deserialize, Serialize};

pub use super::trace::{
    RequestReadSummary, TraceAdoptionClass, TraceAdoptionPage, TraceBootstrapFact,
    TraceCanonicalSummary, TraceFactData, TraceFactHeader, TraceFactKind, TraceFactQuery,
    TraceFactScope, TraceRequestContextPage, TraceRequestPredecessorHeader, TraceTerminalClass,
    TraceTerminalHeader, TraceToolProposalSummary,
};

pub const HEADER_PAGE_MAX: usize = 32;
pub const HEADER_ENCODED_MAX: usize = 16_384;

/// The bounded transcript row whose response facts are needed at one native cut.
#[derive(Debug, Clone)]
pub struct ResponsePageSelection {
    pub cursor: u64,
    pub kind: ResponsePageSelectionKind,
}

#[derive(Debug, Clone)]
pub enum ResponsePageSelectionKind {
    Process,
    Message(MessageId),
    /// The exact descriptor field; malformed or absent locators do not select a response.
    Deferred(Option<serde_json::Value>),
}

/// Finite response facts assembled in the same read transaction as their native owners.
#[derive(Debug, Clone)]
pub struct ResponsePageItem {
    pub process: Option<crate::runtime_client::response::TurnProcessView>,
    pub response: Option<crate::runtime_client::response::CompletedResponseView>,
    pub failure_deferred: bool,
    pub models: Option<(u64, u64, Option<u64>)>,
    pub identity_fields: Vec<ResponseIdentityField>,
}

#[derive(Debug, Clone)]
pub struct ResponsePage {
    /// Same length and order as the selections, including rows with no owner.
    pub items: Vec<ResponsePageItem>,
    pub statistics: crate::runtime_client::response::ConversationStatistics,
}

/// Bounded live checkpoint domains observed together at one native read cut.
/// Original source values and explicit trace pages remain independently readable.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckpointHead {
    pub transcript: crate::runtime_client::snapshot::RuntimeClientTranscriptPage,
    /// Complete body plus response decoration, owned by native read evidence.
    /// Internal only: page serialization and public cursor contracts are unchanged.
    pub(crate) transcript_rows: Vec<crate::runtime_client::checkpoint::PreparedTranscriptRow>,
    pub occupancy: Option<crate::runtime_client::occupancy::RuntimeClientContextOccupancy>,
    pub trace_total: u64,
    pub projection_head: Option<crate::runtime_client::snapshot::RuntimeClientProjectionHeadView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ObservationReadCut {
    pub read: super::reading::ConversationReadCut,
    pub ledger_through: u64,
    pub transcript_total: u64,
    pub surface_revision: SurfaceRevision,
    pub inherited_through: super::TranscriptCursor,
}

/// Source-owned publication proof; this never advances a client cursor by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSourceReady {
    pub prefix: u64,
    pub read_cut: ObservationReadCut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MessageHeader {
    pub deliveries: Option<crate::runtime_client::delivery::RuntimeClientDeliveryHead>,
    pub tool_calls: Vec<crate::runtime_client::snapshot::ForegroundToolExecution>,
    pub transcript_cursor: Option<super::TranscriptCursor>,
    /// Exact inline identity only while its complete UTF-8 representation fits 256 bytes.
    pub identity: Option<MessageId>,
    pub locator: Option<crate::runtime_client::content::CanonicalContentLocator>,
    /// None while durably accepted inbound has not entered the canonical Ledger.
    pub ledger_position: Option<u64>,
    pub role: String,
    pub source_label: Option<String>,
    /// Native eligible opening input; derived at canonical commit, never from preview text.
    pub opening_prompt: bool,
    pub human_activity_at: Option<chrono::DateTime<chrono::Utc>>,
    pub opening_preview: Option<String>,
    /// A text/refusal block contains a non-whitespace character.
    pub has_text_preview: bool,
    /// Exact semantic family, including deferred Compaction/Goal/Context metadata.
    pub kind: String,
    pub block_count: u64,
    pub tool_call_count: u64,
    pub display: Option<MessageBlock>,
    pub content: Vec<ContentBinding>,
    pub collections: Vec<CollectionFrontier>,
    pub deferred: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageHeaderPage {
    pub cut: ObservationReadCut,
    pub total: u64,
    /// Zero-based canonical commit-order offset, independent of Surface membership.
    pub offset: u64,
    pub headers: Vec<MessageHeader>,
    pub next_offset: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptHeader {
    pub tool_calls: Vec<crate::runtime_client::snapshot::ForegroundToolExecution>,
    #[serde(default)]
    pub publication_settlement:
        Option<crate::runtime_client::snapshot::RuntimeClientPublicationSettlementRecord>,
    pub cursor: super::TranscriptCursor,
    pub reference_kind: String,
    pub reference_id: String,
    pub message: Option<MessageHeader>,
    pub display: Option<RuntimeClientTranscriptEntry>,
    pub content: Vec<ContentBinding>,
    pub collections: Vec<CollectionFrontier>,
    pub deferred: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptHeaderPage {
    pub cut: ObservationReadCut,
    pub total: u64,
    pub headers: Vec<TranscriptHeader>,
    pub next_cursor: Option<super::TranscriptCursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MessageBlockHeader {
    pub block_index: crate::message::types::ContentBlockIndex,
    pub kind: String,
    pub value: Option<serde_json::Value>,
    pub content: Vec<ContentBinding>,
    pub deferred: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageBlockHeaderPage {
    pub cut: ObservationReadCut,
    pub message_id: Option<MessageId>,
    pub locator: crate::runtime_client::content::CanonicalContentLocator,
    pub total: u64,
    pub offset: u64,
    pub headers: Vec<MessageBlockHeader>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservationEventField {
    pub path: Vec<String>,
    pub encoded_bytes: u64,
    pub children: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservationEventWitness {
    /// Exact native Compaction reading, including explicit absence; independent of body layout.
    pub occupancy: Option<OccupancyObservation>,
    #[serde(default)]
    pub native_direct: bool,
    /// Same-J native anchor total; absence explicitly denotes unavailable derived evidence.
    pub trace_total: Option<u64>,
    pub source_unavailable: bool,
    pub sequence: u64,
    pub transcript_cursor: Option<super::TranscriptCursor>,
    pub event_id: Option<crate::runtime::identity::EventId>,
    pub attempt_id: Option<crate::runtime::identity::AttemptId>,
    pub turn_id: Option<crate::runtime::identity::TurnId>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event_kind: String,
    pub tool_occurrence: Option<crate::events::types::ToolObservationOccurrence>,
    /// Exact canonical L for `AssistantMessageCommitted` or `ToolMessageCommitted`.
    pub result_ledger_position: Option<u64>,
    pub value: serde_json::Value,
    pub fields: Vec<ObservationEventField>,
    pub deferred: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OccupancyObservation {
    /// Finite head captured with this original Completed J, before publication.
    pub projection_head: crate::runtime_client::snapshot::RuntimeClientProjectionHeadView,
    pub compaction: crate::runtime_client::snapshot::RuntimeClientCompactionView,
    pub value: Option<crate::runtime_client::occupancy::RuntimeClientContextOccupancy>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ToolAssociationHeader {
    pub block_index: crate::message::types::ContentBlockIndex,
    pub call_id: Option<crate::runtime::identity::ToolCallId>,
    pub tool_id: Option<crate::runtime::identity::ToolId>,
    pub call: MessageBlockHeader,
    pub result: Option<MessageHeader>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolAssociationHeaderPage {
    pub cut: ObservationReadCut,
    pub message_id: Option<MessageId>,
    pub locator: crate::runtime_client::content::CanonicalContentLocator,
    pub total: u64,
    pub offset: u64,
    pub headers: Vec<ToolAssociationHeader>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolResultHeader {
    pub result: crate::tools::types::ToolExecutionResult,
    pub result_deferred: bool,
    pub content_total: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolObservationHeader {
    pub name: String,
    pub arguments_json: Option<String>,
    pub committed_result: Option<NativeToolResultHeader>,
    pub association: ToolAssociationHeader,
    pub started: Option<ObservationEventWitness>,
    pub terminal: Option<ObservationEventWitness>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeToolObservationPage {
    pub cut: ObservationReadCut,
    pub message_id: Option<MessageId>,
    pub locator: crate::runtime_client::content::CanonicalContentLocator,
    pub total: u64,
    pub offset: u64,
    pub headers: Vec<NativeToolObservationHeader>,
    pub next_offset: Option<u64>,
}

/// One immutable same-C selected delivery, scoped by an authentic native cut.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeFileDelivery {
    pub cut: ObservationReadCut,
    pub ledger_position: u64,
    pub index: u64,
    pub file: crate::tools::session_files::SessionFileReference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservationWindow {
    pub cut: ObservationReadCut,
    pub page: TranscriptHeaderPage,
    pub newer_cursor: Option<super::TranscriptCursor>,
    pub target: Option<crate::runtime_client::turn::ObservedTurnIdentity>,
    pub target_cursor: Option<super::TranscriptCursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingHeader {
    pub sequence: u64,
    pub revision: u64,
    pub message: MessageHeader,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingHeaderPage {
    pub cut: super::reading::ConversationReadCut,
    pub total: u64,
    pub headers: Vec<PendingHeader>,
    /// Exclusive native inbound sequence, independent of a shrinking array index.
    pub next_sequence: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationValueChunk {
    pub bytes: Vec<u8>,
    pub end_offset: u64,
    pub readable_end: u64,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AttemptMemberCounts {
    pub message_count: u64,
    pub tool_call_count: u64,
    pub first_cursor: Option<super::TranscriptCursor>,
    pub final_message_id: Option<MessageId>,
    pub final_message_locator: Option<crate::runtime_client::content::CanonicalContentLocator>,
    pub final_message_id_deferred: bool,
    pub final_message_id_source: Option<crate::runtime_client::content::ContentRef>,
    /// Proven by the original successful terminal or inherited response, never C alone.
    pub completed_response: bool,
    pub process: Option<MemberProcessHeader>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemberProcessHeader {
    pub conversation_id: crate::runtime::identity::ConversationId,
    pub attempt_id: Option<crate::runtime::identity::AttemptId>,
    pub attempt_id_source: Option<crate::runtime_client::response::AttemptIdentitySource>,
    pub event_id: Option<crate::runtime::identity::EventId>,
    pub outcome: super::reading::TurnProcessOutcome,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub native_start: Option<u64>,
    pub native_terminal: Option<u64>,
    pub owner_anchor: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AttemptExecutionSummary {
    pub failure: Option<crate::runtime_client::event::RuntimeClientAttemptFailure>,
    pub failure_deferred: bool,
    pub failure_readable_end: u64,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub terminal_event_id: Option<crate::runtime::identity::EventId>,
    pub terminal_sequence: Option<u64>,
    pub terminal_kind: Option<String>,
    pub finish_reason: Option<crate::model::ModelFinishReason>,
    pub finish_reason_deferred: bool,
    pub finish_reason_sequence: Option<u64>,
    pub closing_message_id: Option<MessageId>,
    pub closing_ledger_position: Option<u64>,
    pub closing_identity_sequence: Option<u64>,
    pub last_request_id: Option<crate::runtime::identity::RequestId>,
    pub last_request_sequence: Option<u64>,
    pub last_request_id_source: Option<crate::runtime_client::snapshot::JournalFieldRef>,
    pub requests: u64,
    pub reports: u64,
    pub usage: Option<crate::model::ModelUsage>,
    pub timing: Option<super::response::CompletedResponseTiming>,
    pub models: Vec<String>,
    pub models_total: u64,
    pub models_next_offset: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptModelName {
    pub ordinal: u64,
    pub request_id: Option<crate::runtime::identity::RequestId>,
    pub request_id_source: Option<crate::runtime_client::snapshot::JournalFieldRef>,
    pub source_sequence: u64,
    pub encoded_bytes: u64,
    pub value: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptModelNamePage {
    pub total: u64,
    pub offset: u64,
    pub names: Vec<AttemptModelName>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResponseIdentitySource {
    Attempt {
        sequence: u64,
    },
    Inherited {
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        ordinal: u64,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseIdentityField {
    pub path: Vec<String>,
    pub readable_end: u64,
}

/// Finite inherited display evidence. Full lineage provenance remains immutable
/// native authority; selected membership is bounded by the caller's header page.
#[cfg(test)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InheritedObservation {
    pub turns: Vec<super::reading::TurnReadingProvenance>,
    pub responses: Vec<InheritedResponseHeader>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InheritedResponseHeader {
    pub ordinal: u64,
    pub response: InheritedResponseSummary,
    pub models_total: u64,
    pub models_next_offset: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InheritedResponseSummary {
    pub process_message_ids: Vec<MessageId>,
    pub closing_message_id: Option<MessageId>,
    pub closing_ledger_position: u64,
    pub retry_message_id: Option<MessageId>,
    pub retry_ledger_position: Option<u64>,
    pub origin: InheritedResponseOrigin,
    pub completed_at: chrono::DateTime<chrono::Utc>,
    pub usage: Option<crate::model::ModelUsage>,
    pub timing: Option<super::response::CompletedResponseTiming>,
    pub models: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InheritedResponseOrigin {
    pub conversation_id: crate::runtime::identity::ConversationId,
    pub attempt_id: Option<crate::runtime::identity::AttemptId>,
    pub attempt_id_source: Option<crate::runtime_client::response::AttemptIdentitySource>,
    pub closing_message_id: Option<MessageId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InheritedModelName {
    pub ordinal: u64,
    pub encoded_bytes: u64,
    pub value: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InheritedModelPage {
    pub total: u64,
    pub offset: u64,
    pub names: Vec<InheritedModelName>,
    pub next_offset: Option<u64>,
}

/// One immutable allowlisted request inspection field at its native start cut.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestObservationHeader {
    pub request_id: Option<crate::runtime::identity::RequestId>,
    pub started_sequence: u64,
    pub value: serde_json::Value,
    pub content: Vec<ContentBinding>,
    pub collections: Vec<CollectionFrontier>,
    pub encoded_bytes: u64,
    pub children_total: u64,
    pub deferred: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestObservationField {
    pub path: Vec<String>,
    pub key: Option<RequestObservationKey>,
    pub value: serde_json::Value,
    pub content: Vec<ContentBinding>,
    pub collections: Vec<CollectionFrontier>,
    pub encoded_bytes: u64,
    pub children_total: u64,
    pub deferred: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestObservationKey {
    pub path: Vec<String>,
    pub preview: String,
    pub encoded_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestObservationPage {
    pub total: u64,
    pub offset: u64,
    pub fields: Vec<RequestObservationField>,
    pub next_offset: Option<u64>,
}

/// Exact provider-request outcome at native J. Missing terminal evidence stays
/// absent; an immutable frozen request source alone proves no outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestObservationOutcome {
    pub terminal_sequence: u64,
    pub event_id: Option<crate::runtime::identity::EventId>,
    pub usage: Option<crate::model::ModelUsage>,
    pub failure: Option<crate::runtime_client::trace::TraceRequestFailure>,
    pub generation: Option<crate::runtime_client::trace::TraceGeneration>,
    pub failure_deferred: bool,
    pub encoded_bytes: u64,
}

/// Read-only native Surface append index; Compaction ownership is unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceBoundaryHeader {
    pub surface_revision: crate::conversation::SurfaceRevision,
    pub header: MessageHeader,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceBoundaryHeaderPage {
    pub through: crate::conversation::SurfaceRevision,
    pub total: u64,
    pub boundaries: Vec<SurfaceBoundaryHeader>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditBlockHeaderPage {
    pub cut: ObservationReadCut,
    pub total: u64,
    pub offset: u64,
    pub headers: Vec<MessageBlockHeader>,
    pub next_offset: Option<u64>,
}

/// Ephemeral read-only receipt of one existing native atomic write. It carries
/// no client cursor, acknowledgement, or independently mutable execution state.
#[derive(Debug, Clone)]
pub struct NativeObservationBatch {
    /// Source-released contiguous predecessor Journal prefix captured at this append.
    pub ready_before: Option<u64>,
    pub ready_t_through: Option<u64>,
    pub trace_total: Option<u64>,
    /// Same-transaction pending publication owned by a registry terminal family.
    pub pending: Option<std::sync::Arc<super::inbox::PendingObservationReceipt>>,
    pub source_unavailable: bool,
    /// Existing normalized client failure policy, never a raw provider error.
    pub failure: Option<crate::runtime_client::event::RuntimeClientAttemptFailure>,
    pub failure_deferred: bool,
    pub failure_readable_end: u64,
    pub failure_sequence: Option<u64>,
    pub finish_reason_deferred: bool,
    pub finish_reason: Option<NativeObservationFieldRef>,
    pub read_cut: ObservationReadCut,
    pub journal_range: Option<(u64, u64)>,
    pub canonical: Option<std::sync::Arc<super::inbox::CanonicalObservationBatch>>,
    pub transition: NativeObservationTransition,
}

/// Exact selected public metadata at the existing native Journal source.
#[derive(Debug, Clone)]
pub struct NativeObservationFieldRef {
    pub sequence: u64,
    pub path: Vec<String>,
    pub encoded_bytes: u64,
}

/// The closed atomic families produced by the existing runtime write owners.
/// Growing histories are represented by their native source frontiers, never
/// by a capped list pretending to be the complete sequence of semantic facts.
#[derive(Debug, Clone)]
pub enum NativeObservationTransition {
    Single(ObservationEventWitness),
    /// Attempt admission and its first logical turn share one native commit.
    AttemptFirstTurn {
        attempt: Box<ObservationEventWitness>,
        turn: Box<ObservationEventWitness>,
    },
    RequestStart {
        statuses: Vec<crate::runtime_client::snapshot::AgentStatusView>,
        started: ObservationEventWitness,
        /// Finite native source head; all contribution rows remain indexed at J.
        contributions: Vec<ObservationEventWitness>,
        contribution_total: u64,
    },
    Adoption(ObservationEventWitness),
    ToolResults {
        /// The foreground window is the first 32 native Tool occurrence ordinals.
        loaded_results: Vec<NativeToolResultCommit>,
        tool_result_count: u64,
    },
    Compaction(ObservationEventWitness),
    /// Workflow terminal and its one native output commit share a transaction.
    WorkflowTerminal {
        terminal: Box<ObservationEventWitness>,
        output: Box<ObservationEventWitness>,
    },
    /// One accepted model output and its optional no-tool boundary share one
    /// native transaction and become visible together at its final Journal fact.
    ModelTurnSettlement {
        provider: Box<ObservationEventWitness>,
        canonical: Box<ObservationEventWitness>,
        assistant: std::sync::Arc<super::inbox::CanonicalObservationBatch>,
        boundary: Option<ModelTurnObservationBoundary>,
    },
}

/// The existing no-tool boundary either finishes the Attempt or adopts pending
/// input. Its fixed shape never substitutes a capped list for growing history.
#[derive(Debug, Clone)]
pub enum ModelTurnObservationBoundary {
    Completed {
        turn: Box<ObservationEventWitness>,
        attempt: Box<ObservationEventWitness>,
    },
    Adopted {
        turn: Box<ObservationEventWitness>,
        adoption: Box<ObservationEventWitness>,
        canonical: std::sync::Arc<super::inbox::CanonicalObservationBatch>,
    },
}

/// A complete finite terminal fact for one loaded native Tool occurrence.
/// Original results and diagnostics stay at the existing canonical sources.
#[derive(Debug, Clone)]
pub struct NativeToolResultCommit {
    pub occurrence: crate::events::types::ToolObservationOccurrence,
    pub result_ledger_position: u64,
    pub content_total: u64,
    pub result_deferred: bool,
    /// Positive native status/duration/exit/truncation summary, with deferred bodies.
    pub result: crate::tools::types::ToolExecutionResult,
    /// Canonical metadata includes the exact original status and diagnostics.
    pub metadata: crate::runtime_client::content::ContentRef,
    pub content: Vec<ContentBinding>,
}
