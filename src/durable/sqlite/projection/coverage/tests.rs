use super::*;
use crate::context::checkpoint::{CheckpointItem, CheckpointMessage};
use crate::context::coverage::{CoverageRoot, PrefixCoverage, capture_digest};
use crate::durable::{CanonicalStore, ConversationStore, ProjectionStore, SqliteConversationStore};
use crate::message::content::TextBlock;
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::projected::{
    CanonicalMessageRef, DerivedModelMessage, ProjectedMessage, SemanticTransformation,
};
use crate::runtime::identity::MessageId;

fn message(n: usize) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(format!("coverage-{n}")),
        content: vec![UserContentBlock::Text(TextBlock {
            text: format!("source {n}"),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

fn record(store: &SqliteConversationStore, count: usize) -> PrefixCoverage {
    let cut = store.compaction_resource_cut().unwrap();
    let history = store.projected_history().unwrap();
    PrefixCoverage::from_capture(
        cut.clone(),
        count.try_into().unwrap(),
        capture_digest(&cut, &history.messages, count).unwrap(),
        10,
        "a".repeat(64),
        None,
    )
    .unwrap()
}

fn candidate(store: &SqliteConversationStore, record: &PrefixCoverage) -> CheckpointCandidate {
    let cut = store.compaction_resource_cut().unwrap();
    let history = store.projected_history().unwrap();
    let count = record.captured_prefix_items as usize;
    let summary = CheckpointItem {
        message: CheckpointMessage::Derived(Box::new(DerivedModelMessage {
            message: ProjectedMessage::User {
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "summary".into(),
                })],
                source: UserSource::Runtime,
                kind: crate::model::projected::ProjectedUserKind::CompactionContext,
                timestamp: None,
            },
            sources: history
                .messages
                .iter()
                .take(count)
                .map(|source| {
                    CanonicalMessageRef::of(store.conversation_id(), source.as_canonical().unwrap())
                })
                .collect(),
            checkpoint_sources: Vec::new(),
            transformations: vec![SemanticTransformation::Summary],
        })),
        call_sources: Vec::new(),
        result_call: None,
    };
    let mut items = vec![summary];
    items.extend(history.items.iter().skip(count).cloned());
    CheckpointCandidate {
        checkpoint_id: CheckpointId::new("coverage-new"),
        expected_head: None,
        base_frontier: cut.ledger,
        read_cut: cut,
        items,
        prefix_coverage: Some(record.clone()),
        summary_metadata: None,
        summary_generation: None,
    }
}

#[test]
fn captured_prefix_requires_exact_cut_digest_and_frontier() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store
        .initialize(&[message(1), message(2), message(3)])
        .unwrap();
    let record = record(&store, 1);
    let connection = store.lock().unwrap();
    captured_history(&connection, store.conversation_id(), &record).unwrap();
    let mut forged_frontier = record.clone();
    forged_frontier.retired_through = 2;
    assert!(captured_history(&connection, store.conversation_id(), &forged_frontier).is_err());
    let mut forged_root = record.clone();
    forged_root.root = CoverageRoot::LedgerGenesis {
        conversation_id: ConversationId::new("conv_019a0000-0000-7000-8000-000000000004"),
    };
    assert!(captured_history(&connection, store.conversation_id(), &forged_root).is_err());
    let mut changed_digest = record.clone();
    changed_digest.capture_digest = "b".repeat(64);
    assert!(captured_history(&connection, store.conversation_id(), &changed_digest).is_err());
    let mut out_of_range = record;
    out_of_range.captured_prefix_items = 4;
    assert!(captured_history(&connection, store.conversation_id(), &out_of_range).is_err());
}

#[test]
fn installation_retains_every_current_suffix_occurrence_once() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store
        .initialize(&[message(1), message(2), message(3)])
        .unwrap();
    let record = record(&store, 1);
    store.append_canonical(&message(4)).unwrap();
    let candidate = candidate(&store, &record);
    let connection = store.lock().unwrap();
    validate_current_suffix(
        &connection,
        store.conversation_id(),
        &candidate,
        &record,
        None,
    )
    .unwrap();
    let mut missing = candidate.clone();
    missing.items.pop();
    assert!(
        validate_current_suffix(
            &connection,
            store.conversation_id(),
            &missing,
            &record,
            None
        )
        .is_err()
    );
    let mut reordered = candidate.clone();
    reordered.items.swap(1, 2);
    assert!(
        validate_current_suffix(
            &connection,
            store.conversation_id(),
            &reordered,
            &record,
            None
        )
        .is_err()
    );
    let mut duplicated = candidate;
    duplicated.items[1] = duplicated.items[2].clone();
    assert!(
        validate_current_suffix(
            &connection,
            store.conversation_id(),
            &duplicated,
            &record,
            None
        )
        .is_err()
    );
}

