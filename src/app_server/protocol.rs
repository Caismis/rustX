//! Rust authority for the App Server v37 envelope and method vocabulary.
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
pub const APP_SERVER_PROTOCOL_VERSION: u16 = 37;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum JsonRpcVersion {
    #[serde(rename = "2.0")]
    V2,
}

/// Integer request IDs use the JavaScript safe-integer range at admission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Integer(i64),
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
    #[serde(rename = "artifact/read")]
    ArtifactRead {
        target: AttachmentTarget,
        artifact_id: crate::runtime::identity::ArtifactId,
    },
    #[serde(rename = "session/uploadPrepare")]
    SessionUploadPrepare {
        target: AttachmentTarget,
        operation_id: String,
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
    #[serde(rename = "session/trace")]
    Trace {
        target: AttachmentTarget,
        #[serde(default)]
        records: Vec<crate::runtime_client::trace::TraceCursor>,
        before: Option<crate::runtime_client::trace::TraceCursor>,
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
    #[serde(rename = "session/transcript")]
    Transcript {
        target: AttachmentTarget,
        at: crate::durable::reading::ConversationWindowAt,
        #[schemars(range(min = 1, max = 64))]
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
    #[serde(rename = "resources/read")]
    Capability { target: AttachmentTarget },
    #[serde(rename = "context/compact")]
    CompactContext {
        target: AttachmentTarget,
        request_id: crate::runtime::identity::ManualCompactionRequestId,
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
    #[serde(rename = "agent/sendMessage")]
    AgentSendMessage {
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
    #[serde(rename = "agent/transcript")]
    AgentTranscript {
        target: AttachmentTarget,
        agent_id: crate::runtime::identity::AgentId,
        before: Option<crate::runtime_client::snapshot::RuntimeClientTranscriptCursor>,
        limit: usize,
    },
    #[serde(rename = "subagent/disposeWorkspace")]
    SubagentDispose {
        target: AttachmentTarget,
        subagent_id: crate::runtime::identity::SubagentId,
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
        before: Option<crate::runtime_client::trace::TraceCursor>,
        limit: usize,
    },
    #[serde(rename = "session/traceHistoryDetail")]
    SessionTraceHistoryDetail {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        record_id: String,
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
    #[serde(rename = "session/fork")]
    SessionFork {
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
        surface_revision: crate::conversation::SurfaceRevision,
        boundary: Option<MessageId>,
        side: crate::local_runtime::session::LineageSide,
    },
    #[serde(rename = "session/branch")]
    SessionBranch {
        session_id: SessionId,
        node_id: SessionNodeId,
        surface_revision: crate::conversation::SurfaceRevision,
        boundary: MessageId,
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
    #[serde(rename = "session/detach")]
    SessionDetach { target: AttachmentTarget },
    #[serde(rename = "session/snapshot")]
    SessionSnapshot {
        target: AttachmentTarget,
        /// Bounded loaded Trace identities to repair at the same snapshot cut.
        #[serde(default)]
        #[schemars(length(max = 512))]
        trace_records: Vec<crate::runtime_client::trace::TraceCursor>,
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
    #[serde(rename = "mcp/connect")]
    McpConnect {
        target: crate::local_runtime::configuration::settings::SourceTarget,
        id: crate::runtime::identity::McpServerId,
        expected_revision: String,
        refresh: bool,
    },
    #[serde(rename = "mcp/status")]
    McpStatus {
        target: crate::local_runtime::configuration::settings::SourceTarget,
    },
    #[serde(rename = "mcp/disconnect")]
    McpDisconnect {
        target: crate::local_runtime::configuration::settings::SourceTarget,
        id: crate::runtime::identity::McpServerId,
    },
    #[serde(rename = "configuration/sourcesRead")]
    SourcesRead {
        target: crate::local_runtime::configuration::settings::SourceTarget,
    },
    #[serde(rename = "configuration/sourceWrite")]
    SourcesWrite {
        target: crate::local_runtime::configuration::settings::SourceTarget,
        expected_revision: String,
        mutation: crate::local_runtime::configuration::settings::SourceMutation,
    },
    #[serde(rename = "session/settings")]
    SettingsRead { session_id: SessionId },
    #[serde(rename = "configuration/reconcile")]
    ConfigurationReconcile {
        target: crate::local_runtime::configuration::settings::SourceTarget,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ErrorData {
    SessionFileRead {
        reason: crate::tools::session_files::SessionFileReadFailure,
    },
    ConfigurationAdoption {
        rejection: crate::local_runtime::configuration::application::AdoptionError,
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
        scope: crate::local_runtime::configuration::settings::SourceScope,
        expected: String,
        actual: String,
    },
    StaleSettings {
        expected: u64,
        actual: u64,
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
    SessionFileBytes {
        file: crate::tools::session_files::SessionFileReference,
        data: String,
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
    Model {
        model: Box<crate::model::session::SessionModelView>,
    },
    Models {
        catalog: crate::model::catalog::ModelCatalogView,
    },

    Capabilities {
        capabilities: crate::runtime_client::snapshot::CapabilityView,
    },
    Context {
        context: crate::runtime_client::snapshot::RuntimeClientContextView,
    },
    Trace {
        page: crate::runtime_client::trace::TracePage,
    },
    TraceDetail {
        /// Absent when the identity names no record at the read cut. Boxed
        /// because inspection detail is by far the largest result: keeping it
        /// off the shared enum keeps every other response cheap to move.
        detail: Option<Box<crate::runtime_client::trace::TraceDetail>>,
    },
    Transcript {
        page: crate::runtime_client::snapshot::RuntimeClientTranscriptPage,
    },
    SessionHistory {
        conversation_id: ConversationId,
        window: crate::runtime_client::snapshot::ConversationWindow,
    },
    SessionStatistics {
        conversation_id: ConversationId,
        statistics: crate::runtime_client::response::ConversationStatistics,
        occupancy: Option<crate::context::occupancy::ContextOccupancy>,
    },
    SessionTraceHistory {
        conversation_id: ConversationId,
        page: crate::runtime_client::trace::TracePage,
    },
    SessionTraceHistoryDetail {
        conversation_id: ConversationId,
        detail: Option<Box<crate::runtime_client::trace::TraceDetail>>,
    },
    TranscriptWindow {
        window: crate::runtime_client::snapshot::ConversationWindow,
    },
    ConversationTurns {
        page: crate::durable::reading::ConversationTurnPage,
    },
    Goal {
        view: crate::goal::GoalView,
    },
    Job {
        job: crate::runtime_client::snapshot::RuntimeClientJob,
    },
    Jobs {
        jobs: Vec<crate::runtime_client::snapshot::RuntimeClientJob>,
        returned: usize,
        matched: usize,
        limit: usize,
        truncated: bool,
    },
    Agent {
        agent: Box<crate::runtime_client::snapshot::RuntimeClientAgent>,
    },
    Agents {
        agents: Vec<crate::runtime_client::snapshot::RuntimeClientAgent>,
        returned: usize,
        matched: usize,
        limit: usize,
        truncated: bool,
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
        agent: Box<crate::runtime_client::snapshot::RuntimeClientAgent>,
    },
    WorkspaceDisposed {
        subagent_id: crate::runtime::identity::SubagentId,
        workspace: crate::runtime_client::snapshot::RuntimeClientAgentWorkspace,
        outcome: crate::runtime_client::types::RuntimeClientAgentWorkspaceDisposalOutcome,
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
    SessionTransition {
        session: crate::local_runtime::session::SessionSnapshot,
        editor_content: Option<Vec<crate::local_runtime::session::uploads::UserInputBlock>>,
        durability_diagnostic: Option<String>,
    },
    SessionSummary {
        summary: crate::local_runtime::session::SessionSummary,
    },
    Sessions {
        sessions: Vec<crate::local_runtime::session::SessionSummary>,
        next_offset: Option<usize>,
    },
    Tree {
        nodes: Vec<crate::local_runtime::session::SessionNode>,
        next_offset: Option<usize>,
    },
    Boundaries {
        /// The exact committed head the page was selected against.
        surface_revision: crate::conversation::SurfaceRevision,
        boundaries: Vec<crate::local_runtime::session::SessionUserMessageBoundary>,
        next_offset: Option<usize>,
    },
    Deletion {
        result: crate::app_server::session_deletion::SessionDeletionResult,
    },
    Attached {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        configuration:
            Option<crate::local_runtime::configuration::application::ConfigurationApplication>,
        target: AttachmentTarget,
        snapshot: Box<crate::runtime_client::snapshot::RuntimeClientSnapshot>,
        cursor: RuntimeClientCursor,
    },
    Snapshot {
        snapshot: Box<crate::runtime_client::snapshot::RuntimeClientSnapshot>,
        cursor: RuntimeClientCursor,
    },
    Detached {},
    Subscribed {
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
    ConfigurationApplication {
        application: crate::local_runtime::configuration::application::ConfigurationApplication,
    },
    EffectiveConfiguration {
        projection: Box<crate::local_runtime::configuration::settings::EffectiveConfiguration>,
    },
    McpConnections {
        connections:
            Vec<crate::local_runtime::configuration::mcp_connections::McpConnectionSnapshot>,
    },
    SourceSettings {
        projection: Box<crate::local_runtime::configuration::settings::SourceSettings>,
    },
    SessionConfiguration {
        application:
            Option<crate::local_runtime::configuration::application::ConfigurationApplication>,
    },
    Settings {
        /// Exact durable Session-intent revision for subsequent CAS controls.
        /// Loaded configuration has its own immutable generation.
        revision: u64,
        settings: SessionPersistentState,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerCapabilities {
    pub upload_policy: crate::local_runtime::session::uploads::UploadPolicy,
    pub multi_session: bool,
    pub single_writable_controller: bool,
    pub headless_interactions: bool,
    pub experimental_methods: Vec<String>,
}

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self {
            upload_policy: crate::local_runtime::session::uploads::UPLOAD_POLICY,
            multi_session: true,
            single_writable_controller: true,
            headless_interactions: true,
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
    /// Retire this Session's execution generations through the inclusive native watermark.
    #[serde(rename = "session/ownershipRetired")]
    OwnershipRetired {
        session_id: SessionId,
        /// Exact decimal integer; independent of notification delivery sequence.
        retired_through: String,
    },
    #[serde(rename = "configuration/changed")]
    ConfigurationChanged {
        application: crate::local_runtime::configuration::application::ConfigurationApplication,
    },
    #[serde(rename = "session/event")]
    Event {
        target: AttachmentTarget,
        cursor: RuntimeClientCursor,
        event: Box<crate::runtime_client::event::RuntimeClientEvent>,
    },
    #[serde(rename = "session/resyncRequired")]
    ResyncRequired {
        target: AttachmentTarget,
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
