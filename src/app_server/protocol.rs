//! Rust authority for the App Server v57 envelope and method vocabulary.
//!
//! Request identities correlate responses on a connection. They carry no
//! execution identity, persistence, or exactly-once guarantee.

use serde::{Deserialize, Serialize};

use crate::local_runtime::session::{SessionId, SessionNodeId, SessionPersistentState};
use crate::runtime::identity::{ConversationId, MessageId};
use crate::runtime::interaction::{InteractionRef, InteractionResponse};
use crate::runtime_client::types::{AttachmentId, RuntimeClientCursor};

/// Independent of crate, journal, manifest and local stdio protocol versions.
/// One version identifies the complete mandatory method vocabulary. No compatibility mode.
pub const APP_SERVER_PROTOCOL_VERSION: u16 = 57;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum JsonRpcVersion {
    #[serde(rename = "2.0")]
    V2,
}

/// Integer request IDs use the JavaScript safe-integer range at admission.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(untagged)]
#[schemars(rename = "AppServerRequestId")]
pub enum RequestId {
    String(String),
    Integer(i64),
}

struct IdentityBudget(usize);
impl std::io::Write for IdentityBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("request identity exceeds limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl RequestId {
    /// Validate the re-encoded identity, including JSON escaping, before admission.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Even unescaped JSON needs two quote bytes. Refuse a giant typed
        // String before the serializer scans it for escaping.
        if matches!(self, Self::String(value) if value.len() > 254) {
            return false;
        }
        if matches!(self, Self::Integer(n) if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(n))
        {
            return false;
        }
        serde_json::to_writer(IdentityBudget(256), self).is_ok()
    }
    /// Native correlation identities have the same encoded String budget.
    #[must_use]
    pub fn is_valid_string(value: &str) -> bool {
        value.len() <= 254 && serde_json::to_writer(IdentityBudget(256), value).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClientIdentity {
    pub name: String,
    pub version: String,
}

/// Rendering hints never confer execution or interaction authority.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentationCapabilities {
    pub images: bool,
    pub questionnaires: bool,
    pub reviews: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitializeParams {
    pub protocol_version: u16,
    pub client: ClientIdentity,
    pub presentation: PresentationCapabilities,
}

/// Every attached operation addresses all routing domains explicitly.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentTarget {
    pub session_id: SessionId,
    pub conversation_id: ConversationId,
    pub runtime_incarnation: crate::local_runtime::session_runtime_manager::RuntimeIncarnationId,
    pub attachment_id: AttachmentId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(remote = "Self", deny_unknown_fields)]
pub struct Request {
    pub jsonrpc: JsonRpcVersion,
    pub id: RequestId,
    #[serde(flatten)]
    pub call: Method,
}

pub use crate::local_runtime::session::uploads::UserInputBlock;

/// A single public method space, with no nested Runtime Client envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
#[allow(clippy::large_enum_variant)] // Wire commands are short-lived and bounded by the transport frame.
pub enum Method {
    #[serde(rename = "auxiliary/classify")]
    AuxiliaryClassify {
        target: AttachmentTarget,
        submission: crate::runtime::auxiliary::submission::ClassificationSubmission,
    },
    #[serde(rename = "auxiliary/btw")]
    AuxiliaryBtw {
        target: AttachmentTarget,
        submission: crate::runtime::auxiliary::submission::BtwSubmission,
    },
    #[serde(rename = "auxiliary/status")]
    AuxiliaryStatus {
        target: AttachmentTarget,
        call_id: crate::runtime::identity::AuxiliaryCallId,
    },
    #[serde(rename = "auxiliary/result")]
    AuxiliaryResult {
        target: AttachmentTarget,
        call_id: crate::runtime::identity::AuxiliaryCallId,
    },
    #[serde(rename = "auxiliary/cancel")]
    AuxiliaryCancel {
        target: AttachmentTarget,
        call_id: crate::runtime::identity::AuxiliaryCallId,
    },
    #[serde(rename = "auxiliary/discard")]
    AuxiliaryDiscard {
        target: AttachmentTarget,
        call_id: crate::runtime::identity::AuxiliaryCallId,
    },
    #[serde(rename = "auxiliary/submissionLookup")]
    AuxiliarySubmissionLookup {
        target: AttachmentTarget,
        correlation: crate::runtime::auxiliary::submission::CorrelationId,
        #[serde(default)]
        scope: Option<crate::model::AuxiliaryAttribution>,
    },