#[test]
fn competing_native_head_must_have_less_compatible_coverage() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store
        .initialize(&[message(1), message(2), message(3)])
        .unwrap();
    let first = record(&store, 1);
    let second = record(&store, 2);
    let first_candidate = candidate(&store, &first);
    let activation = ProjectionActivation {
        activation: ProjectionActivationId(1),
        ledger: first_candidate.base_frontier,
        checkpoint_id: first_candidate.checkpoint_id.clone(),
        origin: OriginActivation {
            conversation_id: store.conversation_id().clone(),
            activation: ProjectionActivationId(1),
            completed_journal: JournalFrontier(1),
        },
        destination_installation: None,
    };
    let current = ProjectionCheckpoint {
        fingerprint: first_candidate.fingerprint(),
        candidate: first_candidate,
        activation,
        occupancy: None,
    };
    store.append_canonical(&message(4)).unwrap();
    let candidate = candidate(&store, &second);
    let connection = store.lock().unwrap();
    validate_current_suffix(
        &connection,
        store.conversation_id(),
        &candidate,
        &second,
        Some(&current),
    )
    .unwrap();
    assert!(
        validate_current_suffix(
            &connection,
            store.conversation_id(),
            &candidate,
            &first,
            Some(&current)
        )
        .is_err()
    );
    let mut foreign = second;
    foreign.root = CoverageRoot::LedgerGenesis {
        conversation_id: ConversationId::new("conv_019a0000-0000-7000-8000-000000000004"),
    };
    assert!(
        validate_current_suffix(
            &connection,
            store.conversation_id(),
            &candidate,
            &foreign,
            Some(&current)
        )
        .is_err()
    );
}

#[test]
fn serialized_coverage_record_alone_cannot_authorize_a_commit() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize(&[message(1), message(2)]).unwrap();
    let record = record(&store, 1);
    let operation_id = CompactionOperationId::new("missing-proof");
    let signal = crate::runtime::cancellation::CancellationSignal::new();
    let input = ProjectionCommitInput {
        grant: signal
            .authorize_compaction_commit(operation_id.clone())
            .unwrap(),
        operation_id,
        candidate: candidate(&store, &record),
        prefix_proof: None,
        owner: store.projection_owner(None).unwrap(),
        tokens_before: crate::runtime::types::TokenMeasurement {
            input_tokens: 100,
            source: crate::runtime::types::TokenMeasurementSource::Estimated,
        },
        estimated_tokens_after: 10,
        occupancy: None,
        turn_id: None,
        timestamp: chrono::Utc::now(),
    };
    let connection = store.lock().unwrap();
    assert!(matches!(
        validate_commit(&connection, store.conversation_id(), &input, None),
        Err(ConversationStoreError::InvalidReference(message)) if message.contains("sealed completed-generation proof")
    ));
}

#[test]
fn head_competition_is_distinct_from_corruption_and_preserves_receipt_replay() {
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};

    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize(&[message(1), message(2)]).unwrap();
    let record = record(&store, 1);
    let mut first_candidate = candidate(&store, &record);
    first_candidate.prefix_coverage = None;
    let mut stale_candidate = first_candidate.clone();
    stale_candidate.checkpoint_id = CheckpointId::new("stale-coverage-test");
    let make_input = |operation: &str, candidate| {
        let operation_id = CompactionOperationId::new(operation);
        ProjectionCommitInput {
            grant: CancellationSignal::new()
                .authorize_compaction_commit(operation_id.clone())
                .unwrap(),
            operation_id,
            candidate,
            prefix_proof: None,
            owner: store.projection_owner(None).unwrap(),
            tokens_before: TokenMeasurement {
                input_tokens: 100,
                source: TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 10,
            occupancy: None,
            turn_id: None,
            timestamp: chrono::Utc::now(),
        }
    };
    let first = make_input("first-coverage-test", first_candidate);
    let stale = make_input("stale-coverage-test", stale_candidate);
    let receipt = store.commit_projection(first.clone()).unwrap();
    assert!(matches!(
        store.commit_projection(stale.clone()),
        Err(ConversationStoreError::ProjectionHeadChanged)
    ));
    let replay = store.commit_projection(first).unwrap();
    assert!(!replay.newly_committed);
    assert_eq!(replay.candidate_fingerprint, receipt.candidate_fingerprint);
    store
        .lock()
        .unwrap()
        .execute("DELETE FROM projection_head WHERE id=1", [])
        .unwrap();
    assert!(matches!(
        store.commit_projection(stale),
        Err(ConversationStoreError::InvalidReference(_))
    ));
}

#[test]
fn restart_evidence_refuses_missing_or_corrupted_atomic_commit_parts() {
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};
    for damage in ["receipt", "receipt_digest", "checkpoint"] {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize(&[message(1), message(2)]).unwrap();
        let record = record(&store, 1);
        let mut candidate = candidate(&store, &record);
        candidate.prefix_coverage = None;
        let operation = CompactionOperationId::new("restart-evidence");
        store
            .commit_projection(ProjectionCommitInput {
                grant: CancellationSignal::new()
                    .authorize_compaction_commit(operation.clone())
                    .unwrap(),
                operation_id: operation.clone(),
                candidate,
                prefix_proof: None,
                owner: store.projection_owner(None).unwrap(),
                tokens_before: TokenMeasurement {
                    input_tokens: 100,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 10,
                occupancy: None,
                turn_id: None,
                timestamp: chrono::Utc::now(),
            })
            .unwrap();
        assert!(
            store
                .compaction_restart_evidence(&operation)
                .unwrap()
                .committed
        );
        let connection = store.lock().unwrap();
        match damage {
            "receipt" => connection.execute("DELETE FROM compaction_operations WHERE operation_id=?1", [operation.as_str()]),
            "receipt_digest" => connection.execute("UPDATE compaction_operations SET receipt_digest=?1 WHERE operation_id=?2", rusqlite::params!["0".repeat(64), operation.as_str()]),
            "checkpoint" => connection.execute("UPDATE projection_checkpoints SET candidate_fingerprint=?1 WHERE checkpoint_id='coverage-new'", ["0".repeat(64)]),
            _ => unreachable!(),
        }.unwrap();
        drop(connection);
        assert!(
            store.compaction_restart_evidence(&operation).is_err(),
            "{damage}"
        );
    }
}
