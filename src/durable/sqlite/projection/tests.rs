use super::*;
use crate::context::checkpoint::{CheckpointItem, CheckpointMessage};
use crate::durable::{ConversationStore, SqliteConversationStore};
use crate::message::TextBlock;
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::projected::{
    CanonicalMessageRef, DerivedModelMessage, ProjectedMessage, SemanticTransformation,
};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::MessageId;
use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};

fn message(n: u64) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(format!("M{n}")),
        content: vec![UserContentBlock::Text(TextBlock {
            text: format!("content {n}"),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}
fn store() -> SqliteConversationStore {
    SqliteConversationStore::in_memory(ConversationId::generate()).unwrap()
}
fn summary(text: &str) -> CheckpointItem {
    CheckpointItem {
        message: CheckpointMessage::Derived(Box::new(DerivedModelMessage {
            message: ProjectedMessage::User {
                content: vec![UserContentBlock::Text(TextBlock { text: text.into() })],
                source: UserSource::Runtime,
                kind: crate::model::projected::ProjectedUserKind::Message,
                timestamp: None,
            },
            sources: Vec::new(),
            checkpoint_sources: Vec::new(),
            transformations: vec![SemanticTransformation::Summary],
        })),
        call_sources: Vec::new(),
        result_call: None,
    }
}
fn candidate(
    store: &SqliteConversationStore,
    id: &str,
    items: Vec<CheckpointItem>,
) -> CheckpointCandidate {
    let cut = store.compaction_resource_cut().unwrap();
    CheckpointCandidate {
        checkpoint_id: CheckpointId::new(id),
        expected_head: store
            .projection_head()
            .unwrap()
            .map(|p| p.candidate.checkpoint_id),
        base_frontier: cut.ledger,
        read_cut: cut,
        items,
        summary_metadata: None,
    }
}
fn commit_input(
    store: &SqliteConversationStore,
    operation: &str,
    candidate: CheckpointCandidate,
    signal: &CancellationSignal,
) -> ProjectionCommitInput {
    let operation_id = CompactionOperationId::new(operation);
    ProjectionCommitInput {
        grant: signal
            .authorize_compaction_commit(operation_id.clone())
            .expect("commit granted"),
        operation_id,
        candidate,
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
}

#[test]
fn checkpoint_base_excludes_m96_through_m100_and_only_m101_enters_tail() {
    let store = store();
    let messages: Vec<_> = (1..=100).map(message).collect();
    store.initialize(&messages).unwrap();
    let original = |n: usize| CheckpointItem {
        message: CheckpointMessage::Original(CanonicalMessageRef::of(
            store.conversation_id(),
            &messages[n - 1],
        )),
        call_sources: Vec::new(),
        result_call: None,
    };
    let value = candidate(
        &store,
        "P1",
        vec![summary("summary M1–M70"), original(90), original(95)],
    );
    let receipt = store
        .commit_projection(commit_input(
            &store,
            "O1",
            value,
            &CancellationSignal::new(),
        ))
        .unwrap();
    assert_eq!(
        receipt.checkpoint.candidate.base_frontier,
        LedgerFrontier(100)
    );
    assert_eq!(
        store.load_canonical().unwrap(),
        messages,
        "projection commits no canonical summary"
    );
    store.append_canonical(&message(101)).unwrap();
    let history = store.projected_history().unwrap();
    let ids: Vec<_> = history
        .messages
        .iter()
        .filter_map(|m| m.canonical_id().map(ToString::to_string))
        .collect();
    assert_eq!(ids, ["M90", "M95", "M101"]);
    assert!(history.messages[0].canonical_id().is_none());
    assert_eq!(history.cut.ledger, LedgerFrontier(101));
    assert_eq!(history.cut.base_frontier, LedgerFrontier(100));
}

#[test]
fn operation_receipts_are_idempotent_conflicting_and_never_rewind_a_newer_head() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let first = commit_input(
        &store,
        "O1",
        candidate(&store, "P1", vec![summary("first")]),
        &CancellationSignal::new(),
    );
    let receipt = store.commit_projection(first.clone()).unwrap();
    assert_eq!(store.commit_projection(first.clone()).unwrap(), receipt);
    let second = store
        .commit_projection(commit_input(
            &store,
            "O2",
            candidate(&store, "P2", vec![summary("second")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    assert_eq!(
        store.commit_projection(first.clone()).unwrap(),
        receipt,
        "receipt remains historical after another activation"
    );
    assert_eq!(store.projection_head().unwrap().unwrap(), second.checkpoint);
    let mut conflicting = first;
    conflicting.candidate.items = vec![summary("changed candidate")];
    assert!(
        store
            .commit_projection(conflicting)
            .unwrap_err()
            .to_string()
            .contains("different candidate")
    );
    let events = store.read_events(None, 64).unwrap();
    assert_eq!(
        events
            .events
            .iter()
            .filter(|e| matches!(e.event, RuntimeEvent::CompactionCompleted { .. }))
            .count(),
        2
    );
}

#[test]
fn unrelated_journal_appends_and_post_base_canonical_growth_do_not_stale_a_candidate() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let input = commit_input(
        &store,
        "O1",
        candidate(&store, "P1", vec![summary("one")]),
        &CancellationSignal::new(),
    );
    store
        .append_event(RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: chrono::Utc::now(),
            event: RuntimeEvent::CompactionStarted {
                operation_id: input.operation_id.clone(),
            },
        })
        .unwrap();
    store.append_canonical(&message(2)).unwrap();
    store.commit_projection(input).unwrap();
    let history = store.projected_history().unwrap();
    assert_eq!(history.cut.base_frontier, LedgerFrontier(1));
    assert_eq!(history.messages[1].canonical_id(), Some(message(2).id()));
}

#[test]
fn cancellation_before_grant_prevents_commit_and_after_grant_preserves_authorized_durability() {
    let before = CancellationSignal::new();
    before.cancel();
    assert!(
        before
            .authorize_compaction_commit(CompactionOperationId::new("cancelled"))
            .is_none()
    );
    let store = std::sync::Arc::new(store());
    store.initialize(&[message(1)]).unwrap();
    let signal = CancellationSignal::new();
    let (granted_tx, granted_rx) = std::sync::mpsc::channel();
    let (commit_tx, commit_rx) = std::sync::mpsc::channel();
    let thread = {
        let store = store.clone();
        let signal = signal.clone();
        std::thread::spawn(move || {
            let input = commit_input(
                &store,
                "O1",
                candidate(&store, "P1", vec![summary("authorized")]),
                &signal,
            );
            granted_tx.send(()).unwrap(); // linearization: shared cancellation gate granted O1
            commit_rx.recv().unwrap();
            store.commit_projection(input).unwrap()
        })
    };
    granted_rx.recv().unwrap();
    signal.cancel();
    commit_tx.send(()).unwrap();
    let receipt = thread.join().unwrap();
    assert_eq!(
        store.projection_head().unwrap().unwrap(),
        receipt.checkpoint
    );
    assert!(
        store
            .read_events(None, 64)
            .unwrap()
            .events
            .iter()
            .all(|e| !matches!(e.event, RuntimeEvent::CompactionFailed { .. }))
    );
}

#[test]
fn same_ledger_frontier_retains_both_activation_boundaries() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    store
        .commit_projection(commit_input(
            &store,
            "O1",
            candidate(&store, "P1", vec![summary("first")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    store
        .commit_projection(commit_input(
            &store,
            "O2",
            candidate(&store, "P2", vec![summary("second")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    let timeline = store
        .projection_timeline(ProjectionActivationId(0), 64)
        .unwrap();
    assert_eq!(timeline.len(), 2);
    assert_eq!(timeline[0].activation.ledger, timeline[1].activation.ledger);
    assert_ne!(
        timeline[0].activation.activation,
        timeline[1].activation.activation
    );
    let first = store
        .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(1))
        .unwrap();
    let second = store
        .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(2))
        .unwrap();
    assert_eq!(
        first.projections.last().unwrap().candidate.checkpoint_id,
        CheckpointId::new("P1")
    );
    assert_eq!(
        second.projections.last().unwrap().candidate.checkpoint_id,
        CheckpointId::new("P2")
    );
}

#[test]
fn missing_head_content_or_original_dependencies_never_fall_back_to_identity() {
    for missing in ["head", "content", "original"] {
        let store = store();
        let original = message(1);
        store.initialize(std::slice::from_ref(&original)).unwrap();
        let item = CheckpointItem {
            message: CheckpointMessage::Original(CanonicalMessageRef::of(
                store.conversation_id(),
                &original,
            )),
            call_sources: Vec::new(),
            result_call: None,
        };
        store
            .commit_projection(commit_input(
                &store,
                "O1",
                candidate(&store, "P1", vec![item]),
                &CancellationSignal::new(),
            ))
            .unwrap();
        let connection = store.lock().unwrap();
        match missing {
            "head" => {
                connection
                    .execute("DELETE FROM projection_head", [])
                    .unwrap();
            }
            "content" => {
                connection
                    .execute("UPDATE projection_checkpoints SET candidate_json='{}'", [])
                    .unwrap();
            }
            _ => {
                connection
                    .execute("UPDATE message_ledger SET message_json='{}'", [])
                    .unwrap();
            }
        }
        drop(connection);
        assert!(
            store.projected_history().is_err(),
            "{missing} must fail explicitly"
        );
    }
}

#[test]
fn unknown_commit_outcome_recovers_the_same_operation_without_reexecution() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let input = commit_input(
        &store,
        "O1",
        candidate(&store, "P1", vec![summary("one")]),
        &CancellationSignal::new(),
    );
    store.arm_compaction_fault_script([
        crate::durable::sqlite::CompactionFaultOperation::AfterCommit,
    ]);
    assert!(
        store.commit_projection(input.clone()).is_err(),
        "the transaction committed but its receipt was lost"
    );
    let receipt = store
        .projection_receipt(&input.operation_id, &input.candidate.fingerprint())
        .unwrap()
        .unwrap();
    assert_eq!(store.commit_projection(input).unwrap(), receipt);
    assert_eq!(
        store.projection_head().unwrap().unwrap(),
        receipt.checkpoint
    );
    assert_eq!(store.read_events(None, 64).unwrap().events.len(), 1);
    assert!(matches!(
        store.projected_history().unwrap().messages[0],
        crate::model::ModelInputMessage::Derived(_)
    ));
}

#[test]
fn execution_owner_transition_fences_a_prepared_checkpoint_grant() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let input = commit_input(
        &store,
        "O1",
        candidate(&store, "P1", vec![summary("one")]),
        &CancellationSignal::new(),
    );
    let attempt = AttemptId::new("new-owner");
    store
        .append_event(RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: Some(attempt.clone()),
            turn_id: None,
            timestamp: chrono::Utc::now(),
            event: RuntimeEvent::AttemptStarted {
                attempt_id: attempt,
            },
        })
        .unwrap();
    assert!(
        store
            .commit_projection(input)
            .unwrap_err()
            .to_string()
            .contains("owner changed")
    );
    assert!(store.projection_head().unwrap().is_none());
    assert_eq!(store.load_canonical().unwrap(), [message(1)]);
}

#[test]
fn activation_ledger_is_commit_time_frontier_and_not_checkpoint_base() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let input = commit_input(
        &store,
        "growth",
        candidate(&store, "before-growth", vec![summary("summary")]),
        &CancellationSignal::new(),
    );
    store.append_canonical(&message(2)).unwrap();
    let committed = store.commit_projection(input).unwrap();
    assert_eq!(
        committed.checkpoint.candidate.base_frontier,
        LedgerFrontier(1)
    );
    assert_eq!(committed.checkpoint.activation.ledger, LedgerFrontier(2));
    let earlier = store
        .read_lineage_cut(crate::conversation::SurfaceRevision::new(1))
        .unwrap();
    assert!(
        earlier.projections.is_empty(),
        "a future activation is legitimately ineligible at the earlier canonical boundary"
    );
    assert!(
        store
            .read_projection_lineage_cut(
                LedgerFrontier(1),
                committed.checkpoint.activation.activation
            )
            .is_err()
    );
    let at = store
        .read_projection_lineage_cut(
            LedgerFrontier(2),
            committed.checkpoint.activation.activation,
        )
        .unwrap();
    assert_eq!(at.projections.len(), 1);
    let history = store.projected_history().unwrap();
    assert_eq!(
        history.messages.last().unwrap().canonical_id(),
        Some(&MessageId::new("M2"))
    );
}

#[test]
fn ordinary_projection_preparation_does_not_hydrate_retired_summary_provenance() {
    let store = store();
    let source = (1..=1000).map(message).collect::<Vec<_>>();
    store.initialize(&source).unwrap();
    let mut derived = summary("bounded immutable summary");
    let CheckpointMessage::Derived(body) = &mut derived.message else {
        unreachable!()
    };
    body.sources = source
        .iter()
        .map(|m| CanonicalMessageRef::of(store.conversation_id(), m))
        .collect();
    let p = candidate(
        &store,
        "bounded",
        vec![
            derived,
            CheckpointItem {
                message: CheckpointMessage::Original(CanonicalMessageRef::of(
                    store.conversation_id(),
                    &source[989],
                )),
                call_sources: vec![],
                result_call: None,
            },
        ],
    );
    store
        .commit_projection(commit_input(
            &store,
            "bounded-op",
            p,
            &CancellationSignal::new(),
        ))
        .unwrap();
    store.append_canonical(&message(1001)).unwrap();
    super::super::compaction_resources::take_body_decodes();
    let input = store.projected_history().unwrap();
    assert_eq!(input.messages.len(), 3);
    assert_eq!(
        super::super::compaction_resources::take_body_decodes(),
        2,
        "only the visible Original and post-base Tail are hydrated; 1000 retired provenance facts are scalar integrity reads"
    );
    store.lock().unwrap().execute("UPDATE message_ledger SET message_json=replace(message_json,'content 1','corrupt 1') WHERE message_id='M1'",[]).unwrap();
    assert!(
        store.projected_history().is_err(),
        "provenance-only corruption still fails closed"
    );
}
