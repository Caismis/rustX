//! Bounded observation regressions: occupancy origin.

use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, JournalStore, ObservationStore, ProjectionStore};
use crate::durable::{ConversationStore, LineageSeed};
use crate::events::types::RuntimeEventEnvelope;
use crate::runtime::identity::{AttemptId, ConversationId, MessageId};
use std::sync::atomic::Ordering;

use crate::runtime::{TokenMeasurement, TokenMeasurementSource};
use crate::runtime_client::RuntimeClientError;
// Exact immutable native occupancy origins, independent of newer display readings.
pub(super) fn occupancy_origin_cut(
    store: &SqliteConversationStore,
) -> crate::observation::cut::CheckpointReadCut {
    let source = store.observation_read_cut().unwrap();
    crate::observation::cut::CheckpointReadCut {
        canonical_foreground: None,
        live_publication: None,
        conversation_id: store.conversation_id().clone(),
        activation_id: None,
        revision: source.read.journal,
        through: source.read.journal,
        source_cut: Some(source),
        domain_revisions: std::collections::BTreeMap::new(),
        collection_cuts: std::collections::BTreeMap::new(),
    }
}

pub(super) fn occupancy_origin_read_model(
    store: &SqliteConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    domain: &str,
) -> String {
    use base64::Engine as _;
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = crate::runtime_client::occupancy::value_read(
            store,
            cut,
            domain,
            0,
            &[],
            offset,
            if offset == 0 { 3 } else { 65_536 },
        )
        .unwrap();
        assert_eq!(&chunk.cut, cut);
        assert_eq!(chunk.domain, domain);
        assert_eq!(chunk.item_offset, 0);
        assert!(chunk.path.is_empty());
        assert_eq!(chunk.offset, offset);
        assert_eq!(
            chunk.representation,
            crate::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1
        );
        let original = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert!(!original.is_empty());
        assert!(original.len() <= 65_536);
        if offset == 0 {
            assert_eq!(
                original,
                [b'"', 0xef, 0xbb],
                "the first chunk deliberately splits the original BOM's UTF-8 bytes"
            );
        }
        bytes.extend(original);
        let Some(next) = chunk.next_offset else {
            assert_eq!(bytes.len() as u64, chunk.readable_end);
            let eof = crate::runtime_client::occupancy::value_read(
                store,
                cut,
                domain,
                0,
                &[],
                chunk.readable_end,
                65_536,
            )
            .unwrap();
            assert!(eof.bytes_base64.is_empty());
            assert!(eof.next_offset.is_none());
            assert!(
                crate::runtime_client::occupancy::value_read(
                    store,
                    cut,
                    domain,
                    0,
                    &[],
                    chunk.readable_end + 1,
                    65_536
                )
                .is_err()
            );
            break;
        };
        assert!(next > offset);
        assert_eq!(next, bytes.len() as u64);
        offset = next;
    }
    serde_json::from_slice(&bytes).unwrap()
}

pub(super) fn occupancy_origin_compact(
    store: &SqliteConversationStore,
    predecessor: &str,
    summary: &str,
    occupancy: &crate::context::occupancy::ContextOccupancy,
) -> RuntimeEventEnvelope {
    let _ = predecessor;
    let history = store.projected_history().unwrap();
    let candidate = crate::scripted_suites::support::context::checkpoint_fixture(
        store,
        summary,
        history.messages.len(),
        "native summary",
    );
    let mut input =
        crate::scripted_suites::support::context::checkpoint_commit_fixture(store, candidate, None);
    input.tokens_before = TokenMeasurement {
        input_tokens: 400,
        source: TokenMeasurementSource::Estimated,
    };
    input.estimated_tokens_after = occupancy.input_tokens;
    input.occupancy = Some(occupancy.clone());
    store.commit_projection(input).unwrap().completed
}

