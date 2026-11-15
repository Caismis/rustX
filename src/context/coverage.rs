//! Bounded coordinates for comparable, authenticated prefix coverage.
//!
//! A coordinate is meaningful only after the Store verifies its capture and
//! installation evidence. Comparing values from different roots is never a
//! claim that either one covers the other.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::context::checkpoint::SummaryGenerationRef;
use crate::context::chunks::CompletedSourceCoverage;
use crate::context::resources::ResourceReadCut;
use crate::context::{ContextError, ContextErrorKind};
use crate::model::ModelInputMessage;
use crate::model::SelectedModelInput;
use crate::model::projected::{
    CanonicalMessageRef, CheckpointSourceRef, DerivedModelMessage, ProjectedMessage,
    SemanticTransformation,
};
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
            || self.captured_prefix_items as usize
                > super::resources::ResourceLimits::default().total_items
            || self.rendered_bytes > super::resources::ResourceLimits::default().total_bytes as u64
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

/// Process-local generation proof. It is never deserialized from a checkpoint
/// or accepted from an external caller; a native generator seals it only from
/// an accepted complete `ChunkPlan` witness and its exact source/output values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPrefixCoverage {
    record: PrefixCoverage,
    summary_digest: String,
    generation: Option<SummaryGenerationRef>,
}

impl VerifiedPrefixCoverage {
    /// Seal one fully accepted captured prefix and its actual final Summary.
    /// The witness binds both the unrendered capture and the frozen rendered
    /// source. Store later checks the predecessor and retained current suffix.
    pub(crate) fn seal(
        generated: &super::compaction::GeneratedCompaction,
    ) -> Result<Self, ContextError> {
        let cut = generated.source_cut().clone();
        let previous = generated.source_checkpoint();
        let previous_source = previous.map(|checkpoint| CheckpointSourceRef {
            conversation_id: cut.conversation_id.clone(),
            checkpoint_id: checkpoint.candidate.checkpoint_id.clone(),
            content_digest: checkpoint.fingerprint.clone(),
        });
        let predecessor = previous_source.as_ref().map(|source| {
            (
                source,
                previous.and_then(|checkpoint| checkpoint.candidate.prefix_coverage.as_ref()),
            )
        });
        let generation = generated.summary_generation().generation.as_ref();
        Self::seal_parts(
            generated.completed_coverage(),
            cut,
            &generated.source_history().messages,
            predecessor,
            generated.generated_summary(),
            generation,
        )
    }

    #[allow(clippy::too_many_arguments)] // These are independently frozen authorities.
    fn seal_parts(
        witness: &CompletedSourceCoverage,
        cut: ResourceReadCut,
        input: &SelectedModelInput,
        predecessor: Option<(&CheckpointSourceRef, Option<&PrefixCoverage>)>,
        summary: &DerivedModelMessage,
        generation: Option<&SummaryGenerationRef>,
    ) -> Result<Self, ContextError> {
        let count = usize::try_from(witness.source_count()).map_err(|_| {
            ContextError::new(
                ContextErrorKind::CannotFit,
                "captured prefix count exceeds platform bounds",
            )
        })?;
        let digest = capture_digest(&cut, input, count)?;
        if digest != witness.capture_digest()
            || generation.is_some_and(|generation| {
                generation.conversation_id != cut.conversation_id
                    || generation.through_journal <= cut.journal.0
                    || generation.call_id.as_str().is_empty()
            })
        {
            return Err(ContextError::new(
                ContextErrorKind::MalformedHistory,
                "completed Summary witness disagrees with its frozen capture or generation",
            ));
        }
        validate_summary_sources(
            &cut.conversation_id,
            input,
            count,
            predecessor.map(|(source, _)| source),
            summary,
        )?;
        let record = PrefixCoverage::from_capture(
            cut,
            u32::try_from(count).map_err(|_| {
                ContextError::new(
                    ContextErrorKind::CannotFit,
                    "captured prefix count exceeds its bound",
                )
            })?,
            digest,
            witness.rendered_bytes(),
            witness.rendered_digest().to_owned(),
            predecessor,
        )
        .map_err(|error| ContextError::new(ContextErrorKind::MalformedHistory, error))?;
        Ok(Self {
            record,
            summary_digest: summary_digest(summary, generation)?,
            generation: generation.cloned(),
        })
    }

