//! Exhaustive classification of every request into the family that serves it.
//!
//! The connection owner admits every request once; this classification only
//! decides which handler the admitted request reaches. A new protocol method
//! must name its family here, so it can never silently bypass its owner.
use crate::app_server::protocol::{AttachmentTarget, Method};

/// The handler family of one typed request.
#[derive(Clone, Copy)]
pub(super) enum Family<'a> {
    /// Negotiation and server-level reads the connection owner answers itself.
    Connection,
    /// Transport-authorized delivery reads, registered before dispatch.
    Delivery,
    /// This connection's attachment relationships: attach, detach, status.
    Attachment,
    /// Session catalog reads and commits.
    Catalog,
    /// Bounded historical reads of a Session's committed lineage.
    History,
    /// Configuration sources, settings and adoption.
    Configuration,
    /// Durable Scheduler storage operations, independent of attachments.
    Scheduler,
    /// Operations on one exact attachment's live runtime.
    Runtime(&'a AttachmentTarget),
}

impl Family<'_> {
    #[allow(clippy::too_many_lines)] // Exhaustive public RPC routing keeps new variants from silently bypassing their owner.
    pub(super) fn of(method: &Method) -> Family<'_> {
        match method {
            Method::ConfigurationGet { target, .. }
            | Method::ConfigurationValueRead { target, .. }
            | Method::ConfigurationPage { target, .. }
            | Method::ModelGet { target, .. }
            | Method::ModelCatalog { target, .. }
            | Method::ModelSet { target, .. }
            | Method::ModelSelect { target, .. }
            | Method::Capability { target, .. }
            | Method::AgentArtifactRead { target, .. }
            | Method::ArtifactRead { target, .. }
            | Method::SessionUploadPrepare { target, .. }
            | Method::SessionUploadStatus { target, .. }
            | Method::Trace { target, .. }
            | Method::Projections { target, .. }
            | Method::ProjectionDetail { target, .. }
            | Method::TraceDetail { target, .. }
            | Method::TraceLocateTool { target, .. }
            | Method::Transcript { target, .. }
            | Method::ConversationTurns { target, .. }
            | Method::Goal { target, .. }
            | Method::JobStatus { target, .. }
            | Method::JobList { target, .. }
            | Method::JobWait { target, .. }
            | Method::JobCancel { target, .. }
            | Method::AgentStatus { target, .. }
            | Method::AgentList { target, .. }
            | Method::AgentObserve { target, .. }
            | Method::AgentStatistics { target, .. }
            | Method::AgentSendMessage { target, .. }
            | Method::AgentWait { target, .. }
            | Method::AgentInterrupt { target, .. }
            | Method::AgentTrace { target, .. }
            | Method::AgentTraceDetail { target, .. }
            | Method::AgentTraceLocateTool { target, .. }
            | Method::AgentTranscript { target, .. }
            | Method::AgentTurns { target, .. }
            | Method::CompactContext { target, .. }
            | Method::SessionBoundaries { target, .. }
            | Method::SessionCheckpoint { target, .. }
            | Method::ConversationContentRead { target, .. }
            | Method::ConversationCheckpointPage { target, .. }
            | Method::ConversationCheckpointValueRead { target, .. }
            | Method::SessionSubscribe { target, .. }
            | Method::TurnStart { target, .. }
            | Method::TurnSteer { target, .. }
            | Method::InboundEdit { target, .. }
            | Method::InboundRemove { target, .. }
            | Method::TurnCancel { target, .. }
            | Method::InteractionRespond { target, .. }
            | Method::InteractionCancel { target, .. } => Family::Runtime(target),
            Method::WorkflowScheduledRecovery { .. }
            | Method::WorkflowScheduledCancel { .. }
            | Method::WorkflowScheduledReconcile { .. }
            | Method::ScheduleList { .. }
            | Method::ScheduleGet { .. }
            | Method::ScheduleCommand { .. }
            | Method::ScheduleRecoverStorage { .. } => Family::Scheduler,
            Method::Initialize(_)
            | Method::ServerInfo { .. }
            | Method::ServerDiagnostics { .. }
            | Method::AgentObserveCancel { .. }
            | Method::DeliveryCancel { .. } => Family::Connection,
            Method::AuxiliaryClassify { .. }
            | Method::AuxiliaryBtw { .. }
            | Method::AuxiliaryStatus { .. }
            | Method::AuxiliaryResult { .. }
            | Method::AuxiliaryCancel { .. }
            | Method::AuxiliaryDiscard { .. }
            | Method::AuxiliarySubmissionLookup { .. }
            | Method::AgentDeliveryRead { .. }
            | Method::AgentDeliveryLocate { .. }
            | Method::DeliveryRead { .. }
            | Method::DeliveryLocate { .. } => Family::Delivery,
            Method::SessionAttach { .. }
            | Method::SessionDetach { .. }
            | Method::SessionAttachmentStatus { .. } => Family::Attachment,
            Method::SessionExportPrepare { .. }
            | Method::SessionList { .. }
            | Method::SessionCreate { .. }
            | Method::SessionRead { .. }
            | Method::SessionSummary { .. }
            | Method::SessionName { .. }
            | Method::SessionNode { .. }
            | Method::SessionNodeValueRead { .. }
            | Method::SessionTree { .. }
            | Method::SessionFork { .. }
            | Method::SessionForkProjection { .. }
            | Method::SessionBranch { .. }
            | Method::SessionSwitchNode { .. }
            | Method::SessionDeletePreview { .. }
            | Method::SessionDelete { .. }
            | Method::SessionRecoverDeletion { .. } => Family::Catalog,
            Method::SessionHistory { .. }
            | Method::SessionNodeOriginHistory { .. }
            | Method::SessionTraceHistory { .. }
            | Method::SessionTraceHistoryDetail { .. }
            | Method::SessionTracePage { .. }
            | Method::SessionTraceValueRead { .. }
            | Method::SessionTraceContentRead { .. }
            | Method::SessionHistoryContentRead { .. }
            | Method::SessionHistoryPage { .. }
            | Method::SessionHistoryValueRead { .. }
            | Method::SessionStatistics { .. }
            | Method::SessionEditorRead { .. } => Family::History,
            Method::SourcesRead { .. }
            | Method::McpProbe { .. }
            | Method::SourcesPage { .. }
            | Method::SourcesValueRead { .. }
            | Method::SourcesWrite { .. }
            | Method::WorkflowList { .. }
            | Method::WorkflowRead { .. }
            | Method::WorkflowModelChoices { .. }
            | Method::WorkflowCheck { .. }
            | Method::SettingsRead { .. }
            | Method::SettingsValueRead { .. }
            | Method::ConfigurationReconcile { .. }
            | Method::SessionApplicationValueRead { .. }
            | Method::SessionConfiguration { .. }
            | Method::AdoptConfiguration { .. } => Family::Configuration,
        }
    }
}