    #[serde(rename = "agent/artifactRead")]
    AgentArtifactRead {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
        artifact_id: crate::runtime::identity::ArtifactId,
    },
    #[serde(rename = "agent/deliveryRead")]
    AgentDeliveryRead {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
        locator: crate::observation::content::CanonicalContentLocator,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        delivery_index: usize,
    },
    #[serde(rename = "agent/deliveryLocate")]
    AgentDeliveryLocate {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
        locator: crate::observation::content::CanonicalContentLocator,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        delivery_index: usize,
    },
    #[serde(rename = "artifact/read")]
    ArtifactRead {
        target: AttachmentTarget,
        artifact_id: crate::runtime::identity::ArtifactId,
    },
    /// Original bytes (at most 512 KiB) of one committed `present` delivery.
    ///
    /// Requires delivery access, which only transport authentication grants
    /// (an explicitly delegated stdio owner, or the separate WebSocket
    /// delivery credential). Without it this fails `session_file_read`
    /// `unauthorized` before any lookup, whatever the coordinates.
    #[serde(rename = "delivery/read")]
    DeliveryRead {
        target: AttachmentTarget,
        locator: crate::observation::content::CanonicalContentLocator,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        delivery_index: usize,
    },
    /// The verified absolute native path and current leaf identity of one
    /// committed delivery, under the same authority as `delivery/read`.
    /// Transfers no bytes. Only a client that demonstrably shares the
    /// server's filesystem may interpret the path locally.
    #[serde(rename = "delivery/locate")]
    DeliveryLocate {
        target: AttachmentTarget,
        locator: crate::observation::content::CanonicalContentLocator,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        delivery_index: usize,
    },
    /// Cancels this connection's own in-flight delivery operation, including
    /// auxiliary status/result/lookup reads, with exactly this request id.
    ///
    /// `accepted: true` means the cancellation won before that request's
    /// publication commit: its one response is `delivery_cancelled`, sent
    /// after its native work has physically settled. `false` means no such
    /// request is running on this connection (unknown id, or its response
    /// was already committed); its response stands as produced. Other
    /// requests and connections are never affected.
    #[serde(rename = "delivery/cancel")]
    DeliveryCancel { request_id: RequestId },
    #[serde(rename = "session/uploadPrepare")]
    SessionUploadPrepare {
        target: AttachmentTarget,
        operation_id: String,
        /// One file per wire operation keeps committed receipts recoverable within a record.
        #[schemars(length(min = 1, max = 1))]
        files: Vec<crate::local_runtime::session::uploads::UploadMetadata>,
    },
    #[serde(rename = "session/uploadStatus")]
    SessionUploadStatus {
        target: AttachmentTarget,
        operation_id: String,
    },
    #[serde(rename = "session/switchNode")]
    SessionSwitchNode {
        target: AttachmentTarget,
        node_id: SessionNodeId,
    },
    #[serde(rename = "session/projections")]
    Projections {
        target: AttachmentTarget,
        after: crate::context::checkpoint::ProjectionActivationId,
        limit: usize,
    },
    #[serde(rename = "session/projectionDetail")]
    ProjectionDetail {
        target: AttachmentTarget,
        checkpoint_id: crate::runtime::identity::CheckpointId,
        offset: u32,
        limit: usize,
    },
    #[serde(rename = "session/trace")]
    Trace {
        target: AttachmentTarget,
        #[serde(default)]
        #[schemars(length(max = 8))]
        records: Vec<crate::observation::trace::TraceCursor>,
        before: Option<crate::observation::trace::TraceCursor>,
        limit: usize,
    },
    /// Heavy inspection detail for one exact Trace record identity.
    ///
    /// Separated from `session/trace` so a page of summaries stays cheap:
    /// request contexts, Tool schemas and Tool results are fetched only for
    /// the record a reader selected. Like every Trace read it mutates
    /// nothing and advances no cursor.
    #[serde(rename = "session/traceDetail")]
    TraceDetail {
        target: AttachmentTarget,
        #[schemars(length(max = 256))]
        record_id: String,
    },
    #[serde(rename = "session/traceLocateTool")]
    TraceLocateTool {
        target: AttachmentTarget,
        locator: crate::runtime_client::trace::TraceToolLocator,
    },
    #[serde(rename = "session/transcript")]
    Transcript {
        target: AttachmentTarget,
        at: crate::durable::reading::ConversationWindowAt,
        #[schemars(range(min = 1, max = 32))]
        limit: usize,
    },
    #[serde(rename = "session/turns")]
    ConversationTurns {
        target: AttachmentTarget,
        /// Absent selects the newest native outline page.
        offset: Option<usize>,
        #[schemars(range(min = 1, max = 64))]
        limit: usize,
    },
    #[serde(rename = "session/model")]
    ModelGet { target: AttachmentTarget },
    #[serde(rename = "session/models")]
    ModelCatalog { target: AttachmentTarget },
    #[serde(rename = "session/setModel")]
    ModelSet {
        target: AttachmentTarget,
        config: Box<crate::model::session::SessionModelConfig>,
    },
    /// Choose catalog defaults by finite native ordinals, even when names are deferred.
    #[serde(rename = "session/selectModel")]
    ModelSelect {
        target: AttachmentTarget,
        choice: crate::model::session::CatalogModelSelection,
    },
    #[serde(rename = "resources/read")]
    Capability { target: AttachmentTarget },
    #[serde(rename = "context/compact")]
    CompactContext {
        target: AttachmentTarget,
        request_id: crate::runtime::identity::ManualCompactionRequestId,
    },
    /// Durable exact-node controls. These do not acquire a runtime attachment.

