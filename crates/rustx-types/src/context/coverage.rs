//! Bounded capture coordinates shared by generation and the durable store.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::context::resources::{
    COMPACTION_RESOURCE_BYTES_MAX, COMPACTION_RESOURCE_ITEMS_MAX, ResourceReadCut,
};
use crate::model::projected::CheckpointSourceRef;
use crate::runtime::identity::ConversationId;

/// The immutable origin of a continuous logical projection prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CoverageRoot {
    /// Original canonical projection with no predecessor checkpoint.
    LedgerGenesis { conversation_id: ConversationId },
    /// A checkpoint without native coverage, identified by its exact content.
    OpaqueCheckpoint { source: CheckpointSourceRef },
}

/// The captured retirement prefix and its coordinate in one coverage root.
///
/// `retired_through` is not a Ledger position or a count comparable across
/// roots. `captured_prefix_items` is interpreted only at `capture_cut` after
/// the Store has authenticated that exact projection and its predecessor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixCoverage {
    pub root: CoverageRoot,
    pub retired_through: u64,
    pub capture_cut: ResourceReadCut,
    pub captured_prefix_items: u32,
    /// Exact captured prefix identity at its original Resource Read Cut.
    pub capture_digest: String,
    /// Frozen rendered source bytes actually covered by the accepted chunks.
    pub rendered_bytes: u64,
    /// Digest of the frozen rendered source roles, indices and text.
    pub rendered_digest: String,
}

impl PrefixCoverage {
    /// Derive a coordinate from one captured projection predecessor. The
    /// caller must obtain the predecessor and its digest from native Store
    /// authority; this arithmetic alone does not authenticate either one.
    ///
    /// # Errors
    /// Rejects an empty/out-of-bounds prefix, foreign predecessor, impossible
    /// activation shape, or an overflowing logical frontier.
    pub fn from_capture(
        capture_cut: ResourceReadCut,
        captured_prefix_items: u32,
        capture_digest: String,
        rendered_bytes: u64,
        rendered_digest: String,
        predecessor: Option<(&CheckpointSourceRef, Option<&Self>)>,
    ) -> Result<Self, &'static str> {
        let count = u64::from(captured_prefix_items);
        let (root, retired_through) = match predecessor {
            None if capture_cut.projection_activation.0 == 0 => (
                CoverageRoot::LedgerGenesis {
                    conversation_id: capture_cut.conversation_id.clone(),
                },
                count,
            ),
            Some((source, prior)) if capture_cut.projection_activation.0 > 0 => {
                if source.conversation_id != capture_cut.conversation_id {
                    return Err("coverage predecessor belongs to another conversation");
                }
                match prior {
                    Some(prior) => {
                        prior.validate_shape()?;
                        if prior.capture_cut.conversation_id != capture_cut.conversation_id
                            || count == 0
                        {
                            return Err("native predecessor coverage or prefix is invalid");
                        }
                        (
                            prior.root.clone(),
                            prior
                                .retired_through
                                .checked_add(count - 1)
                                .ok_or("coverage frontier overflow")?,
                        )
                    }
                    None => (
                        CoverageRoot::OpaqueCheckpoint {
                            source: source.clone(),
                        },
                        count,
                    ),
                }
            }
            _ => return Err("coverage predecessor disagrees with captured activation"),
        };
        let value = Self {
            root,
            retired_through,
            capture_cut,
            captured_prefix_items,
            capture_digest,
            rendered_bytes,
            rendered_digest,
        };
        value.validate_shape()?;
        Ok(value)
    }

    /// Compare already-authenticated continuous prefixes; differing roots
    /// remain incomparable, regardless of their numerical frontiers.
    #[must_use]
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        (self.root == other.root).then(|| self.retired_through.cmp(&other.retired_through))
    }

    /// Compare this authenticated record to a current authenticated Head.
    /// An unmarked Head is an opaque root; no Head is the Ledger genesis.
    #[must_use]
    pub fn compare_to_head(
        &self,
        head: Option<(&CheckpointSourceRef, Option<&Self>)>,
    ) -> Option<Ordering> {
        match head {
            Some((source, Some(active)))
                if source.conversation_id == self.capture_cut.conversation_id =>
            {
                self.compare(active)
            }
            Some((source, None))
                if self.root
                    == (CoverageRoot::OpaqueCheckpoint {
                        source: source.clone(),
                    }) =>
            {
                Some(self.retired_through.cmp(&0))
            }
            None if self.root
                == (CoverageRoot::LedgerGenesis {
                    conversation_id: self.capture_cut.conversation_id.clone(),
                }) =>
            {
                Some(self.retired_through.cmp(&0))
            }
            _ => None,
        }
    }

    /// Index of the retained suffix for a strict compatible advance. The
    /// caller must still verify this bound and the complete source structure.
    #[must_use]
    pub fn retained_offset(
        &self,
        head: Option<(&CheckpointSourceRef, Option<&Self>)>,
    ) -> Option<usize> {
        if self.compare_to_head(head) != Some(Ordering::Greater) {
            return None;
        }
        let (active, summary) = head
            .and_then(|(_, active)| active)
            .map_or((0, 0_usize), |active| (active.retired_through, 1));
        usize::try_from(self.retired_through.checked_sub(active)?)
            .ok()?
            .checked_add(summary)
    }

    /// Check finite local shape before the Store resolves authoritative facts.
    /// # Errors
    /// Rejects empty, oversized, foreign or malformed coverage coordinates.
    pub fn validate_shape(&self) -> Result<(), &'static str> {
        let scope = &self.capture_cut.conversation_id;
        if self.retired_through == 0
            || self.captured_prefix_items == 0
            || self.captured_prefix_items as usize > COMPACTION_RESOURCE_ITEMS_MAX
            || self.rendered_bytes > COMPACTION_RESOURCE_BYTES_MAX as u64
            || !valid_digest(&self.capture_digest)
            || !valid_digest(&self.rendered_digest)
        {
            return Err("coverage prefix must be positive and within captured item bounds");
        }
        match &self.root {
            CoverageRoot::LedgerGenesis { conversation_id } if conversation_id == scope => Ok(()),
            CoverageRoot::OpaqueCheckpoint { source }
                if &source.conversation_id == scope
                    && !source.checkpoint_id.as_str().is_empty()
                    && valid_digest(&source.content_digest) =>
            {
                Ok(())
            }
            _ => Err("coverage root is foreign or has invalid checkpoint identity"),
        }
    }
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
