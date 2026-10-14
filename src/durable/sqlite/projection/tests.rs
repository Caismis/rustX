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

#[test]
fn dependency_free_checkpoint_rejects_every_future_resource_cut_atomically() {
    for domain in [
        "ledger",
        "journal",
        "transcript",
        "activation",
        "conversation",
    ] {
        let store = store();
        store.initialize(&[message(1)]).unwrap();
        let mut value = candidate(&store, "P-invalid", vec![summary("detached")]);
        match domain {
            "ledger" => {
                value.read_cut.ledger = LedgerFrontier(100);
                value.base_frontier = LedgerFrontier(100);
            }
            "journal" => value.read_cut.journal = JournalFrontier(100),
            "transcript" => value.read_cut.transcript = crate::durable::TranscriptCursor::new(100),
            "activation" => value.read_cut.projection_activation = ProjectionActivationId(100),
            _ => value.read_cut.conversation_id = ConversationId::generate(),
        }
        let before = store.compaction_resource_cut().unwrap();
        assert!(
            store
                .commit_projection(commit_input(
                    &store,
                    "invalid",
                    value,
                    &CancellationSignal::new()
                ))
                .is_err(),
            "accepted future {domain}"
        );
        assert_eq!(store.compaction_resource_cut().unwrap(), before);
        assert!(store.projection_head().unwrap().is_none());
        let connection = store.lock().unwrap();
        for table in [
            "projection_checkpoints",
            "projection_activations",
            "compaction_operations",
        ] {
            let count: u64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "partial {table}");
        }
    }
}

#[test]
fn trusted_metadata_sources_share_one_predecode_materialization_budget() {
    for count in [16, 33] {
        let store = store();
        let messages: Vec<_> = (1..=count)
            .map(|n| {
                let mut m = message(n);
                if let MessageBlock::User(u) = &mut m {
                    u.content = vec![UserContentBlock::Text(TextBlock {
                        text: "x".repeat(512 * 1024),
                    })];
                }
                m
            })
            .collect();
        store.initialize(&messages).unwrap();
        let mut item = summary("bounded metadata");
        let CheckpointMessage::Derived(d) = &mut item.message else {
            unreachable!()
        };
        d.sources = messages
            .iter()
            .map(|m| CanonicalMessageRef::of(store.conversation_id(), m))
            .collect();
        let mut value = candidate(&store, "metadata", vec![item]);
        value.summary_metadata = Some(crate::message::types::CompactionSummaryMetadata::empty());
        super::super::compaction_resources::take_body_decodes();
        let result = store.commit_projection(commit_input(
            &store,
            "metadata",
            value,
            &CancellationSignal::new(),
        ));
        let decoded = super::super::compaction_resources::take_body_decodes();
        if count == 16 {
            result.unwrap();
            assert_eq!(decoded, 16);
        } else {
            assert!(result.is_err(), "cumulative 16 MiB budget bypassed");
            assert_eq!(decoded, 31, "next body must be rejected before decoding");
            assert!(store.projection_head().unwrap().is_none());
        }
    }
}

