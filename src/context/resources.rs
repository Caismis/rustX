//! Bounded detached reads of immutable compaction inputs. No execution authority.

use std::io::Read as _;

use crate::durable::{ConversationStore, ConversationStoreError};
use crate::events::types::RuntimeEventEnvelope;
use crate::message::types::MessageBlock;
use crate::runtime::identity::MessageId;

pub use rustx_durable::context_resources::{
    JournalFrontier, LedgerFrontier, ResourceLimits, ResourcePage, ResourceReadCut,
    TrajectoryMessage, TrajectoryTool, bound, bounded_encoded_len,
};

/// An operation's read-only capabilities and cumulative materialization account.
/// Each SQL read is short-lived; retaining this value never retains a transaction.
pub struct CompactionResources<'a> {
    store: &'a dyn ConversationStore,
    cut: ResourceReadCut,
    limits: ResourceLimits,
    remaining_items: usize,
    remaining_bytes: usize,
}

impl<'a> CompactionResources<'a> {
    /// Freeze all source positions together before obtaining any working material.
    /// # Errors
    /// Rejects invalid bounds or unavailable durable authority.
    pub fn capture(
        store: &'a dyn ConversationStore,
        limits: ResourceLimits,
    ) -> Result<Self, ConversationStoreError> {
        limits.validate()?;
        Self::at_cut(store, store.compaction_resource_cut()?, limits)
    }

    /// Reattach bounded reads to an already captured immutable source cut.
    /// This does not advance any source coordinate or acquire a transaction.
    pub(crate) fn at_cut(
        store: &'a dyn ConversationStore,
        cut: ResourceReadCut,
        limits: ResourceLimits,
    ) -> Result<Self, ConversationStoreError> {
        limits.validate()?;
        if &cut.conversation_id != store.conversation_id() {
            return Err(ConversationStoreError::ConversationIdMismatch {
                stored: store.conversation_id().clone(),
                requested: cut.conversation_id,
            });
        }
        Ok(Self {
            store,
            cut,
            limits,
            remaining_items: limits.total_items,
            remaining_bytes: limits.total_bytes,
        })
    }

    #[must_use]
    pub fn cut(&self) -> &ResourceReadCut {
        &self.cut
    }

    fn allowance(&self) -> Result<ResourceLimits, ConversationStoreError> {
        if self.remaining_items == 0 || self.remaining_bytes == 0 {
            return Err(bound("compaction cumulative materialization exhausted"));
        }
        Ok(ResourceLimits {
            page_items: self.limits.page_items.min(self.remaining_items),
            page_bytes: self.limits.page_bytes.min(self.remaining_bytes),
            ..self.limits
        })
    }

    fn charge<T>(
        &mut self,
        page: ResourcePage<T>,
    ) -> Result<ResourcePage<T>, ConversationStoreError> {
        self.remaining_items = self
            .remaining_items
            .checked_sub(page.rows.len())
            .ok_or_else(|| bound("compaction cumulative item bound exceeded"))?;
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(page.bytes)
            .ok_or_else(|| bound("compaction cumulative byte bound exceeded"))?;
        Ok(page)
    }

    /// Reads canonical records strictly after a Ledger cursor through the frozen cut.
    /// # Errors
    /// Rejects oversized records before decoding and refuses cumulative over-materialization.
    pub fn messages(
        &mut self,
        after: LedgerFrontier,
    ) -> Result<ResourcePage<MessageBlock>, ConversationStoreError> {
        let page = self
            .store
            .compaction_message_page(&self.cut, after, None, self.allowance()?)?;
        self.charge(page)
    }

