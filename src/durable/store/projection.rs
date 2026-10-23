//! Store contract for Context projection, compaction resources, and lineage read cuts.

use crate::conversation::SurfaceRevision;
use crate::events::types::RuntimeEventEnvelope;
use crate::message::types::MessageBlock;
use crate::runtime::identity::MessageId;

use crate::durable::ConversationStoreError;

/// The Context projection, compaction resource, and lineage read-cut domain of
/// the conversation Store. Checkpoint content is immutable; the projection head
/// is the only mutable selection, and lineage cuts capture (R, C) authority in
/// one native snapshot.
#[allow(clippy::missing_errors_doc)]
pub trait ProjectionStore: Send + Sync + 'static {
    /// Freeze the immutable compaction source domains in one short read transaction.
    fn compaction_resource_cut(
        &self,
    ) -> Result<crate::context::resources::ResourceReadCut, ConversationStoreError>;
    /// Bounded detached canonical reads, through the Resource Read Cut.
    fn compaction_message_page(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        after: crate::context::resources::LedgerFrontier,
        ids: Option<&[MessageId]>,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::context::resources::ResourcePage<MessageBlock>, ConversationStoreError>;
    /// Keyed immutable provenance integrity, with body only when explicitly required.
    fn compaction_dependency(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        source: &crate::model::projected::CanonicalMessageRef,
        body_required: bool,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::context::checkpoint::CanonicalDependency, ConversationStoreError>;
    /// Detached typed trajectory metadata and exact physical tool occurrences.
    fn compaction_trajectory_page(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        after: crate::context::resources::LedgerFrontier,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<
        crate::context::resources::ResourcePage<crate::context::resources::TrajectoryMessage>,
        ConversationStoreError,
    >;
    /// Bounded real Journal facts, through the Resource Read Cut.
    fn compaction_event_page(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        after: crate::context::resources::JournalFrontier,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::context::resources::ResourcePage<RuntimeEventEnvelope>, ConversationStoreError>;
    /// Only copied response provenance intersecting exact requested members.
    fn inherited_responses_for(
        &self,
        messages: &[MessageId],
    ) -> Result<Vec<crate::durable::response::CompletedResponseProvenance>, ConversationStoreError>;
    /// Captures lineage authority (R, C) in one native snapshot: revision R owns
    /// structure, invocation-time read cut C owns temporal provenance. The same
    /// R at a newer C may observe newer outcomes; captured (R, C) is immutable.
    /// Select a historical activation independently of its canonical Ledger frontier.
    fn read_projection_lineage_cut(
        &self,
        ledger: crate::context::resources::LedgerFrontier,
        activation: crate::context::checkpoint::ProjectionActivationId,
    ) -> Result<crate::durable::reading::LineageReadCut, ConversationStoreError>;
    fn read_lineage_cut(
        &self,
        revision: SurfaceRevision,
    ) -> Result<crate::durable::reading::LineageReadCut, ConversationStoreError>;
    fn load_inherited_turns(
        &self,
    ) -> Result<Vec<crate::durable::reading::TurnReadingProvenance>, ConversationStoreError>;
    fn inherited_turns_for(
        &self,
        messages: &[MessageId],
    ) -> Result<Vec<crate::durable::reading::TurnReadingProvenance>, ConversationStoreError>;
    /// Read the current independent projection head; a dangling head fails closed.
    fn projected_history(
        &self,
    ) -> Result<crate::durable::projection::HistoricalModelInput, ConversationStoreError>;
    /// One committed projection and its exact resource coordinates from the same snapshot.
    fn auxiliary_history(
        &self,
    ) -> Result<
        (
            crate::durable::projection::HistoricalModelInput,
            crate::context::resources::ResourceReadCut,
        ),
        ConversationStoreError,
    >;
    fn projection_at_journal_cut(
        &self,
        through: u64,
    ) -> Result<Option<crate::durable::projection::ProjectionCheckpoint>, ConversationStoreError>;
    /// Read one immutable checkpoint at the frozen resource cut with early byte admission.
    fn compaction_checkpoint_at(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        id: &crate::runtime::identity::CheckpointId,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::durable::projection::ProjectionCheckpoint, ConversationStoreError>;
    fn compaction_request(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        request: &crate::runtime::identity::RequestId,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::model::RequestSnapshot, ConversationStoreError>;
    fn projection_head(
        &self,
    ) -> Result<Option<crate::durable::projection::ProjectionCheckpoint>, ConversationStoreError>;
    fn projection_checkpoint(
        &self,
        id: &crate::runtime::identity::CheckpointId,
    ) -> Result<crate::durable::projection::ProjectionCheckpoint, ConversationStoreError>;
    fn projection_timeline(
        &self,
        after: crate::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<Vec<crate::durable::projection::ProjectionCheckpoint>, ConversationStoreError>;
    /// Decide published-cut eligibility before immutable checkpoint hydration.
    fn projection_timeline_at_journal_cut(
        &self,
        through: u64,
        after: crate::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<crate::durable::projection::ProjectionTimelinePage, ConversationStoreError>;
    fn projection_checkpoint_at_journal_cut(
        &self,
        through: u64,
        id: &crate::runtime::identity::CheckpointId,
    ) -> Result<crate::durable::projection::ProjectionCheckpoint, ConversationStoreError>;
    fn projection_owner(
        &self,
        attempt: Option<&crate::runtime::identity::AttemptId>,
    ) -> Result<crate::durable::projection::ProjectionOwner, ConversationStoreError>;
    fn projection_receipt(
        &self,
        operation: &crate::runtime::identity::CompactionOperationId,
        fingerprint: &str,
    ) -> Result<Option<crate::durable::projection::ProjectionCommitReceipt>, ConversationStoreError>;
    fn commit_projection(
        &self,
        input: crate::durable::projection::ProjectionCommitInput,
    ) -> Result<crate::durable::projection::ProjectionCommitReceipt, ConversationStoreError>;
}