pub(super) fn occupancy_origin_native(
    model: &str,
    input_tokens: u64,
) -> crate::context::occupancy::ContextOccupancy {
    crate::context::occupancy::ContextOccupancy {
        estimated: true,
        input_tokens,
        context_window_tokens: 8_192,
        model: model.into(),
        breakdown: crate::context::occupancy::ContextBreakdown {
            system_tokens: 7,
            tool_tokens: 11,
            message_tokens: input_tokens - 18,
        },
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "One immutable native origin and its replacement exercise every public read fence."
)]
pub(super) fn t08_t12_t14_t19_compaction_occupancy_model_keeps_its_original_journal_origin() {
    let store = store();
    store
        .initialize(&[user_message("origin-input", "native")])
        .unwrap();
    let original = occupancy_origin_native(&"\u{feff}🙂水original".repeat(20_000), 200);
    let first = occupancy_origin_compact(&store, "origin-input", "origin-summary-one", &original);
    let first_cut = occupancy_origin_cut(&store);
    let first_reading = store
        .observation_occupancy(first.sequence)
        .unwrap()
        .unwrap();
    assert_eq!(first_reading.input_tokens, original.input_tokens);
    assert_eq!(first_reading.breakdown, original.breakdown);
    assert!(first_reading.model.is_none());
    assert!(first_reading.model_deferred);
    let domain = first_reading.model_source.as_deref().unwrap();
    assert_eq!(
        domain,
        format!("context.occupancy.{}.model", first.sequence)
    );
    let replacement = occupancy_origin_native(&"\u{feff}水🙂replacement".repeat(20_000), 300);
    let second = occupancy_origin_compact(
        &store,
        "origin-summary-one",
        "origin-summary-two",
        &replacement,
    );
    let latest_cut = occupancy_origin_cut(&store);
    let current = store
        .observation_occupancy(second.sequence)
        .unwrap()
        .unwrap();
    assert_eq!(current.input_tokens, replacement.input_tokens);
    assert_ne!(current.model_source, first_reading.model_source);
    assert_eq!(
        store.observation_occupancy(first.sequence).unwrap(),
        Some(first_reading.clone())
    );
    let request_reads = store.request_snapshot_body_reads.load(Ordering::SeqCst);
    let message_reads = store.checkpoint_read_counts[0].load(Ordering::SeqCst);
    assert_eq!(
        occupancy_origin_read_model(&store, &first_cut, domain),
        original.model
    );
    assert_eq!(
        occupancy_origin_read_model(&store, &latest_cut, domain),
        original.model,
        "newer readable authority cannot retag an immutable old source"
    );
    assert_eq!(
        occupancy_origin_read_model(
            &store,
            &latest_cut,
            current.model_source.as_deref().unwrap()
        ),
        replacement.model
    );
    assert_eq!(
        store.request_snapshot_body_reads.load(Ordering::SeqCst),
        request_reads
    );
    assert_eq!(
        store.checkpoint_read_counts[0].load(Ordering::SeqCst),
        message_reads
    );
    let read = |cut, domain, item, path: &[String]| {
        crate::runtime_client::occupancy::value_read(&store, cut, domain, item, path, 0, 64)
    };
    assert!(read(&first_cut, current.model_source.as_deref().unwrap(), 0, &[]).is_err());
    assert!(matches!(
        read(&first_cut, domain, 1, &[]),
        Err(RuntimeClientError::InvalidRequest { .. })
    ));
    assert!(matches!(
        read(&first_cut, domain, 0, &["model".into()]),
        Err(RuntimeClientError::InvalidRequest { .. })
    ));
    assert!(matches!(
        read(&first_cut, "context.occupancy.latest.model", 0, &[]),
        Err(RuntimeClientError::InvalidRequest { .. })
    ));
    let mut missing = first_cut.clone();
    missing.source_cut = None;
    assert!(matches!(
        read(&missing, domain, 0, &[]),
        Err(RuntimeClientError::ReadCutExpired)
    ));
    let mut foreign = first_cut.clone();
    foreign.conversation_id = ConversationId::generate();
    assert!(matches!(
        read(&foreign, domain, 0, &[]),
        Err(RuntimeClientError::ReadCutExpired)
    ));
    assert!(
        store
            .observation_occupancy_model_value_read(Some(0), second.sequence, 0, 64)
            .is_err()
    );
    assert!(
        store
            .observation_occupancy_model_value_read(
                Some(second.sequence + 1),
                second.sequence,
                0,
                64
            )
            .is_err()
    );
    assert!(
        store
            .observation_occupancy_model_value_read(Some(first.sequence), second.sequence, 0, 0)
            .is_err()
    );
    assert!(
        store
            .observation_occupancy_model_value_read(
                Some(first.sequence),
                second.sequence,
                0,
                65_537
            )
            .is_err()
    );
}