    /// Resolve an immutable integrity header, materializing the body only when requested.
    /// # Errors
    /// Missing, corrupt, foreign or oversized dependencies and exhausted budgets fail explicitly.
    pub fn dependency(
        &mut self,
        source: &crate::model::projected::CanonicalMessageRef,
        body_required: bool,
    ) -> Result<crate::context::checkpoint::CanonicalDependency, ConversationStoreError> {
        let allowance = self.allowance()?;
        let dependency =
            self.store
                .compaction_dependency(&self.cut, source, body_required, allowance)?;
        let bytes = bounded_encoded_len(&dependency, allowance.page_bytes)?;
        self.remaining_items = self
            .remaining_items
            .checked_sub(1)
            .ok_or_else(|| bound("dependency item budget exhausted"))?;
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(bytes)
            .ok_or_else(|| bound("dependency byte budget exhausted"))?;
        Ok(dependency)
    }

    /// Read detached typed trajectory and lifecycle information without observing future facts.
    /// # Errors
    /// Rejects unavailable source facts or exhausted materialization bounds.
    pub fn trajectory(
        &mut self,
        after: LedgerFrontier,
    ) -> Result<ResourcePage<TrajectoryMessage>, ConversationStoreError> {
        let page = self
            .store
            .compaction_trajectory_page(&self.cut, after, self.allowance()?)?;
        // Nested physical occurrences and facts consume the same finite item budget.
        let nested = page.rows.iter().try_fold(0_usize, |count, (_, row)| {
            row.tools.iter().try_fold(
                count + usize::from(row.committed.is_some()),
                |count, tool| {
                    count
                        .checked_add(1 + tool.facts.len())
                        .ok_or_else(|| bound("trajectory item count overflow"))
                },
            )
        })?;
        self.remaining_items = self
            .remaining_items
            .checked_sub(nested)
            .ok_or_else(|| bound("trajectory cumulative item bound exceeded"))?;
        self.charge(page)
    }

    /// Read only a settled Artifact explicitly referenced by this canonical
    /// fact at the frozen cut. No directory enumeration or mutation capability.
    /// # Errors
    /// Rejects foreign/unreferenced, unsettled, missing or oversized artifacts.
    pub fn artifact(
        &mut self,
        artifacts: &crate::tools::artifacts::ArtifactStore,
        source: &MessageId,
        id: &crate::runtime::identity::ArtifactId,
    ) -> Result<Vec<u8>, ConversationStoreError> {
        use crate::message::types::{AssistantContentBlock, UserContentBlock};
        use crate::model::projected::ModelMessageView;
        use crate::tools::types::ToolResultContent;
        if artifacts.conversation_id() != &self.cut.conversation_id {
            return Err(bound("foreign compaction Artifact capability"));
        }
        let source = self
            .originals(std::slice::from_ref(source))?
            .rows
            .pop()
            .ok_or_else(|| bound("missing Artifact source"))?
            .1;
        let input = crate::model::ModelInputMessage::Canonical(source);
        let authorized = match input.view() {
            ModelMessageView::User(content) => content.iter().any(|b| match b {
                UserContentBlock::Image(r) => &r.artifact_id == id,
                UserContentBlock::File(r) => &r.artifact_id == id,
                _ => false,
            }),
            ModelMessageView::Assistant(content) => content
                .iter()
                .any(|b| matches!(b,AssistantContentBlock::Image(r) if &r.artifact_id==id)),
            ModelMessageView::Tool(_, result) => {
                result.artifacts.iter().any(|r| &r.artifact_id == id)
                    || result.content.iter().any(|b| match b {
                        ToolResultContent::Image(r) => &r.artifact_id == id,
                        ToolResultContent::File(r) => &r.artifact_id == id,
                        _ => false,
                    })
            }
            ModelMessageView::RequestOnly(_) => false,
        };
        if !authorized {
            return Err(bound(
                "Artifact is not authorized by the frozen source fact",
            ));
        }
        let mut reader =
            crate::tools::artifacts::ArtifactStore::open_archive_reader(artifacts.root(), id)
                .map_err(|e| bound(&e.to_string()))?;
        let limit = self.allowance()?.record_bytes.min(self.remaining_bytes);
        if reader.len > limit as u64 {
            return Err(bound(
                "compaction Artifact record exceeds materialization budget",
            ));
        }
        let length = usize::try_from(reader.len).map_err(|_| bound("Artifact length overflow"))?;
        let mut bytes = Vec::with_capacity(length);
        std::io::Read::by_ref(&mut reader)
            .take(length as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| bound(&e.to_string()))?;
        if bytes.len() != length {
            return Err(bound("settled Artifact length changed"));
        }
        let mut verification = reader.verification();
        for (index, chunk) in bytes.chunks(65_536).enumerate() {
            let offset =
                u64::try_from(index * 65_536).map_err(|_| bound("Artifact offset overflow"))?;
            reader
                .verify_chunk(&mut verification, offset, chunk)
                .map_err(|error| bound(&error.to_string()))?;
        }
        reader
            .finish_verification(verification)
            .map_err(|error| bound(&error.to_string()))?;
        self.remaining_bytes -= bytes.len();
        self.remaining_items -= 1;
        Ok(bytes)
    }