    #[serde(rename = "workflow/scheduled/getRecovery")]
    WorkflowScheduledRecovery {
        session_id: SessionId,
        node_id: SessionNodeId,
        start_id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
        after_effect_ordinal: u64,
        #[schemars(range(min = 1, max = 100))]
        limit: u32,
    },
    #[serde(rename = "workflow/scheduled/cancel")]
    WorkflowScheduledCancel {
        session_id: SessionId,
        node_id: SessionNodeId,
        start_id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
    },
    #[serde(rename = "workflow/scheduled/reconcile")]
    WorkflowScheduledReconcile {
        session_id: SessionId,
        node_id: SessionNodeId,
        start_id: crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId,
        after_effect_ordinal: u64,
        #[schemars(range(min = 1, max = 100))]
        limit: u32,
    },
    #[serde(rename = "schedule/list")]
    ScheduleList {
        session_id: SessionId,
        node_id: SessionNodeId,
        after_ordinal: u64,
        #[schemars(range(min = 1, max = 100))]
        limit: u32,
    },
    #[serde(rename = "schedule/get")]
    ScheduleGet {
        session_id: SessionId,
        node_id: SessionNodeId,
        id: crate::scheduler::ScheduleId,
    },
    #[serde(rename = "schedule/command")]
    ScheduleCommand {
        session_id: SessionId,
        node_id: SessionNodeId,
        command: crate::scheduler::ScheduleCommand,
    },
    #[serde(rename = "schedule/recoverStorage")]
    ScheduleRecoverStorage {
        session_id: SessionId,
        node_id: SessionNodeId,
    },
    #[serde(rename = "goal/control")]
    Goal {
        target: AttachmentTarget,
        control: crate::goal::GoalControl,
    },
    #[serde(rename = "job/status")]
    JobStatus {
        target: AttachmentTarget,
        job_id: crate::runtime::identity::ToolExecutionId,
    },
    #[serde(rename = "job/list")]
    JobList { target: AttachmentTarget },
    #[serde(rename = "job/wait")]
    JobWait {
        target: AttachmentTarget,
        job_id: crate::runtime::identity::ToolExecutionId,
    },
    #[serde(rename = "job/cancel")]
    JobCancel {
        target: AttachmentTarget,
        job_id: crate::runtime::identity::ToolExecutionId,
    },
    #[serde(rename = "agent/status")]
    AgentStatus {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
    },
    #[serde(rename = "agent/list")]
    AgentList { target: AttachmentTarget },
    #[serde(rename = "agent/observeCancel")]
    AgentObserveCancel { request_id: RequestId },
    #[serde(rename = "agent/observe")]
    AgentObserve {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
        after: Option<crate::runtime_client::agent_observation::AgentObservationCursor>,
    },
    #[serde(rename = "agent/statistics")]
    AgentStatistics {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
    },
    #[serde(rename = "agent/sendMessage")]
    AgentSendMessage {
        attachments: Vec<crate::local_runtime::session::uploads::UploadReceipt>,
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
        message: String,
    },
    #[serde(rename = "agent/wait")]
    AgentWait {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
    },
    #[serde(rename = "agent/interrupt")]
    AgentInterrupt {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
    },
    /// Independent child-owned Trace read domain; no parent records are used.
    #[serde(rename = "agent/trace")]
    AgentTrace {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
        before: Option<crate::observation::trace::TraceCursor>,
        limit: usize,
        #[serde(default)]
        #[schemars(length(max = 8))]
        records: Vec<crate::observation::trace::TraceCursor>,
    },
    #[serde(rename = "agent/traceDetail")]
    AgentTraceDetail {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
        record_id: String,
    },
    #[serde(rename = "agent/traceLocateTool")]
    AgentTraceLocateTool {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
        locator: crate::runtime_client::trace::TraceToolLocator,
    },
    /// Read the exact child's bounded canonical history; never accepts a child path.
    #[serde(rename = "agent/transcript")]
    AgentTranscript {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
        at: crate::durable::reading::ConversationWindowAt,
        #[schemars(range(min = 1, max = 32))]
        limit: usize,
    },
    /// All turns across the Agent's activations, in native start order.
    #[serde(rename = "agent/turns")]
    AgentTurns {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
        offset: Option<usize>,
        #[schemars(range(min = 1, max = 64))]
        limit: usize,
    },
    #[serde(rename = "initialize")]
    Initialize(InitializeParams),
    #[serde(rename = "server/info")]
    ServerInfo {},
    #[serde(rename = "server/diagnostics")]
    ServerDiagnostics {},
    #[serde(rename = "session/exportPrepare")]
    SessionExportPrepare { session_id: SessionId },
    #[serde(rename = "session/list")]
    SessionList {
        query: Option<String>,
        offset: usize,
        limit: usize,
    },
    /// Commit Session identity and validated initial settings without composing a runtime.
    /// An acknowledged identity remains committed when `durability_diagnostic` is present.
    #[serde(rename = "session/create")]
    SessionCreate { settings: SessionPersistentState },
    #[serde(rename = "session/read")]
    SessionRead { session_id: SessionId },
    /// Read durable history without loading runtime resources or acquiring control.
    #[serde(rename = "session/history")]
    SessionHistory {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        at: crate::durable::reading::ConversationWindowAt,
        limit: usize,
    },
    /// Inspect durable trace without composing a runtime or acquiring control.
    #[serde(rename = "session/traceHistory")]
    SessionTraceHistory {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        before: Option<crate::observation::trace::TraceCursor>,
        limit: usize,
    },
    #[serde(rename = "session/traceHistoryDetail")]
    SessionTraceHistoryDetail {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        record_id: String,
    },
    #[serde(rename = "session/tracePage")]
    SessionTracePage {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        cut: crate::observation::cut::CheckpointReadCut,
        domain: String,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 32))]
        limit: usize,
    },
    #[serde(rename = "session/traceValueRead")]
    SessionTraceValueRead {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        cut: crate::observation::cut::CheckpointReadCut,
        domain: String,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        item_offset: u64,
        #[schemars(length(max = 32))]
        path: Vec<String>,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 65_536))]
        max_bytes: u32,
    },
    #[serde(rename = "session/traceContentRead")]
    SessionTraceContentRead {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        cut: crate::observation::cut::CheckpointReadCut,
        content_ref: crate::observation::content::ContentRef,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 65_536))]
        max_bytes: u32,
    },
    #[serde(rename = "session/historyContentRead")]
    SessionHistoryContentRead {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        cut: crate::durable::observation::ObservationReadCut,
        content_ref: crate::observation::content::ContentRef,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 65_536))]
        max_bytes: u32,
    },
    /// Continues a collection from an exact historical native window cut.
    #[serde(rename = "session/historyPage")]
    SessionHistoryPage {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        cut: crate::durable::observation::ObservationReadCut,
        domain: String,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 32))]
        limit: usize,
    },
    /// Reads a selected historical metadata value in bounded byte ranges.
    #[serde(rename = "session/historyValueRead")]
    SessionHistoryValueRead {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        cut: crate::durable::observation::ObservationReadCut,
        domain: String,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        item_offset: u64,
        #[schemars(length(max = 32))]
        path: Vec<String>,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 65_536))]
        max_bytes: u32,
    },
    #[serde(rename = "session/statistics")]
    SessionStatistics {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
    },
    #[serde(rename = "session/summary")]
    SessionSummary { session_id: SessionId },
    #[serde(rename = "session/name")]
    SessionName { session_id: SessionId, name: String },
    /// Read one exact native node without runtime composition or tree enumeration.
    #[serde(rename = "session/node")]
    SessionNode {
        session_id: SessionId,
        node_id: SessionNodeId,
    },
    #[serde(rename = "session/nodeOriginHistory")]
    SessionNodeOriginHistory {
        session_id: SessionId,
        node_id: SessionNodeId,
        limit: usize,
    },
    #[serde(rename = "session/nodeValueRead")]
    SessionNodeValueRead {
        reference: crate::local_runtime::session::observation::SessionNodeValueRef,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 65_536))]
        max_bytes: u32,
    },
    #[serde(rename = "session/tree")]
    SessionTree {
        session_id: SessionId,
        offset: usize,
        limit: usize,
    },
    #[serde(rename = "session/boundaries")]
    SessionBoundaries {
        target: AttachmentTarget,
        offset: usize,
        limit: usize,
    },
    #[serde(rename = "session/forkProjection")]
    SessionForkProjection {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        ledger: crate::context::resources::LedgerFrontier,
        activation: crate::context::checkpoint::ProjectionActivationId,
    },
    #[serde(rename = "session/fork")]
    SessionFork {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        surface_revision: crate::conversation::SurfaceRevision,
        boundary: Option<crate::observation::content::CanonicalContentLocator>,
        side: crate::local_runtime::session::LineageSide,
    },
    #[serde(rename = "session/branch")]
    SessionBranch {
        session_id: SessionId,
        node_id: SessionNodeId,
        surface_revision: crate::conversation::SurfaceRevision,
        boundary: crate::observation::content::CanonicalContentLocator,
        side: crate::local_runtime::session::LineageSide,
    },
    #[serde(rename = "session/deletePreview")]
    SessionDeletePreview { session_id: SessionId },
    #[serde(rename = "session/delete")]
    SessionDelete {
        session_id: SessionId,
        expected_target_revision: String,
    },
    #[serde(rename = "session/recoverDeletion")]
    SessionRecoverDeletion { session_id: SessionId },
    #[serde(rename = "session/attach")]
    SessionAttach {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
    },
    #[serde(rename = "session/editorRead")]
    SessionEditorRead {
        locator: super::editor_read::EditorLocator,
        block_index: u32,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 65_536))]
        max_bytes: u32,
    },
    #[serde(rename = "session/detach")]
    SessionDetach { target: AttachmentTarget },
    #[serde(rename = "session/attachmentStatus")]
    SessionAttachmentStatus { session_id: SessionId },
    #[serde(rename = "conversation/contentRead")]
    ConversationContentRead {
        target: AttachmentTarget,
        #[serde(default)]
        agent_id: Option<crate::runtime::identity::AgentId>,
        content_ref: crate::observation::content::ContentRef,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 65_536))]
        max_bytes: u32,
    },
    #[serde(rename = "conversation/checkpointValueRead")]
    ConversationCheckpointValueRead {
        target: AttachmentTarget,
        agent_id: Option<crate::runtime::identity::AgentId>,
        cut: crate::observation::cut::CheckpointReadCut,
        domain: String,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        item_offset: u64,
        #[schemars(length(max = 32))]
        path: Vec<String>,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 65_536))]
        max_bytes: u32,
    },
    #[serde(rename = "conversation/checkpointPage")]
    ConversationCheckpointPage {
        target: AttachmentTarget,
        #[serde(default)]
        agent_id: Option<crate::runtime::identity::AgentId>,
        cut: crate::observation::cut::CheckpointReadCut,
        domain: String,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        limit: usize,
    },
    #[serde(rename = "session/checkpoint")]
    SessionCheckpoint {
        target: AttachmentTarget,
        /// Bounded loaded Trace identities to repair at the same snapshot cut.
        #[serde(default)]
        #[schemars(length(max = 8))]
        trace_records: Vec<crate::observation::trace::TraceCursor>,
    },
    #[serde(rename = "session/subscribe")]
    SessionSubscribe {
        target: AttachmentTarget,
        after_cursor: RuntimeClientCursor,
    },
    #[serde(rename = "turn/start")]
    TurnStart {
        target: AttachmentTarget,
        content: Vec<UserInputBlock>,
    },
    #[serde(rename = "turn/steer")]
    TurnSteer {
        target: AttachmentTarget,
        content: Vec<UserInputBlock>,
    },
    #[serde(rename = "inbound/edit")]
    InboundEdit {
        target: AttachmentTarget,
        expected: crate::durable::inbox::PendingInboundRef,
        text: String,
    },
    #[serde(rename = "inbound/remove")]
    InboundRemove {
        target: AttachmentTarget,
        expected: crate::durable::inbox::PendingInboundRef,
    },
    #[serde(rename = "turn/cancel")]
    TurnCancel { target: AttachmentTarget },
    #[serde(rename = "interaction/respond")]
    InteractionRespond {
        target: AttachmentTarget,
        interaction: InteractionRef,
        response: InteractionResponse,
    },
    #[serde(rename = "interaction/cancel")]
    InteractionCancel {
        target: AttachmentTarget,
        interaction: InteractionRef,
    },
    #[serde(rename = "session/effectiveConfiguration")]
    ConfigurationGet { target: AttachmentTarget },
    #[serde(rename = "session/effectiveConfigurationValueRead")]
    ConfigurationValueRead {
        target: AttachmentTarget,
        reference: crate::local_runtime::configuration::effective_observation::EffectiveFieldRef,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 65_536))]
        max_bytes: u32,
    },
    #[serde(rename = "session/effectiveConfigurationPage")]
    ConfigurationPage {
        target: AttachmentTarget,
        revision: String,
        field: crate::local_runtime::configuration::effective_observation::EffectiveFieldName,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 32))]
        limit: usize,
    },
    #[serde(rename = "configuration/sourcesRead")]
    SourcesRead {
        target: crate::local_runtime::configuration::settings::SourceTarget,
    },
    /// Finite configuration connectivity check; never an Agent connection.
    #[serde(rename = "mcp/probe")]
    McpProbe {
        target: crate::local_runtime::configuration::settings::SourceTarget,
        id: crate::runtime::identity::McpServerId,
        expected_revision: String,
    },
    #[serde(rename = "configuration/sourcePage")]
    SourcesPage {
        target: crate::local_runtime::configuration::settings::SourceTarget,
        revision: String,
        field: crate::local_runtime::configuration::observation::SourceFieldName,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 32))]
        limit: usize,
    },
    #[serde(rename = "configuration/sourceValueRead")]
    SourcesValueRead {
        target: crate::local_runtime::configuration::settings::SourceTarget,
        reference: crate::local_runtime::configuration::observation::SourceFieldRef,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 65_536))]
        max_bytes: u32,
    },
    /// The Workflow files physically owned by exactly one source scope
    /// (Issue #491); never the merged effective catalog.
    #[serde(rename = "workflow/list")]
    WorkflowList {
        target: crate::local_runtime::configuration::settings::SourceTarget,
    },
    /// The complete authorized source of one exact Workflow file.
    #[serde(rename = "workflow/read")]
    WorkflowRead {
        target: crate::local_runtime::configuration::settings::SourceTarget,
        id: crate::runtime::workflow::WorkflowId,
    },
    /// Bounded, revision-fenced registered Model/Profile choices. No execution.
    #[serde(rename = "workflow/modelChoices")]
    WorkflowModelChoices {
        target: crate::local_runtime::configuration::settings::SourceTarget,
        query: crate::local_runtime::workflow_authoring::WorkflowModelChoicesQuery,
        #[serde(default)]
        expected_revision: Option<String>,
    },
    /// Offline structural check, optional explanation, and dependency
    /// admission in an explicit Workspace context of one candidate document.
    /// Executes nothing; writes nothing.
    #[serde(rename = "workflow/check")]
    WorkflowCheck {
        target: crate::local_runtime::configuration::settings::SourceTarget,
        id: crate::runtime::workflow::WorkflowId,
        document: Box<crate::runtime::workflow::WorkflowDefinition>,
        /// A User (Global) file's explicit Workspace dependency context. A
        /// Workspace file is always checked in its own Workspace.
        #[serde(default)]
        context: Option<std::path::PathBuf>,
        #[serde(default)]
        explain: bool,
    },
    #[serde(rename = "configuration/sourceWrite")]
    SourcesWrite {
        target: crate::local_runtime::configuration::settings::SourceTarget,
        expected_revision: String,
        mutation: crate::local_runtime::configuration::settings::SourceMutation,
    },
    #[serde(rename = "session/settings")]
    SettingsRead { session_id: SessionId },
    #[serde(rename = "session/settingsValueRead")]
    SettingsValueRead {
        session_id: SessionId,
        reference: crate::local_runtime::configuration::observation::SessionSettingsRef,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1, max = 65_536))]
        max_bytes: u32,
    },
    #[serde(rename = "configuration/reconcile")]
    ConfigurationReconcile {
        target: crate::local_runtime::configuration::settings::SourceTarget,
    },
    #[serde(rename = "session/configurationValueRead")]
    SessionApplicationValueRead {
        session_id: SessionId,
        reference: crate::local_runtime::configuration::observation::SessionApplicationRef,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
        #[schemars(range(min = 1_u32, max = 65_536_u32))]
        max_bytes: u32,
    },
    #[serde(rename = "session/configuration")]
    SessionConfiguration { session_id: SessionId },
    #[serde(rename = "session/adoptConfiguration")]
    AdoptConfiguration {
        session_id: SessionId,
        candidate: crate::local_runtime::configuration::application::ApplicationIdentity,
        expected_binding: u64,
    },
}