#[test]
fn t08_t14_t19_inherited_occupancy_model_remains_immutable_after_local_compaction() {
    use crate::durable::reading::{
        ConversationTurnId, InheritedTurnOutcome, TurnExecution, TurnReadingProvenance,
    };
    let store = store();
    let original = occupancy_origin_native(&"\u{feff}🙂水inherited".repeat(90_000), 200);
    let inherited = TurnReadingProvenance {
        id: ConversationTurnId {
            conversation_id: ConversationId::generate(),
            attempt_id: AttemptId::new("origin-inherited-attempt"),
        },
        process_message_ids: vec![MessageId::new("origin-inherited-answer")],
        preceding_message_id: None,
        prompt_message_id: Some(MessageId::new("origin-inherited-message")),
        outcome: InheritedTurnOutcome::Completed,
        started_at: None,
        ended_at: None,
        execution: Some(TurnExecution {
            model_requests: 1,
            occupancy: Some(original.clone()),
            ..TurnExecution::default()
        }),
    };
    store
        .initialize_lineage(
            &LineageSeed::history(vec![
                user_message("origin-inherited-message", "native original"),
                assistant_message("origin-inherited-answer", "native inherited answer"),
            ])
            .with_turns(vec![inherited])
            .unwrap(),
        )
        .unwrap();
    let original_cut = occupancy_origin_cut(&store);
    let reading = store.observation_occupancy(0).unwrap().unwrap();
    assert_eq!(reading.input_tokens, original.input_tokens);
    assert_eq!(reading.breakdown, original.breakdown);
    assert!(reading.model_deferred);
    assert!(reading.model.is_none());
    assert_eq!(
        reading.model_source.as_deref(),
        Some("context.occupancy.inherited.model")
    );
    let replacement = occupancy_origin_native("local-model", 300);
    let local = occupancy_origin_compact(
        &store,
        "origin-inherited-message",
        "origin-local-summary",
        &replacement,
    );
    assert_eq!(
        store
            .observation_occupancy(local.sequence)
            .unwrap()
            .unwrap()
            .model
            .as_deref(),
        Some("local-model")
    );
    assert_eq!(store.observation_occupancy(0).unwrap(), Some(reading));
    assert_eq!(
        occupancy_origin_read_model(&store, &original_cut, "context.occupancy.inherited.model"),
        original.model
    );
    assert_eq!(
        occupancy_origin_read_model(
            &store,
            &occupancy_origin_cut(&store),
            "context.occupancy.inherited.model"
        ),
        original.model
    );
    assert!(
        store
            .observation_occupancy_model_value_read(Some(local.sequence), 0, 0, 64)
            .is_err()
    );
}

#[test]
fn t10_t14_t19_compaction_capture_fault_preserves_original_canonical_and_journal_commit() {
    let store = store();
    store
        .initialize(&[user_message("capture-fault-input", "original")])
        .unwrap();
    let before = store.load_head().unwrap().revision;
    let _capture_fault = observation_occupancy::compaction_capture_fault();
    let event = occupancy_origin_compact(
        &store,
        "capture-fault-input",
        "capture-fault-summary",
        &occupancy_origin_native("native-model", 200),
    );
    assert_eq!(store.load_head().unwrap().revision, before);
    assert_eq!(
        store.load_canonical().unwrap(),
        vec![user_message("capture-fault-input", "original")]
    );
    assert!(store.projection_head().unwrap().is_some());
    let retained = store.read_events(Some(event.sequence - 1), 1).unwrap();
    assert_eq!(retained.events[0].event, event.event);
    let witness = store
        .observation_event_witness(event.sequence, event.sequence)
        .unwrap()
        .unwrap();
    assert!(witness.source_unavailable);
    assert!(witness.occupancy.is_none());
    assert_eq!(witness.event_kind, "compaction_completed");
    assert_eq!(witness.sequence, event.sequence);
}

#[test]
fn t10_t14_t19_compaction_witness_write_fault_preserves_original_native_transaction() {
    let store = store();
    store
        .initialize(&[user_message("witness-fault-input", "original")])
        .unwrap();
    let before = store.load_head().unwrap().revision;
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER refuse_compaction_witness BEFORE UPDATE OF observation_witness ON events BEGIN SELECT RAISE(ABORT,'derived witness unavailable'); END;").unwrap();
    let event = occupancy_origin_compact(
        &store,
        "witness-fault-input",
        "witness-fault-summary",
        &occupancy_origin_native("native-model", 200),
    );
    assert_eq!(store.load_head().unwrap().revision, before);
    assert_eq!(
        store.load_canonical().unwrap(),
        vec![user_message("witness-fault-input", "original")]
    );
    assert!(store.projection_head().unwrap().is_some());
    let retained = store.read_events(Some(event.sequence - 1), 1).unwrap();
    assert_eq!(retained.events[0].event, event.event);
    assert!(
        store
            .observation_event_witness(event.sequence, event.sequence)
            .unwrap()
            .is_none()
    );
    let unavailable: bool = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT observation_unavailable FROM events WHERE sequence=?1",
            [event.sequence],
            |row| row.get(0),
        )
        .unwrap();
    assert!(unavailable);
}
