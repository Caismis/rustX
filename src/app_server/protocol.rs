//! Rust authority for the App Server v5 envelope and method vocabulary.
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
pub const APP_SERVER_PROTOCOL_VERSION: u16 = 5;

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
    #[serde(rename = "settings/defaults")]
    DefaultsRead {
        target: AttachmentTarget,
        scope: crate::runtime_client::settings::DefaultScope,
    },
    #[serde(rename = "settings/saveDefault")]
    DefaultSave {
        target: AttachmentTarget,
        scope: crate::runtime_client::settings::DefaultScope,
        expected_revision: String,
        setting: crate::runtime_client::settings::DefaultTarget,
    },
    #[serde(rename = "session/unload")]
    SessionUnload { target: AttachmentTarget },
    #[serde(rename = "session/trace")]
    Trace {
        target: AttachmentTarget,
        before: Option<crate::runtime_client::trace::TraceCursor>,
        limit: usize,
    },
    #[serde(rename = "session/transcript")]
    Transcript {
        target: AttachmentTarget,
        before: Option<crate::runtime_client::snapshot::RuntimeClientTranscriptCursor>,
        limit: usize,
    },
    #[serde(rename = "settings/model")]
    ModelGet { target: AttachmentTarget },
    #[serde(rename = "settings/models")]
    ModelCatalog { target: AttachmentTarget },
    #[serde(rename = "settings/setModel")]
    ModelSet {
        target: AttachmentTarget,
        config: Box<crate::model::session::SessionModelConfig>,
    },
    #[serde(rename = "settings/setApprovalMode")]
    ApprovalModeSet {
        target: AttachmentTarget,
        mode: crate::runtime::types::ApprovalMode,
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
    },
    #[serde(rename = "session/branch")]
    SessionBranch {
        session_id: SessionId,
        node_id: SessionNodeId,
        surface_revision: crate::conversation::SurfaceRevision,
        boundary: MessageId,
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
    #[serde(rename = "settings/selectModel")]
    SelectModel {
        session_id: SessionId,
        expected_revision: u64,
        selection: Option<crate::model::session::SessionModelConfig>,
    },
    #[serde(rename = "settings/sourcesRead")]
    SourcesRead { session_id: SessionId },
    #[serde(rename = "settings/sourcesWrite")]
    SourcesWrite {
        session_id: SessionId,
        expected_revision: String,
        mutation: crate::local_runtime::configuration::settings::SourceMutation,
    },
    #[serde(rename = "settings/read")]
    SettingsRead { session_id: SessionId },
    #[serde(rename = "settings/replace")]
    SettingsReplace {
        session_id: SessionId,
        expected_revision: u64,
        settings: SessionPersistentState,
    },
    #[serde(rename = "resources/reload")]
    ResourcesReload { target: AttachmentTarget },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ErrorData {
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
    UntrustedWorkspace,
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
    Defaults {
        document: crate::runtime_client::settings::DefaultDocument,
    },
    DefaultSaved {
        result: crate::runtime_client::settings::SaveDefaultResult,
    },
    Unloaded {},
    Model {
        model: Box<crate::model::session::SessionModelView>,
    },
    Models {
        catalog: crate::model::catalog::ModelCatalogView,
    },
    ApprovalMode {
        effective_approval_mode: crate::runtime::types::ApprovalMode,
        pending_approval_mode: Option<crate::runtime::types::ApprovalMode>,
        revision: u64,
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
    Sessions {
        /// Point-in-time runtime observation for exactly this bounded catalog page.
        /// This never loads or attaches a Session.
        residencies: std::collections::BTreeMap<
            crate::local_runtime::session::SessionId,
            crate::local_runtime::session_runtime_manager::ResidencyState,
        >,
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
    SourceSettings {
        projection: Box<crate::local_runtime::configuration::settings::SourceSettings>,
        session_revision: u64,
        session_selection: Option<crate::model::session::SessionModelConfig>,
    },
    Settings {
        /// Current native project source trust; unresolved is never trusted.
        /// Loaded resources retain their admitted generation independently.
        project_trusted: Option<bool>,
        revision: u64,
        settings: SessionPersistentState,
    },
    SettingsReplaced {
        revision: u64,
    },
    ResourcesReloaded {
        resource_revision: u64,
        capability_revision: crate::runtime::identity::CapabilityRevision,
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