    /// Resolve only these Original dependencies; no full-history hydration.
    /// # Errors
    /// Missing, future or oversized records fail explicitly.
    pub fn originals(
        &mut self,
        ids: &[MessageId],
    ) -> Result<ResourcePage<MessageBlock>, ConversationStoreError> {
        let allowance = self.allowance()?;
        if ids.len() > allowance.page_items {
            return Err(bound("compaction keyed read exceeds page item bound"));
        }
        let page = self.store.compaction_message_page(
            &self.cut,
            LedgerFrontier(0),
            Some(ids),
            allowance,
        )?;
        self.charge(page)
    }

    /// Historical checkpoints remain bounded and cannot cross this activation cut.
    /// # Errors
    /// Rejects missing, corrupt, future or oversized checkpoint dependencies.
    pub fn checkpoint(
        &mut self,
        id: &crate::runtime::identity::CheckpointId,
    ) -> Result<crate::durable::projection::ProjectionCheckpoint, ConversationStoreError> {
        let checkpoint = self
            .store
            .compaction_checkpoint_at(&self.cut, id, self.allowance()?)?;
        let bytes = bounded_encoded_len(&checkpoint, self.remaining_bytes)?;
        self.remaining_items = self
            .remaining_items
            .checked_sub(checkpoint.candidate.items.len())
            .ok_or_else(|| bound("checkpoint cumulative item bound exceeded"))?;
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(bytes)
            .ok_or_else(|| bound("checkpoint cumulative byte bound exceeded"))?;
        Ok(checkpoint)
    }
    /// Read exact frozen historical input only for requests actually started at this cut.
    /// # Errors
    /// Rejects absent, corrupt, future or oversized frozen requests.
    pub fn request(
        &mut self,
        id: &crate::runtime::identity::RequestId,
    ) -> Result<crate::model::RequestSnapshot, ConversationStoreError> {
        let snapshot = self
            .store
            .compaction_request(&self.cut, id, self.allowance()?)?;
        let bytes = bounded_encoded_len(&snapshot, self.remaining_bytes)?;
        self.remaining_items = self
            .remaining_items
            .checked_sub(snapshot.input.len())
            .ok_or_else(|| bound("frozen request cumulative item bound exceeded"))?;
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(bytes)
            .ok_or_else(|| bound("frozen request cumulative byte bound exceeded"))?;
        Ok(snapshot)
    }

    /// Reads real lifecycle facts at the frozen Journal cut, even when the live owner advances.
    /// # Errors
    /// Foreign/future cuts or materialization over budget fail explicitly.
    pub fn events(
        &mut self,
        after: JournalFrontier,
    ) -> Result<ResourcePage<RuntimeEventEnvelope>, ConversationStoreError> {
        let page = self
            .store
            .compaction_event_page(&self.cut, after, self.allowance()?)?;
        self.charge(page)
    }
}