    /// The bounded durable record described by this process-local proof.
    #[must_use]
    pub const fn record(&self) -> &PrefixCoverage {
        &self.record
    }

    /// Check the exact record, generated Summary and Call admitted at commit.
    pub(crate) fn authorizes(
        &self,
        record: &PrefixCoverage,
        summary: &DerivedModelMessage,
        generation: Option<&SummaryGenerationRef>,
    ) -> bool {
        &self.record == record
            && self.generation.as_ref() == generation
            && summary_digest(summary, generation).is_ok_and(|digest| digest == self.summary_digest)
    }
}

fn summary_digest(
    summary: &DerivedModelMessage,
    generation: Option<&SummaryGenerationRef>,
) -> Result<String, ContextError> {
    struct Writer<'a>(&'a mut Sha256, &'a mut usize);
    impl std::io::Write for Writer<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            *self.1 = self
                .1
                .checked_sub(bytes.len())
                .ok_or_else(|| std::io::Error::other("summary proof exceeds byte bound"))?;
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut hash = Sha256::new();
    let mut remaining = 8 * 1024 * 1024_usize;
    serde_json::to_writer(
        Writer(&mut hash, &mut remaining),
        &("rustx-summary-output-v1", summary, generation),
    )
    .map_err(|_| {
        ContextError::new(
            ContextErrorKind::CannotFit,
            "summary proof exceeds byte bound",
        )
    })?;
    Ok(format!("{:x}", hash.finalize()))
}

fn validate_summary_sources(
    scope: &ConversationId,
    input: &SelectedModelInput,
    count: usize,
    predecessor: Option<&CheckpointSourceRef>,
    summary: &DerivedModelMessage,
) -> Result<(), ContextError> {
    let invalid = || {
        ContextError::new(
            ContextErrorKind::MalformedHistory,
            "generated Summary does not name its complete captured prefix sources",
        )
    };
    if !matches!(
        &summary.message,
        ProjectedMessage::User {
            source: crate::message::types::UserSource::Runtime,
            kind: crate::model::projected::ProjectedUserKind::CompactionContext,
            ..
        }
    ) || summary.transformations != [SemanticTransformation::Summary]
        || count > input.len()
    {
        return Err(invalid());
    }
    let mut originals = BTreeMap::new();
    let mut checkpoints = BTreeMap::new();
    let mut includes_previous_summary = false;
    for message in input.iter().take(count) {
        match message {
            ModelInputMessage::Canonical(original) => {
                let source = CanonicalMessageRef::of(scope, original);
                originals.insert(source.message_id.clone(), source);
            }
            ModelInputMessage::Derived(derived)
                if derived
                    .transformations
                    .contains(&SemanticTransformation::Summary) =>
            {
                includes_previous_summary = true;
            }
            ModelInputMessage::Derived(derived) => {
                for source in &derived.sources {
                    originals.insert(source.message_id.clone(), source.clone());
                }
                for source in &derived.checkpoint_sources {
                    checkpoints.insert(source.checkpoint_id.clone(), source.clone());
                }
            }
            ModelInputMessage::RequestOnly(_) => return Err(invalid()),
        }
    }
    if includes_previous_summary {
        let prior = predecessor.ok_or_else(invalid)?;
        checkpoints.insert(prior.checkpoint_id.clone(), prior.clone());
    }
    if summary.sources != originals.into_values().collect::<Vec<_>>()
        || summary.checkpoint_sources != checkpoints.into_values().collect::<Vec<_>>()
    {
        return Err(invalid());
    }
    Ok(())
}