#[test]
fn early_lineage_selection_never_hydrates_later_large_checkpoints() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    store
        .commit_projection(commit_input(
            &store,
            "O1",
            candidate(&store, "P1", vec![summary("early")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    for n in 2..=10 {
        store.append_canonical(&message(n)).unwrap();
        store
            .commit_projection(commit_input(
                &store,
                &format!("O{n}"),
                candidate(
                    &store,
                    &format!("P{n}"),
                    vec![summary(&"x".repeat(2 * 1024 * 1024))],
                ),
                &CancellationSignal::new(),
            ))
            .unwrap();
    }
    take_candidate_decodes();
    let early = store
        .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(1))
        .unwrap();
    assert_eq!(take_candidate_decodes(), 1, "later bodies were not decoded");
    assert_eq!(early.projections.len(), 1);
    assert_eq!(early.projections[0].candidate.checkpoint_id.as_str(), "P1");
    let selected = store
        .read_projection_lineage_cut(LedgerFrontier(10), ProjectionActivationId(1))
        .unwrap();
    assert_eq!(selected.projections.len(), 1);
    assert_eq!(
        take_candidate_decodes(),
        1,
        "explicit activation excludes later same-cut bodies"
    );
    let cut = store
        .read_message_lineage_cut(
            crate::conversation::SurfaceRevision::new(10),
            &MessageId::new("M2"),
            false,
        )
        .unwrap();
    assert_eq!(cut.projections.len(), 1);
    assert_eq!(
        take_candidate_decodes(),
        1,
        "ordinary Fork selects before hydration"
    );
}

#[test]
fn metadata_checkpoint_sources_cannot_reset_the_operation_budget() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let mut sources = Vec::new();
    for n in 1..=3 {
        let mut parent = candidate(
            &store,
            &format!("parent-{n}"),
            vec![summary(&"x".repeat(6 * 1024 * 1024))],
        );
        parent.summary_metadata = Some(crate::message::types::CompactionSummaryMetadata::empty());
        let receipt = store
            .commit_projection(commit_input(
                &store,
                &format!("parent-{n}"),
                parent,
                &CancellationSignal::new(),
            ))
            .unwrap();
        sources.push(crate::model::projected::CheckpointSourceRef {
            conversation_id: store.conversation_id().clone(),
            checkpoint_id: receipt.checkpoint.candidate.checkpoint_id,
            content_digest: receipt.candidate_fingerprint,
        });
    }
    let mut item = summary("cumulative metadata");
    let CheckpointMessage::Derived(d) = &mut item.message else {
        unreachable!()
    };
    d.checkpoint_sources = sources;
    let mut value = candidate(&store, "child", vec![item]);
    value.summary_metadata = Some(crate::message::types::CompactionSummaryMetadata::empty());
    let before = store.compaction_resource_cut().unwrap();
    take_candidate_decodes();
    take_metadata_decodes();
    assert!(
        store
            .commit_projection(commit_input(
                &store,
                "child",
                value,
                &CancellationSignal::new()
            ))
            .is_err()
    );
    assert_eq!(
        take_candidate_decodes(),
        1,
        "only CAS reads the head body; metadata extraction hydrates no retired body"
    );
    assert_eq!(
        take_metadata_decodes(),
        2,
        "third parent refused before metadata decode"
    );
    assert_eq!(store.compaction_resource_cut().unwrap(), before);
}

#[test]
fn inherited_activation_coordinates_are_validated_before_atomic_installation() {
    let source = store();
    let canonical = vec![message(1)];
    source.initialize(&canonical).unwrap();
    let checkpoint = source
        .commit_projection(commit_input(
            &source,
            "origin",
            candidate(&source, "origin", vec![summary("origin")]),
            &CancellationSignal::new(),
        ))
        .unwrap()
        .checkpoint;
    for defect in ["future", "before_base", "identity", "order"] {
        let destination =
            SqliteConversationStore::in_memory(source.conversation_id().clone()).unwrap();
        let mut inherited = checkpoint.clone();
        inherited.activation.destination_installation = Some(
            crate::runtime::identity::LineageInstallationId::new("installed"),
        );
        match defect {
            "future" => inherited.activation.ledger = LedgerFrontier(2),
            "before_base" => inherited.activation.ledger = LedgerFrontier(0),
            "identity" => inherited.activation.checkpoint_id = CheckpointId::new("foreign"),
            _ => inherited.activation.activation = ProjectionActivationId(2),
        }
        let seed = crate::durable::LineageSeed::history(canonical.clone())
            .with_projections(vec![inherited])
            .unwrap();
        assert!(
            destination.initialize_lineage(&seed).is_err(),
            "accepted inconsistent {defect}"
        );
        assert!(destination.projection_head().unwrap().is_none());
        assert!(
            destination.load_canonical().unwrap().is_empty(),
            "installation rolled back"
        );
    }
}

#[test]
fn historical_read_cut_with_inconsistent_activation_is_rejected_without_dependencies() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    store
        .commit_projection(commit_input(
            &store,
            "first",
            candidate(&store, "first", vec![summary("first")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    let mut second = candidate(&store, "second", vec![summary("second")]);
    second.read_cut.journal = JournalFrontier(0);
    let before = store.compaction_resource_cut().unwrap();
    assert!(
        store
            .commit_projection(commit_input(
                &store,
                "second",
                second,
                &CancellationSignal::new()
            ))
            .is_err()
    );
    assert_eq!(store.compaction_resource_cut().unwrap(), before);
}

#[test]
fn corrupted_eligibility_header_cannot_hide_a_required_historical_projection() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    store
        .commit_projection(commit_input(
            &store,
            "first",
            candidate(&store, "first", vec![summary("first")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE projection_activations SET ledger_frontier=100 WHERE activation=1",
            [],
        )
        .unwrap();
    assert!(
        store
            .read_lineage_cut(crate::conversation::SurfaceRevision::new(1))
            .is_err()
    );
    assert!(
        store
            .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(1))
            .is_err()
    );
}

#[test]
fn original_and_checkpoint_only_candidates_validate_their_cut_before_commit() {
    for checkpoint_only in [false, true] {
        let store = store();
        let original = message(1);
        store.initialize(std::slice::from_ref(&original)).unwrap();
        let first = store
            .commit_projection(commit_input(
                &store,
                "first",
                candidate(&store, "first", vec![summary("first")]),
                &CancellationSignal::new(),
            ))
            .unwrap();
        let item = if checkpoint_only {
            let mut item = summary("checkpoint-only");
            let CheckpointMessage::Derived(d) = &mut item.message else {
                unreachable!()
            };
            d.checkpoint_sources
                .push(crate::model::projected::CheckpointSourceRef {
                    conversation_id: store.conversation_id().clone(),
                    checkpoint_id: first.checkpoint.candidate.checkpoint_id.clone(),
                    content_digest: first.candidate_fingerprint,
                });
            item
        } else {
            CheckpointItem {
                message: CheckpointMessage::Original(CanonicalMessageRef::of(
                    store.conversation_id(),
                    &original,
                )),
                call_sources: Vec::new(),
                result_call: None,
            }
        };
        let mut value = candidate(&store, "second", vec![item]);
        value.read_cut.ledger = LedgerFrontier(100);
        value.base_frontier = LedgerFrontier(100);
        let before = store.compaction_resource_cut().unwrap();
        assert!(
            store
                .commit_projection(commit_input(
                    &store,
                    "second",
                    value,
                    &CancellationSignal::new()
                ))
                .is_err()
        );
        assert_eq!(store.compaction_resource_cut().unwrap(), before);
        assert_eq!(store.projection_head().unwrap().unwrap(), first.checkpoint);
    }
}

#[test]
fn historical_selection_rejects_missing_eligible_checkpoint_closure() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let first = store
        .commit_projection(commit_input(
            &store,
            "first",
            candidate(&store, "first", vec![summary("first")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    let mut item = summary("second");
    let CheckpointMessage::Derived(d) = &mut item.message else {
        unreachable!()
    };
    d.checkpoint_sources
        .push(crate::model::projected::CheckpointSourceRef {
            conversation_id: store.conversation_id().clone(),
            checkpoint_id: first.checkpoint.candidate.checkpoint_id,
            content_digest: first.candidate_fingerprint,
        });
    store
        .commit_projection(commit_input(
            &store,
            "second",
            candidate(&store, "second", vec![item]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute("DELETE FROM projection_activations WHERE activation=1", [])
        .unwrap();
    assert!(
        store
            .read_projection_lineage_cut(LedgerFrontier(1), ProjectionActivationId(2))
            .is_err()
    );
    assert!(
        store
            .read_lineage_cut(crate::conversation::SurfaceRevision::new(1))
            .is_err()
    );
}

#[test]
fn checkpoint_predecessor_must_match_the_head_visible_at_its_read_cut() {
    for defect in ["zero_activation", "wrong_predecessor", "wrong_journal"] {
        let store = store();
        store.initialize(&[message(1)]).unwrap();
        let first = store
            .commit_projection(commit_input(
                &store,
                "O1",
                candidate(&store, "P1", vec![summary("first")]),
                &CancellationSignal::new(),
            ))
            .unwrap();
        if defect != "zero_activation" {
            store
                .commit_projection(commit_input(
                    &store,
                    "O2",
                    candidate(&store, "P2", vec![summary("second")]),
                    &CancellationSignal::new(),
                ))
                .unwrap();
        }
        let mut value = candidate(&store, "invalid", vec![summary("dependency-free")]);
        value.read_cut.projection_activation = if defect == "zero_activation" {
            ProjectionActivationId(0)
        } else {
            ProjectionActivationId(1)
        };
        if defect == "wrong_predecessor" {
            value.read_cut.journal = first.checkpoint.activation.origin.completed_journal;
        }
        let before = store.compaction_resource_cut().unwrap();
        let head = store.projection_head().unwrap();
        let counts = || {
            let connection = store.lock().unwrap();
            [
                "projection_checkpoints",
                "projection_activations",
                "compaction_operations",
                "events",
            ]
            .map(|table| {
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
                        r.get::<_, u64>(0)
                    })
                    .unwrap()
            })
        };
        let before_counts = counts();
        assert!(
            store
                .commit_projection(commit_input(
                    &store,
                    "invalid",
                    value,
                    &CancellationSignal::new()
                ))
                .is_err(),
            "accepted incoherent {defect}"
        );
        assert_eq!(store.compaction_resource_cut().unwrap(), before);
        assert_eq!(store.projection_head().unwrap(), head);
        assert_eq!(
            counts(),
            before_counts,
            "no partial checkpoint or completion"
        );
    }
}

#[test]
fn historical_head_validation_preserves_growth_same_ledger_and_old_receipts() {
    let store = store();
    store.initialize(&[message(1)]).unwrap();
    let first_input = commit_input(
        &store,
        "O1",
        candidate(&store, "P1", vec![summary("first")]),
        &CancellationSignal::new(),
    );
    assert_eq!(first_input.candidate.expected_head, None);
    assert_eq!(
        first_input.candidate.read_cut.projection_activation,
        ProjectionActivationId(0)
    );
    store.append_canonical(&message(2)).unwrap();
    let first = store.commit_projection(first_input.clone()).unwrap();
    let second_input = commit_input(
        &store,
        "O2",
        candidate(&store, "P2", vec![summary("second")]),
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
                operation_id: CompactionOperationId::new("unrelated"),
            },
        })
        .unwrap();
    store.append_canonical(&message(3)).unwrap();
    let second = store.commit_projection(second_input).unwrap();
    let third = store
        .commit_projection(commit_input(
            &store,
            "O3",
            candidate(&store, "P3", vec![summary("third")]),
            &CancellationSignal::new(),
        ))
        .unwrap();
    assert_eq!(
        second.checkpoint.activation.ledger,
        third.checkpoint.activation.ledger
    );
    assert_ne!(
        second.checkpoint.activation.activation,
        third.checkpoint.activation.activation
    );
    assert_eq!(store.commit_projection(first_input).unwrap(), first);
    assert_eq!(store.projection_head().unwrap().unwrap(), third.checkpoint);
}
