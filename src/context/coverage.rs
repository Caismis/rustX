//! Seal a Summary only after its frozen chunk cursor has accepted complete coverage.

use crate::context::checkpoint::SummaryGenerationRef;
use crate::context::chunks::CompletedSourceCoverage;
use crate::context::resources::ResourceReadCut;
use crate::context::{ContextError, ContextErrorKind};
use crate::model::SelectedModelInput;
use crate::model::projected::{CheckpointSourceRef, DerivedModelMessage};

pub use rustx_durable::compaction_coverage::VerifiedPrefixCoverage;
pub(crate) use rustx_durable::compaction_coverage::capture_digest;
pub use rustx_types::context::coverage::{CoverageRoot, PrefixCoverage};

pub(super) fn seal(
    generated: &super::compaction::GeneratedCompaction,
    allow_current_reduction: bool,
) -> Result<VerifiedPrefixCoverage, ContextError> {
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
    seal_parts(
        generated.completed_coverage(),
        cut,
        &generated.source_history().messages,
        predecessor,
        generated.generated_summary(),
        generated.summary_generation().generation.as_ref(),
        allow_current_reduction,
    )
}

fn seal_parts(
    witness: &CompletedSourceCoverage,
    cut: ResourceReadCut,
    input: &SelectedModelInput,
    predecessor: Option<(&CheckpointSourceRef, Option<&PrefixCoverage>)>,
    summary: &DerivedModelMessage,
    generation: Option<&SummaryGenerationRef>,
    allow_current_reduction: bool,
) -> Result<VerifiedPrefixCoverage, ContextError> {
    let record = PrefixCoverage::from_capture(
        cut,
        witness.source_count(),
        witness.capture_digest().to_owned(),
        witness.rendered_bytes(),
        witness.rendered_digest().to_owned(),
        predecessor,
    )
    .map_err(|error| ContextError::new(ContextErrorKind::MalformedHistory, error))?;
    VerifiedPrefixCoverage::seal(
        record,
        input,
        predecessor,
        summary,
        generation,
        allow_current_reduction,
    )
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
    use crate::model::ModelInputMessage;
    use crate::model::projected::{CanonicalMessageRef, ProjectedMessage, SemanticTransformation};
    use crate::runtime::identity::{AuxiliaryCallId, MessageId};
    use crate::runtime::identity::{CheckpointId, ConversationId};
    use std::cmp::Ordering;

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
        let proof = seal_parts(
            &witness,
            cut.clone(),
            &selected,
            None,
            &summary,
            Some(&generation),
            false,
        )
        .unwrap();
        assert!(proof.authorizes(proof.record(), &summary, Some(&generation)));
        assert!(!proof.authorizes(proof.record(), &summary, None));
        let scripted = seal_parts(
            &witness,
            cut.clone(),
            &selected,
            None,
            &summary,
            None,
            false,
        )
        .unwrap();
        assert!(scripted.authorizes(scripted.record(), &summary, None));
        assert!(!scripted.authorizes(scripted.record(), &summary, Some(&generation)));
        assert!(
            seal_parts(
                &witness,
                cut.clone(),
                &input("changed"),
                None,
                &summary,
                Some(&generation),
                false,
            )
            .is_err()
        );
        let mut later = cut;
        later.ledger = LedgerFrontier(later.ledger.0 + 1);
        assert!(
            seal_parts(
                &witness,
                later,
                &selected,
                None,
                &summary,
                Some(&generation),
                false,
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