impl Method {
    /// A lost successful response can hide an execution or connection-state commit.
    #[allow(clippy::too_many_lines)] // One exhaustive admission audit covers every public method.
    pub(crate) const fn may_commit(&self) -> bool {
        match self {
            Self::Goal { control, .. } => !matches!(control, crate::goal::GoalControl::Show),
            Self::WorkflowScheduledRecovery { .. }
            | Self::ScheduleList { .. }
            | Self::ScheduleGet { .. }
            | Self::AuxiliaryStatus { .. }
            | Self::AuxiliaryResult { .. }
            | Self::AuxiliarySubmissionLookup { .. }
            | Self::AgentArtifactRead { .. }
            | Self::AgentDeliveryRead { .. }
            | Self::AgentDeliveryLocate { .. }
            | Self::ArtifactRead { .. }
            | Self::DeliveryRead { .. }
            | Self::DeliveryLocate { .. }
            | Self::SessionUploadStatus { .. }
            | Self::Trace { .. }
            | Self::Projections { .. }
            | Self::ProjectionDetail { .. }
            | Self::TraceDetail { .. }
            | Self::TraceLocateTool { .. }
            | Self::Transcript { .. }
            | Self::ConversationTurns { .. }
            | Self::ModelGet { .. }
            | Self::ModelCatalog { .. }
            | Self::Capability { .. }
            | Self::JobStatus { .. }
            | Self::JobList { .. }
            | Self::JobWait { .. }
            | Self::AgentStatus { .. }
            | Self::AgentList { .. }
            | Self::AgentObserve { .. }
            | Self::AgentStatistics { .. }
            | Self::AgentWait { .. }
            | Self::AgentTrace { .. }
            | Self::AgentTraceDetail { .. }
            | Self::AgentTraceLocateTool { .. }
            | Self::AgentTranscript { .. }
            | Self::AgentTurns { .. }
            | Self::ServerInfo { .. }
            | Self::ServerDiagnostics { .. }
            | Self::SessionList { .. }
            | Self::SessionRead { .. }
            | Self::SessionHistory { .. }
            | Self::SessionTraceHistory { .. }
            | Self::SessionTraceHistoryDetail { .. }
            | Self::SessionTracePage { .. }
            | Self::SessionTraceValueRead { .. }
            | Self::SessionTraceContentRead { .. }
            | Self::SessionStatistics { .. }
            | Self::SessionSummary { .. }
            | Self::SessionNode { .. }
            | Self::SessionNodeValueRead { .. }
            | Self::SessionNodeOriginHistory { .. }
            | Self::SessionHistoryContentRead { .. }
            | Self::SessionHistoryPage { .. }
            | Self::SessionHistoryValueRead { .. }
            | Self::SessionTree { .. }
            | Self::SessionBoundaries { .. }
            | Self::SessionDeletePreview { .. }
            | Self::SessionCheckpoint { .. }
            | Self::ConversationContentRead { .. }
            | Self::ConversationCheckpointPage { .. }
            | Self::ConversationCheckpointValueRead { .. }
            | Self::SessionAttachmentStatus { .. }
            | Self::SessionEditorRead { .. }
            | Self::ConfigurationGet { .. }
            | Self::ConfigurationValueRead { .. }
            | Self::ConfigurationPage { .. }
            | Self::SourcesRead { .. }
            | Self::SourcesPage { .. }
            | Self::SourcesValueRead { .. }
            | Self::WorkflowList { .. }
            | Self::WorkflowRead { .. }
            | Self::WorkflowModelChoices { .. }
            | Self::WorkflowCheck { .. }
            | Self::McpProbe { .. }
            | Self::SettingsRead { .. }
            | Self::SettingsValueRead { .. }
            | Self::SessionConfiguration { .. }
            | Self::SessionApplicationValueRead { .. } => false,
            Self::AuxiliaryClassify { .. }
            | Self::AuxiliaryBtw { .. }
            | Self::AuxiliaryCancel { .. }
            | Self::AuxiliaryDiscard { .. }
            | Self::DeliveryCancel { .. }
            | Self::SessionUploadPrepare { .. }
            | Self::SessionSwitchNode { .. }
            | Self::ModelSet { .. }
            | Self::ModelSelect { .. }
            | Self::CompactContext { .. }
            | Self::JobCancel { .. }
            | Self::AgentObserveCancel { .. }
            | Self::AgentSendMessage { .. }
            | Self::AgentInterrupt { .. }
            | Self::Initialize(..)
            | Self::SessionExportPrepare { .. }
            | Self::SessionCreate { .. }
            | Self::WorkflowScheduledCancel { .. }
            | Self::WorkflowScheduledReconcile { .. }
            | Self::ScheduleCommand { .. }
            | Self::ScheduleRecoverStorage { .. }
            | Self::SessionName { .. }
            | Self::SessionFork { .. }
            | Self::SessionForkProjection { .. }
            | Self::SessionBranch { .. }
            | Self::SessionDelete { .. }
            | Self::SessionRecoverDeletion { .. }
            | Self::SessionAttach { .. }
            | Self::SessionDetach { .. }
            | Self::SessionSubscribe { .. }
            | Self::TurnStart { .. }
            | Self::TurnSteer { .. }
            | Self::InboundEdit { .. }
            | Self::InboundRemove { .. }
            | Self::TurnCancel { .. }
            | Self::InteractionRespond { .. }
            | Self::InteractionCancel { .. }
            | Self::SourcesWrite { .. }
            | Self::ConfigurationReconcile { .. }
            | Self::AdoptConfiguration { .. } => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ErrorData {
    Auxiliary {
        reason: crate::runtime::auxiliary::submission::AuxiliaryFailure,
    },
    SessionFileRead {
        reason: crate::tools::session_files::SessionFileReadFailure,
    },
    ConfigurationAdoption {
        rejection: crate::runtime::adoption::AdoptionError,
    },
    /// The Session's published Model Catalog does not admit its configured
    /// selection, so no work is accepted until `session/setModel` commits a
    /// selection it does admit. Nothing falls back to a default.
    ModelUnavailable {
        diagnostic: String,
    },
    ArchivePreparationFailed {
        reason: crate::session_archive::SessionArchivePrepareError,
    },

    JobPublicationAbandoned {
        job_id: crate::runtime::identity::ToolExecutionId,
    },
    /// The input frame was provably not written; no input was delivered.
    AgentNotDelivered {
        agent_id: crate::runtime::identity::AgentId,
    },
    /// An input write was attempted, but canonical acceptance was not acknowledged.
    /// Automatic replay could duplicate user guidance.
    AgentDeliveryUnknown {
        agent_id: crate::runtime::identity::AgentId,
    },
    AgentStopping {
        agent_id: crate::runtime::identity::AgentId,
    },
    AgentSettlement {
        agent_id: crate::runtime::identity::AgentId,
    },
    UnknownAgent {
        agent_id: crate::runtime::identity::AgentId,
    },
    AgentHistoryUnavailable {
        agent_id: crate::runtime::identity::AgentId,
    },
    UnknownSubagent {
        subagent_id: crate::runtime::identity::SubagentId,
    },
    SubagentHistoryUnavailable {
        subagent_id: crate::runtime::identity::SubagentId,
    },
    RequestCapacity,
    DuplicateRequestId,
    ObservationUnavailable {
        diagnostic: String,
    },
    ContentSourceUnavailable,
    InvalidRange,
    ReadCutExpired,
    ObservationCapacityExceeded,
    ObservationInUse,
    ResponseTooLarge,
    ResponseSerializationFailed,
    OutcomeUnknown,
    ResidencyCapacity,
    AttachmentCapacity,
    ServerDraining,
    UnknownSession {
        session_id: SessionId,
    },
    UnknownNode {
        session_id: SessionId,
        node_id: SessionNodeId,
    },
    SourceConflict {
        scope: crate::runtime::resources::SourceScope,
        expected: String,
        actual: String,
    },
    StaleSettings {
        expected: u64,
        actual: u64,
    },
    /// The native parser or compiler rejected a Workflow candidate; the file
    /// is unchanged.
    WorkflowInvalid {
        diagnostic: Box<crate::local_runtime::workflow_authoring::WorkflowDiagnostic>,
    },
    InteractionNotPending {
        interaction: InteractionRef,
    },
    InteractionAuditFailed {
        interaction: InteractionRef,
    },
    CommittedDurabilityUncertain,
    UnsupportedVersion {
        supported: u16,
        requested: u16,
    },
    NotInitialized,
    AlreadyInitialized,
    /// The client cancelled this delivery request before publication.
    DeliveryCancelled,
    ObservationCancelled,
    SubscriptionSuperseded,
    StaleAttachment,
    StaleRuntime,
    ControllerInUse,
    InvalidState,
    InvalidParams,
    ResyncRequired,
    OperationFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<ErrorData>,
}

/// Success and failure are exclusive, including on deserialization.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(remote = "Self", untagged)]
pub enum Response {
    Success(Box<Success<MethodResult>>),
    Failure(Failure),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Success<R> {
    pub jsonrpc: JsonRpcVersion,
    pub id: RequestId,
    pub result: R,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub jsonrpc: JsonRpcVersion,
    pub id: Option<RequestId>,
    pub error: RpcError,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MethodResult {
    ScheduledWorkflowRecovery { recovery:Option<crate::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowRecovery> },

    Schedules {
        schedules: Vec<crate::scheduler::ScheduleHeader>,
        next_ordinal: Option<u64>,
    },
    Schedule { schedule: Option<crate::scheduler::ScheduleSnapshot> },
    ScheduleCommand { result: crate::scheduler::ScheduleCommandResult },
    ScheduleStorageRecovered { generation: u64 },
    AuxiliarySubmitted { receipt: crate::runtime::conversation_runtime::auxiliary::AuxiliarySubmissionReceipt },
    AuxiliaryStatus { status: crate::runtime::conversation_runtime::auxiliary::AuxiliaryStatus },
    AuxiliaryResult { result: Option<crate::runtime::auxiliary::delivery::AuxiliaryResult> },
    AuxiliarySubmissionLookup { lookup: crate::runtime::conversation_runtime::auxiliary::AuxiliarySubmissionLookup },
    AuxiliaryCancelled { accepted: bool },
    AuxiliaryDiscarded { discarded: bool },

    SessionFileBytes {
        file: crate::tools::session_files::SessionFileReference,
        data: String,
    },
    DeliveryCancel {
        accepted: bool,
    },
    SessionFileLocation {
        file: crate::tools::session_files::SessionFileReference,
        /// Absolute path in the server's filesystem namespace.
        path: String,
        /// Lossless decimal device/inode of the verified regular leaf.
        device: String,
        inode: String,
    },
    SessionArchive {
        download: super::archive_download::ArchiveDownloadDescriptor,
    },
    InboundMutation {
        outcome: crate::durable::inbox::PendingMutationOutcome,
    },
    ArtifactBytes {
        data: String,
    },
    UploadPrepared {
        transfer: super::upload_transfer::UploadDescriptor,
    },
    UploadStatus {
        outcome: crate::local_runtime::session::uploads::UploadOutcome,
    },
    Diagnostics {
        snapshot: crate::app_server::host::ServerDiagnostics,
    },
    ContextCompacted {
        compaction: Option<crate::observation::head::RuntimeClientCompactionView>,
    },
    Projections {
        page: crate::runtime_client::projection_history::ProjectionPage,
    },
    ProjectionDetail {
        detail: Box<crate::runtime_client::projection_history::ProjectionDetail>,
    },
    Trace {
        page: crate::observation::trace::TracePage,
    },
    TraceToolLocation {
        location: Option<crate::runtime_client::trace::TraceToolLocation>,
    },
    SessionHistory {
        conversation_id: ConversationId,
        window: crate::runtime_client::snapshot::ConversationWindow,
    },
    SessionStatistics {
        conversation_id: ConversationId,
        read_cut: crate::observation::cut::CheckpointReadCut,
        statistics: crate::observation::statistics::ConversationStatistics,
        occupancy: Option<crate::observation::occupancy::RuntimeClientContextOccupancy>,
    },
    SessionTraceHistory {
        conversation_id: ConversationId,
        page: crate::observation::trace::TracePage,
    },
    SessionTraceHistoryDetail {
        conversation_id: ConversationId,
        page: crate::runtime_client::checkpoint::CheckpointPage,
    },
    TranscriptWindow {
        window: crate::runtime_client::snapshot::ConversationWindow,
    },
    ConversationTurns {
        page: crate::observation::turn::ObservedTurnPage,
    },
    GoalMutation {
        reference: Option<crate::goal::GoalRef>,
    },
    JobCancellationAccepted { job_id: crate::runtime::identity::ToolExecutionId },
    AgentInterruptionAccepted { agent_id: crate::runtime::identity::AgentId, activation_id: Option<crate::runtime::identity::SubagentId> },
    JobOutcome {
        job_id: crate::runtime::identity::ToolExecutionId,
        state: crate::tools::background::BackgroundLifecycle,
    },
    AgentObserveCancelled {
        accepted: bool,
    },
    AgentObserve {
        observation: crate::runtime_client::agent_observation::AgentObservation,
    },
    AgentStatistics {
        metrics: crate::observation::statistics::AgentStatistics,
    },
    AgentMessage {
        agent_id: crate::runtime::identity::AgentId,
        activation_id: crate::runtime::identity::SubagentId,
        resumed: bool,
    },
    AgentWait {
        agent_id: crate::runtime::identity::AgentId,
        activation_id: Option<crate::runtime::identity::SubagentId>,
        outcome: Option<crate::runtime::subagent::SubagentState>,
    },
    Initialized {
        /// Opaque process-owner identity, stable across transport reconnects. Not permission.
        authority_id: String,
        protocol_version: u16,
        capabilities: ServerCapabilities,
    },
    ServerInfo {
        capabilities: ServerCapabilities,
    },
    Session {
        session: crate::local_runtime::session::SessionSnapshot,
    },
    EditorPage {
        page: super::editor_read::EditorPage,
    },
    SessionTransition {
        session: crate::local_runtime::session::SessionSnapshot,
        editor: Option<super::editor_read::EditorLocator>,
        durability_diagnostic: Option<String>,
    },
    SessionSummary {
        summary: crate::local_runtime::session::SessionSummary,
    },
    Sessions {
        sessions: Vec<crate::local_runtime::session::SessionSummary>,
        next_offset: Option<usize>,
    },
    Node {
        node: crate::local_runtime::session::observation::SessionNodeObservation,
    },
    NodeValueChunk {chunk:crate::local_runtime::session::observation::SessionNodeValueChunk},
    Tree {
        nodes: Vec<crate::local_runtime::session::observation::SessionNodeObservation>,
        next_offset: Option<usize>,
    },
    Boundaries {
        /// The exact committed head the page was selected against.
        surface_revision: crate::conversation::SurfaceRevision,
        boundaries: Vec<crate::local_runtime::session::observation::SessionUserMessageBoundaryObservation>,
        next_offset: Option<usize>,
    },
    Deletion {
        result: crate::app_server::session_deletion::SessionDeletionResult,
    },
    Attached {
        target: AttachmentTarget,
        configuration_revision: Option<u64>,
        attachment_kind: AttachmentKind,
        observation_source: ObservationSource,
    },
    AttachmentStatus {
        target: Option<AttachmentTarget>,
        configuration_revision: Option<u64>,
    },
    ContentChunk {
        chunk: crate::observation::content::ContentChunk,
    },
    DomainPage {
        page: crate::runtime_client::checkpoint::CheckpointPage,
    },
    CheckpointValueChunk {
        chunk: crate::runtime_client::checkpoint::CheckpointValueChunk,
    },
    CheckpointPage {
        page: crate::runtime_client::checkpoint::CheckpointPage,
    },
    Checkpoint {
        checkpoint: Box<crate::runtime_client::checkpoint::RuntimeClientCheckpoint>,
    },
    Detached {},
    Subscribed {
        registration_id: String,
        target: AttachmentTarget,
        after_cursor: RuntimeClientCursor,
    },
    InboundAccepted {
        message_id: MessageId,
        inbound_sequence: crate::runtime::inbound::InboundSequence,
    },
    CancellationAccepted {
        attempt_id: crate::runtime::identity::AttemptId,
    },
    InteractionSettled {
        interaction: InteractionRef,
    },
    ConfigurationValueChunk {chunk:crate::local_runtime::configuration::effective_observation::EffectiveValueChunk},
    ConfigurationPage {page:crate::local_runtime::configuration::effective_observation::EffectivePage},
    ConfigurationReceipt {
        scope: String,
        version: u64,
    },
    SourcesWritten {
        revision: String,
        configuration_revision: Option<u64>,
    },
    ModelSet {
        settings_revision: u64,
    },
    EffectiveConfiguration {
        projection: Box<crate::local_runtime::configuration::effective_observation::BoundedEffectiveConfiguration>,
    },
    McpProbe {
        result: crate::local_runtime::mcp_probe::McpProbeResult,
    },
    SourcePage {
        page: crate::local_runtime::configuration::observation::SourceFieldPage,
    },
    SourceValueChunk {
        chunk: crate::local_runtime::configuration::observation::SourceValueChunk,
    },
    SourceSettings {
        projection: Box<crate::local_runtime::configuration::observation::BoundedSourceSettings>,
    },
    WorkflowLibrary {
        library: crate::local_runtime::workflow_authoring::WorkflowLibrary,
    },
    WorkflowSource {
        source: Box<crate::local_runtime::workflow_authoring::WorkflowSource>,
    },
    WorkflowModelChoices {
        choices: crate::local_runtime::workflow_authoring::WorkflowModelChoices,
    },
    WorkflowCheck {
        report: Box<crate::local_runtime::workflow_authoring::WorkflowCheckReport>,
    },
    SessionConfiguration {
        projection: Option<crate::local_runtime::configuration::observation::BoundedSessionApplication>,
    },
    SessionApplicationValueChunk { chunk: crate::local_runtime::configuration::observation::SessionApplicationChunk },
    SettingsValueChunk {
        chunk: crate::local_runtime::configuration::observation::SessionSettingsChunk,
    },
    Settings {
        projection: crate::local_runtime::configuration::observation::BoundedSessionSettings,
    },
}

#[allow(clippy::struct_excessive_bools)] // independent wire capability facts
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerCapabilities {
    pub upload_policy: crate::local_runtime::session::uploads::UploadPolicy,
    pub multi_session: bool,
    pub single_writable_controller: bool,
    pub headless_interactions: bool,
    /// Whether this connection holds transport-granted delivery access.
    /// Reporting it grants nothing; the native seam checks the capability.
    pub delivery_access: bool,
    pub experimental_methods: Vec<String>,
}

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self {
            upload_policy: crate::local_runtime::session::uploads::UPLOAD_POLICY,
            multi_session: true,
            single_writable_controller: true,
            headless_interactions: true,
            delivery_access: false,
            experimental_methods: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(remote = "Self", deny_unknown_fields)]
pub struct Notification {
    pub jsonrpc: JsonRpcVersion,
    #[serde(flatten)]
    pub notification: NotificationMethod,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum NotificationMethod {
    #[serde(rename = "auxiliary/segment")]
    Auxiliary {
        target: AttachmentTarget,
        registration_id: String,
        segment: crate::runtime::auxiliary::observer::AuxiliarySegment,
    },
    /// Retire this Session's execution generations through the inclusive native watermark.
    #[serde(rename = "session/ownershipRetired")]
    OwnershipRetired {
        session_id: SessionId,
        /// Exact decimal integer; independent of notification delivery sequence.
        retired_through: String,
    },
    #[serde(rename = "configuration/changed")]
    ConfigurationChanged {
        application: crate::local_runtime::configuration::observation::BoundedSessionApplication,
    },
    /// The latest automatic `rustx.toml` repair observation of one source
    /// changed (Issue #464).
    ///
    /// Source-scoped: it names a `SourceTarget`, never a Session, attachment
    /// or application scope, and it says nothing about runtime application,
    /// which `configuration/changed` reports separately. A connection observes
    /// the User source and each Workspace source it addressed, the least
    /// recently addressed of its 64 Workspaces forgotten first. Each admission
    /// of a source — including addressing a forgotten one again — announces
    /// its current level once, then every change of it, and a delivered
    /// receipt the finite ledger later forgets is followed by its absence
    /// once. `receipt: null` says the finite process-local ledger retains no
    /// receipt for the source — never recorded, withdrawn, or forgotten —
    /// never that the document is valid, which only a source read reports.
    /// It is a state, not a repair, and presents nothing. Delivery is
    /// level-triggered: after a gap, the authoritative
    /// `configuration/sourcesRead` carries the same receipts. A notice is not
    /// history: one receipt id is one repair occurrence, and a later notice
    /// may carry it again at a higher `sequence`.
    #[serde(rename = "configuration/sourceRepairChanged")]
    SourceRepairChanged {
        target: crate::local_runtime::configuration::settings::SourceTarget,
        receipt: Option<crate::local_runtime::configuration::repair::ConfigRepairReceipt>,
        /// The native repair-ledger sequence this level is current at. Within
        /// one App Server authority it orders this observation against every
        /// other observation of the same source, including the
        /// `repair_sequence` of a `configuration/sourcesRead`: the higher one
        /// is the newer, and two at one sequence agree.
        sequence: u64,
    },
    #[serde(rename = "session/event")]
    Event {
        registration_id: String,
        target: AttachmentTarget,
        cursor: RuntimeClientCursor,
        event: Box<crate::runtime_client::event::RuntimeClientEvent>,
    },
    /// Source preparation failed. This registration is fenced without consuming a Cursor;
    /// recovery requires an explicit checkpoint/subscription after the source is serviceable.
    #[serde(rename = "session/observationUnavailable")]
    ObservationUnavailable {
        registration_id: String,
        target: AttachmentTarget,
        #[schemars(length(max = 512))]
        message: String,
    },
    #[serde(rename = "session/resyncRequired")]
    ResyncRequired {
        registration_id: String,
        target: AttachmentTarget,
        reason: ResyncReason,
        checkpoint_required: bool,
        failed_at_cursor: Option<RuntimeClientCursor>,
        after_cursor: RuntimeClientCursor,
        earliest_serviceable: RuntimeClientCursor,
    },
    #[serde(rename = "session/closed")]
    Closed { target: AttachmentTarget },
    /// Invalidate the named Session summary. When `catalog_changed` is true,
    /// membership or recency order may have changed; reread `session/list` from native authority.
    ///
    /// It is an *invalidation*, not a value: it carries no metadata, makes no
    /// durability claim beyond the catalog commit that produced it, and is not
    /// canonical history, an Agent event, an Attempt event, or a Conversation
    /// runtime cursor. It is addressed by Session identity alone — no
    /// attachment target — because Session metadata belongs to the Session and
    /// not to whichever Conversation of it a client happens to display, and
    /// because observing metadata must never require holding a runtime
    /// attachment.
    ///
    /// Creation, copies and deletion invalidate catalog membership at visibility;
    /// asynchronous display-projection
    /// publication refreshes metadata: the Session's first ordinary root-lineage user message is
    /// committed canonically first, and its derived `preview` is committed to
    /// the catalog afterwards, so a client that read `session/summary` in
    /// between legitimately cached `preview: null`.
    #[serde(rename = "session/summaryInvalidated")]
    SummaryInvalidated {
        session_id: SessionId,
        /// Membership may have changed; reread the catalog and invalidate the named summary.
        /// False invalidates only the named Session's display metadata.
        catalog_changed: bool,
    },
}

/// Complete public wire surface used by schema and client generation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ProtocolMessage {
    Request(Box<Request>),
    Response(Response),
    Notification(Notification),
}

#[cfg(test)]
mod lifecycle_protocol_tests {
    use super::Method;

    #[test]
    fn t01_request_id_budget_counts_encoded_utf8_and_escaping() {
        use super::RequestId;
        for text in ["a".repeat(254), "\n".repeat(127), "é".repeat(127)] {
            assert!(RequestId::String(text.clone()).is_valid());
            assert!(!RequestId::String(format!("{text}a")).is_valid());
        }
        for integer in [-9_007_199_254_740_991, 9_007_199_254_740_991] {
            assert!(RequestId::Integer(integer).is_valid());
        }
        for integer in [-9_007_199_254_740_992, 9_007_199_254_740_992] {
            assert!(!RequestId::Integer(integer).is_valid());
        }
    }

    #[test]
    fn retired_controls_are_not_wire_aliases() {
        for method in [
            "background/status",
            "background/cancel",
            "subagent/status",
            "subagent/cancel",
            "subagent/transcript",
            "execution/status",
            "execution/steer",
        ] {
            assert!(
                serde_json::from_value::<Method>(
                    serde_json::json!({"method": method, "params": {}})
                )
                .is_err(),
                "retired method {method}"
            );
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Controller,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSource {
    Live,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResyncReason {
    ReplayUnavailable,
    RecordTooLarge,
}
