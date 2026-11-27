//! Store contract for Context projection, compaction resources, and lineage read cuts.

use rustx_conversation::SurfaceRevision;
use rustx_types::events::types::RuntimeEventEnvelope;
use rustx_types::message::types::MessageBlock;
use rustx_types::runtime::identity::MessageId;

use crate::ConversationStoreError;

/// The Context projection, compaction resource, and lineage read-cut domain of
/// the conversation Store. Checkpoint content is immutable; the projection head
/// is the only mutable selection, and lineage cuts capture (R, C) authority in
/// one native snapshot.
#[allow(clippy::missing_errors_doc)]
pub trait ProjectionStore: Send + Sync + 'static {
    /// Freeze the immutable compaction source domains in one short read transaction.
    fn compaction_resource_cut(
        &self,
    ) -> Result<rustx_types::context::resources::ResourceReadCut, ConversationStoreError>;
    /// Bounded detached canonical reads, through the Resource Read Cut.
    fn compaction_message_page(
        &self,
        cut: &rustx_types::context::resources::ResourceReadCut,
        after: rustx_types::context::resources::LedgerFrontier,
        ids: Option<&[MessageId]>,
        limits: crate::context_resources::ResourceLimits,
    ) -> Result<crate::context_resources::ResourcePage<MessageBlock>, ConversationStoreError>;
    /// Keyed immutable provenance integrity, with body only when explicitly required.
    fn compaction_dependency(
        &self,
        cut: &rustx_types::context::resources::ResourceReadCut,
        source: &rustx_types::model::projected::CanonicalMessageRef,
        body_required: bool,
        limits: crate::context_resources::ResourceLimits,
    ) -> Result<rustx_types::context::checkpoint::CanonicalDependency, ConversationStoreError>;
    /// Detached typed trajectory metadata and exact physical tool occurrences.
    fn compaction_trajectory_page(
        &self,
        cut: &rustx_types::context::resources::ResourceReadCut,
        after: rustx_types::context::resources::LedgerFrontier,
        limits: crate::context_resources::ResourceLimits,
    ) -> Result<
        crate::context_resources::ResourcePage<crate::context_resources::TrajectoryMessage>,
        ConversationStoreError,
    >;
    /// Bounded real Journal facts, through the Resource Read Cut.
    fn compaction_event_page(
        &self,
        cut: &rustx_types::context::resources::ResourceReadCut,
        after: rustx_types::context::resources::JournalFrontier,
        limits: crate::context_resources::ResourceLimits,
    ) -> Result<crate::context_resources::ResourcePage<RuntimeEventEnvelope>, ConversationStoreError>;
    /// Only copied response provenance intersecting exact requested members.
    fn inherited_responses_for(
        &self,
        messages: &[MessageId],
    ) -> Result<Vec<crate::response::CompletedResponseProvenance>, ConversationStoreError>;
    /// Captures lineage authority (R, C) in one native snapshot: revision R owns
    /// structure, invocation-time read cut C owns temporal provenance. The same
    /// R at a newer C may observe newer outcomes; captured (R, C) is immutable.
    /// Select a historical activation independently of its canonical Ledger frontier.
    fn read_projection_lineage_cut(
        &self,
        ledger: rustx_types::context::resources::LedgerFrontier,
        activation: rustx_types::context::checkpoint::ProjectionActivationId,
    ) -> Result<crate::reading::LineageReadCut, ConversationStoreError>;
    fn read_lineage_cut(
        &self,
        revision: SurfaceRevision,
    ) -> Result<crate::reading::LineageReadCut, ConversationStoreError>;
    fn load_inherited_turns(
        &self,
    ) -> Result<Vec<crate::reading::TurnReadingProvenance>, ConversationStoreError>;
    fn inherited_turns_for(
        &self,
        messages: &[MessageId],
    ) -> Result<Vec<crate::reading::TurnReadingProvenance>, ConversationStoreError>;
    /// Read the current independent projection head; a dangling head fails closed.
    fn projected_history(
        &self,
    ) -> Result<crate::projection::HistoricalModelInput, ConversationStoreError>;
    /// One committed projection and its exact resource coordinates from the same snapshot.
    fn auxiliary_history(
        &self,
    ) -> Result<
        (
            crate::projection::HistoricalModelInput,
            rustx_types::context::resources::ResourceReadCut,
        ),
        ConversationStoreError,
    >;
    fn projection_at_journal_cut(
        &self,
        through: u64,
    ) -> Result<Option<crate::projection::ProjectionCheckpoint>, ConversationStoreError>;
    /// Read one immutable checkpoint at the frozen resource cut with early byte admission.
    fn compaction_checkpoint_at(
        &self,
        cut: &rustx_types::context::resources::ResourceReadCut,
        id: &rustx_types::runtime::identity::CheckpointId,
        limits: crate::context_resources::ResourceLimits,
    ) -> Result<crate::projection::ProjectionCheckpoint, ConversationStoreError>;
    fn compaction_request(
        &self,
        cut: &rustx_types::context::resources::ResourceReadCut,
        request: &rustx_types::runtime::identity::RequestId,
        limits: crate::context_resources::ResourceLimits,
    ) -> Result<crate::model::snapshot::RequestSnapshot, ConversationStoreError>;
    fn projection_head(
        &self,
    ) -> Result<Option<crate::projection::ProjectionCheckpoint>, ConversationStoreError>;
    fn projection_checkpoint(
        &self,
        id: &rustx_types::runtime::identity::CheckpointId,
    ) -> Result<crate::projection::ProjectionCheckpoint, ConversationStoreError>;
    fn projection_timeline(
        &self,
        after: rustx_types::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<Vec<crate::projection::ProjectionCheckpoint>, ConversationStoreError>;
    /// Decide published-cut eligibility before immutable checkpoint hydration.
    fn projection_timeline_at_journal_cut(
        &self,
        through: u64,
        after: rustx_types::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<crate::projection::ProjectionTimelinePage, ConversationStoreError>;
    fn projection_checkpoint_at_journal_cut(
        &self,
        through: u64,
        id: &rustx_types::runtime::identity::CheckpointId,
    ) -> Result<crate::projection::ProjectionCheckpoint, ConversationStoreError>;
    fn projection_owner(
        &self,
        attempt: Option<&rustx_types::runtime::identity::AttemptId>,
    ) -> Result<crate::projection::ProjectionOwner, ConversationStoreError>;
    fn projection_receipt(
        &self,
        operation: &rustx_types::runtime::identity::CompactionOperationId,
        fingerprint: &str,
    ) -> Result<Option<crate::projection::ProjectionCommitReceipt>, ConversationStoreError>;
    /// Indexed, bounded restart evidence for one known Compaction operation.
    /// A completion without an intact matching receipt is a durable error.
    fn compaction_restart_evidence(
        &self,
        operation: &rustx_types::runtime::identity::CompactionOperationId,
    ) -> Result<crate::projection::CompactionRestartEvidence, ConversationStoreError>;
    fn commit_projection(
        &self,
        input: crate::projection::ProjectionCommitInput,
    ) -> Result<crate::projection::ProjectionCommitReceipt, ConversationStoreError>;
}