/// Hash the typed original projection occurrences and capture cut without
/// materializing encoded source copies. The same value binds a completed
/// chunk witness to the exact prefix that the Store can independently read.
pub(crate) fn capture_digest(
    cut: &ResourceReadCut,
    input: &SelectedModelInput,
    count: usize,
) -> Result<String, ContextError> {
    struct DigestBudget<'a> {
        digest: &'a mut Sha256,
        remaining: usize,
    }
    impl std::io::Write for DigestBudget<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.remaining = self
                .remaining
                .checked_sub(bytes.len())
                .ok_or_else(|| std::io::Error::other("capture identity exceeds its byte budget"))?;
            self.digest.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let limit = super::resources::ResourceLimits::default();
    if count == 0 || count > limit.total_items || count > input.len() {
        return Err(ContextError::new(
            ContextErrorKind::CannotFit,
            "captured prefix item count exceeds its source or resource bounds",
        ));
    }
    let prefix = input.range(0..count);
    let mut hash = Sha256::new();
    serde_json::to_writer(
        DigestBudget {
            digest: &mut hash,
            remaining: limit.total_bytes,
        },
        &("rustx-compaction-prefix-v1", cut, prefix),
    )
    .map_err(|_| {
        ContextError::new(
            ContextErrorKind::CannotFit,
            "capture identity exceeds its byte budget",
        )
    })?;
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::checkpoint::ProjectionActivationId;
    use crate::context::resources::{JournalFrontier, LedgerFrontier};
    use crate::durable::TranscriptCursor;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::CheckpointId;
    use crate::runtime::identity::{AuxiliaryCallId, MessageId};

    const CONVERSATION_ONE: &str = "conv_019a0000-0000-7000-8000-000000000001";
    const CONVERSATION_OTHER: &str = "conv_019a0000-0000-7000-8000-000000000002";

    fn genesis(scope: &str, frontier: u64) -> PrefixCoverage {
        let conversation_id = ConversationId::new(scope);
        PrefixCoverage {
            root: CoverageRoot::LedgerGenesis {
                conversation_id: conversation_id.clone(),
            },
            retired_through: frontier,
            capture_cut: ResourceReadCut {
                conversation_id,
                ledger: LedgerFrontier(10),
                journal: JournalFrontier(10),
                transcript: TranscriptCursor::new(10),
                projection_activation: ProjectionActivationId(0),
            },
            captured_prefix_items: 1,
            capture_digest: "a".repeat(64),
            rendered_bytes: 10,
            rendered_digest: "b".repeat(64),
        }
    }

    #[test]
    fn compares_only_equal_roots_and_checks_bounded_scope() {
        let earlier = genesis(CONVERSATION_ONE, 2);
        let later = genesis(CONVERSATION_ONE, 3);
        assert_eq!(earlier.compare(&later), Some(Ordering::Less));
        assert_eq!(later.compare(&earlier), Some(Ordering::Greater));
        assert_eq!(earlier.compare(&earlier), Some(Ordering::Equal));
        assert_eq!(earlier.compare(&genesis(CONVERSATION_OTHER, 99)), None);
        assert!(earlier.validate_shape().is_ok());
        let mut invalid = earlier;
        invalid.captured_prefix_items = 0;
        assert!(invalid.validate_shape().is_err());
    }

    #[test]
    fn derives_checked_frontiers_from_genesis_native_and_opaque_predecessors() {
        let mut initial = genesis(CONVERSATION_ONE, 2);
        initial.captured_prefix_items = 2;
        let root = PrefixCoverage::from_capture(
            initial.capture_cut.clone(),
            2,
            "a".repeat(64),
            10,
            "b".repeat(64),
            None,
        )
        .unwrap();
        assert_eq!(root, initial);
        let source = CheckpointSourceRef {
            conversation_id: ConversationId::new(CONVERSATION_ONE),
            checkpoint_id: CheckpointId::new("checkpoint"),
            content_digest: "a".repeat(64),
        };
        let mut cut = root.capture_cut.clone();
        cut.projection_activation = ProjectionActivationId(1);
        let later = PrefixCoverage::from_capture(
            cut.clone(),
            4,
            "c".repeat(64),
            20,
            "d".repeat(64),
            Some((&source, Some(&root))),
        )
        .unwrap();
        assert_eq!(later.retired_through, 5);
        assert_eq!(later.root, root.root);
        assert_eq!(
            later.compare_to_head(Some((&source, Some(&root)))),
            Some(Ordering::Greater)
        );
        assert_eq!(later.retained_offset(Some((&source, Some(&root)))), Some(4));
        assert_eq!(root.compare_to_head(None), Some(Ordering::Greater));
        assert_eq!(root.retained_offset(None), Some(2));
        assert_eq!(root.retained_offset(Some((&source, Some(&later)))), None);
        let opaque = PrefixCoverage::from_capture(
            cut,
            4,
            "c".repeat(64),
            20,
            "d".repeat(64),
            Some((&source, None)),
        )
        .unwrap();
        assert_eq!(opaque.retired_through, 4);
        assert_eq!(opaque.compare(&later), None);
        assert_eq!(opaque.retained_offset(Some((&source, None))), Some(4));
    }

    fn input(text: &str) -> SelectedModelInput {
        vec![ModelInputMessage::Canonical(MessageBlock::User(
            UserMessageBlock {
                id: MessageId::new("source"),
                content: vec![UserContentBlock::Text(TextBlock { text: text.into() })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            },
        ))]
        .into()
    }

    #[test]
    fn sealed_proof_rejects_a_changed_capture_summary_or_generation() {
        let cut = genesis(CONVERSATION_ONE, 1).capture_cut;
        let selected = input("frozen");
        let mut chunks =
            super::super::chunks::ChunkPlan::new_captured(&selected, &selected, 1, &cut).unwrap();
        assert!(chunks.completed_coverage().is_err());
        let request = chunks
            .next(None, None, None, u64::MAX, true, |_| Ok(1))
            .unwrap();
        chunks.accept(&request).unwrap();
        let witness = chunks.completed_coverage().unwrap();
        let original = selected[0].as_canonical().unwrap();
        let mut summary = DerivedModelMessage {
            message: ProjectedMessage::User {
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "generated".into(),
                })],
                source: UserSource::Runtime,
                kind: crate::model::projected::ProjectedUserKind::CompactionContext,
                timestamp: None,
            },
            sources: vec![CanonicalMessageRef::of(&cut.conversation_id, original)],
            checkpoint_sources: Vec::new(),
            transformations: vec![SemanticTransformation::Summary],
        };
        let generation = SummaryGenerationRef {
            conversation_id: cut.conversation_id.clone(),
            call_id: AuxiliaryCallId::new("aux_019a0000-0000-7000-8000-000000000001"),
            through_journal: cut.journal.0 + 1,
        };
        let proof = VerifiedPrefixCoverage::seal_parts(
            &witness,
            cut.clone(),
            &selected,
            None,
            &summary,
            Some(&generation),
        )
        .unwrap();
        assert!(proof.authorizes(proof.record(), &summary, Some(&generation)));
        assert!(!proof.authorizes(proof.record(), &summary, None));
        let scripted = VerifiedPrefixCoverage::seal_parts(
            &witness,
            cut.clone(),
            &selected,
            None,
            &summary,
            None,
        )
        .unwrap();
        assert!(scripted.authorizes(scripted.record(), &summary, None));
        assert!(!scripted.authorizes(scripted.record(), &summary, Some(&generation)));
        assert!(
            VerifiedPrefixCoverage::seal_parts(
                &witness,
                cut.clone(),
                &input("changed"),
                None,
                &summary,
                Some(&generation)
            )
            .is_err()
        );
        let mut later = cut;
        later.ledger = LedgerFrontier(later.ledger.0 + 1);
        assert!(
            VerifiedPrefixCoverage::seal_parts(
                &witness,
                later,
                &selected,
                None,
                &summary,
                Some(&generation)
            )
            .is_err()
        );
        let mut another_generation = generation.clone();
        another_generation.call_id =
            AuxiliaryCallId::new("aux_019a0000-0000-7000-8000-000000000002");
        assert!(!proof.authorizes(proof.record(), &summary, Some(&another_generation)));
        if let ProjectedMessage::User { content, .. } = &mut summary.message
            && let UserContentBlock::Text(text) = &mut content[0]
        {
            text.text = "altered".into();
        }
        assert!(!proof.authorizes(proof.record(), &summary, Some(&generation)));
    }
}
