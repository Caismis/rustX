use super::*;
use crate::context::checkpoint::{OriginActivation, ProjectionActivation, ProjectionActivationId};
use crate::context::resources::{JournalFrontier, LedgerFrontier, ResourceReadCut};
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::ModelInputMessage;
use crate::runtime::identity::{CheckpointId, MessageId};

fn scope() -> ConversationId {
    ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18")
}

fn source(index: usize) -> CheckpointSourceRef {
    CheckpointSourceRef {
        conversation_id: scope(),
        checkpoint_id: CheckpointId::new(format!("source-{index}")),
        content_digest: "digest".into(),
    }
}

fn message(index: usize) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(format!("message-{index}")),
        content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
            text: "history".into(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

fn derived(
    sources: Vec<CanonicalMessageRef>,
    checkpoint_sources: Vec<CheckpointSourceRef>,
    summary: bool,
) -> DerivedModelMessage {
    DerivedModelMessage {
        message: ProjectedMessage::User {
            content: vec![],
            source: UserSource::Runtime,
            kind: if summary {
                crate::model::projected::ProjectedUserKind::CompactionContext
            } else {
                crate::model::projected::ProjectedUserKind::Message
            },
            timestamp: None,
        },
        sources,
        checkpoint_sources,
        transformations: vec![if summary {
            SemanticTransformation::Summary
        } else {
            SemanticTransformation::UploadRendering
        }],
    }
}

fn item(derived: DerivedModelMessage) -> CheckpointItem {
    CheckpointItem {
        message: CheckpointMessage::Derived(Box::new(derived)),
        call_sources: vec![],
        result_call: None,
    }
}

fn candidate(items: Vec<CheckpointItem>) -> CheckpointCandidate {
    CheckpointCandidate {
        checkpoint_id: CheckpointId::new("parent"),
        expected_head: None,
        read_cut: ResourceReadCut {
            conversation_id: scope(),
            ledger: LedgerFrontier(2048),
            journal: JournalFrontier(2048),
            transcript: crate::durable::TranscriptCursor::new(2048),
            projection_activation: ProjectionActivationId(0),
        },
        base_frontier: LedgerFrontier(2048),
        prefix_coverage: None,
        items,
        summary_metadata: None,
        summary_generation: None,
    }
}

fn parent(items: Vec<CheckpointItem>) -> crate::durable::projection::ProjectionCheckpoint {
    let candidate = candidate(items);
    let fingerprint = candidate.fingerprint();
    crate::durable::projection::ProjectionCheckpoint {
        activation: ProjectionActivation {
            activation: ProjectionActivationId(1),
            ledger: candidate.base_frontier,
            checkpoint_id: candidate.checkpoint_id.clone(),
            origin: OriginActivation {
                conversation_id: scope(),
                activation: ProjectionActivationId(1),
                completed_journal: JournalFrontier(1),
            },
            destination_installation: None,
        },
        candidate,
        fingerprint,
        occupancy: None,
    }
}

// Check exactly the shape that generation emits against the actual budget validator.
fn assert_admission_matches_candidate(
    provenance: &SummaryProvenance,
    retained: &[CheckpointItem],
    admitted: bool,
) {
    let mut items = vec![item(derived(
        provenance.sources.values().cloned().collect(),
        provenance.checkpoint_sources.values().cloned().collect(),
        true,
    ))];
    items.extend_from_slice(retained);
    assert_eq!(provenance.admit(retained).is_ok(), admitted);
    assert_eq!(candidate(items).validate_budget().is_ok(), admitted);
}

#[test]
fn non_summary_checkpoint_sources_are_counted_and_deduplicated() {
    // A valid general checkpoint can contain a non-Summary Derived item
    // backed solely by prior checkpoint references. Its full 1024 refs fit.
    let mut refs: Vec<_> = (0..1023).map(source).collect();
    refs.push(source(0));
    let original = derived(vec![], refs, false);
    let previous = parent(vec![item(original.clone())]);
    assert!(previous.candidate.validate_budget().is_ok());
    // This general checkpoint shape compiles without canonical bodies.
    assert!(
        crate::context::checkpoint::compile_checkpoint::<
            crate::context::checkpoint::CheckpointError,
        >(&previous.candidate, |_, _| panic!(
            "checkpoint-only provenance needs no canonical body"
        ),)
        .is_ok()
    );
    // The Derived item is followed by canonical Tail, exactly as
    // HistoricalModelInput::checkpoint_items constructs it.
    for (canonical_count, admitted) in [(1, true), (2, false)] {
        let input: crate::model::SelectedModelInput =
            std::iter::once(ModelInputMessage::Derived(original.clone()))
                .chain((0..canonical_count).map(|i| ModelInputMessage::Canonical(message(i))))
                .collect();
        let provenance =
            SummaryProvenance::collect(&input, canonical_count + 1, &scope(), Some(&previous))
                .unwrap();
        assert_eq!(provenance.checkpoint_sources.len(), 1023);
        assert!(!provenance.includes_previous_summary);
        assert_admission_matches_candidate(&provenance, &[], admitted);
    }

    // Retained Derived refs consume slots independently from retired refs.
    let input: crate::model::SelectedModelInput = vec![ModelInputMessage::Derived(original)].into();
    let provenance = SummaryProvenance::collect(&input, 1, &scope(), Some(&previous)).unwrap();
    for (count, admitted) in [(1, true), (2, false)] {
        let retained = vec![item(derived(
            vec![],
            (0..count).map(source).collect(),
            false,
        ))];
        assert_admission_matches_candidate(&provenance, &retained, admitted);
    }
}

#[test]
fn parent_is_counted_only_when_a_summary_is_retired() {
    let previous_summary = derived(vec![], vec![source(0)], true);
    let previous = parent(vec![item(previous_summary.clone())]);
    let make_input = |count| {
        std::iter::once(ModelInputMessage::Derived(previous_summary.clone()))
            .chain((0..count).map(|i| ModelInputMessage::Canonical(message(i))))
            .collect::<crate::model::SelectedModelInput>()
    };
    for (canonical_count, admitted) in [(1023, true), (1024, false)] {
        let input = make_input(canonical_count);
        let provenance =
            SummaryProvenance::collect(&input, canonical_count + 1, &scope(), Some(&previous))
                .unwrap();
        assert_eq!(provenance.checkpoint_sources.len(), 1);
        assert_eq!(
            provenance
                .checkpoint_sources
                .values()
                .next()
                .unwrap()
                .checkpoint_id,
            previous.candidate.checkpoint_id
        );
        assert_admission_matches_candidate(&provenance, &[], admitted);
    }
}
