//! Rust authority for the App Server v16 envelope and method vocabulary.
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
pub const APP_SERVER_PROTOCOL_VERSION: u16 = 16;

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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

/// Bounded JSON carrier. The Session domain accepts decoded bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadBytes {
    pub name: String,
    pub data: String,
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
    #[serde(rename = "session/upload")]
    SessionUpload {
        target: AttachmentTarget,
        files: Vec<UploadBytes>,
    },
    #[serde(rename = "session/switchNode")]
    SessionSwitchNode {
        target: AttachmentTarget,
        node_id: SessionNodeId,
    },
    #[serde(rename = "session/trace")]
    Trace {
        target: AttachmentTarget,
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
        before: Option<crate::runtime_client::snapshot::RuntimeClientTranscriptCursor>,
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
    CompactContext { target: AttachmentTarget },
    #[serde(rename = "goal/control")]
    Goal {
        target: AttachmentTarget,
        control: crate::goal::GoalControl,
    },
    #[serde(rename = "background/status")]
    BackgroundStatus {
        target: AttachmentTarget,
        execution_id: crate::runtime::identity::ToolExecutionId,
    },
    #[serde(rename = "background/cancel")]
    BackgroundCancel {
        target: AttachmentTarget,
        execution_id: crate::runtime::identity::ToolExecutionId,
    },
    /// Bounded canonical history of an exact child owned by the addressed parent.
    #[serde(rename = "subagent/transcript")]
    SubagentTranscript {
        target: AttachmentTarget,
        subagent_id: crate::runtime::identity::SubagentId,
        before: Option<crate::runtime_client::snapshot::RuntimeClientTranscriptCursor>,
        limit: usize,
    },
    #[serde(rename = "subagent/status")]
    SubagentStatus {
        target: AttachmentTarget,
        subagent_id: crate::runtime::identity::SubagentId,
    },
    #[serde(rename = "subagent/cancel")]
    SubagentCancel {
        target: AttachmentTarget,
        subagent_id: crate::runtime::identity::SubagentId,
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
    #[serde(rename = "session/create")]
    SessionCreate { settings: SessionPersistentState },
    #[serde(rename = "session/read")]
    SessionRead { session_id: SessionId },
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
    ConfigurationAdoption {
        rejection: crate::local_runtime::configuration::application::AdoptionError,
    },
    ArchivePreparationFailed {
        reason: crate::session_archive::SessionArchivePrepareError,
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
    SessionArchive {
        download: super::archive_download::ArchiveDownloadDescriptor,
    },
    InboundMutation {
        outcome: crate::durable::inbox::PendingMutationOutcome,
    },
    ArtifactBytes {
        data: String,
    },
    SessionUploaded {
        files: Vec<crate::local_runtime::session::uploads::UploadedFile>,
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
    Goal {
        view: crate::goal::GoalView,
    },
    Background {
        execution: crate::runtime_client::snapshot::RuntimeClientBackgroundExecution,
    },
    Subagent {
        subagent: Box<crate::runtime_client::snapshot::RuntimeClientSubagent>,
    },
    WorkspaceDisposed {
        subagent: Box<crate::runtime_client::snapshot::RuntimeClientSubagent>,
        outcome: crate::runtime_client::types::RuntimeClientSubagentWorkspaceDisposalOutcome,
    },
    Initialized {
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
        result: crate::runtime_client::session_deletion::RuntimeClientSessionDeletionResult,
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
    pub multi_session: bool,
    pub single_writable_controller: bool,
    pub headless_interactions: bool,
    pub experimental_methods: Vec<String>,
}

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self {
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
}

/// Complete public wire surface used by schema and client generation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ProtocolMessage {
    Request(Box<Request>),
    Response(Response),
    Notification(Notification),
}
