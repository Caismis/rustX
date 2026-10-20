//! Bounded observation regressions of the SQLite store.

use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{CanonicalStore, JournalStore, ObservationStore};
use crate::events::types::RuntimeEvent;
use crate::runtime::identity::{AttemptId, ConversationId, MessageId, TurnId};
use std::sync::Arc;

mod event_probe;
mod indexed_fact;
mod inspection;
mod member;
mod occupancy;
mod occupancy_origin;
mod owner;
mod projection;
mod scale;
mod selected_source;
mod statistics;
mod trace;

#[test]
fn t10_t19_t20_readiness_layout_failure_never_refuses_native_journal_commit() {
    struct Observer;
    impl crate::durable::presentation::JournalObserver for Observer {
        fn committed(&self, _through: Option<u64>) {}
    }
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936174");
    let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
    let make = |label: &str| {
        let mut event = envelope(
            &id,
            label,
            Some(AttemptId::new("readiness-failure")),
            RuntimeEvent::TurnStarted,
        );
        event.turn_id = Some(TurnId::new("1"));
        event
    };
    store.append_event(make("first")).unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER readiness_layout_failure BEFORE INSERT ON event_observation_readiness WHEN NEW.sequence=2 BEGIN SELECT RAISE(FAIL,'derived readiness failure'); END;").unwrap();
    let second = store.append_event(make("second")).unwrap();
    assert_eq!(second.sequence, 2, "original native Journal committed");
    assert!(second.observation.as_ref().unwrap().source_unavailable);
    let flags: (bool, bool) = store.lock().unwrap().query_row(
        "SELECT observation_readiness_missing,observation_unavailable FROM events WHERE sequence=2",
        [], |row| Ok((row.get(0)?,row.get(1)?)),
    ).unwrap();
    assert_eq!(
        flags,
        (true, false),
        "only the readiness owner needs repair"
    );
    assert!(
        store
            .observation_event_witness(2, 2)
            .unwrap()
            .unwrap()
            .source_unavailable
    );
    store.mark_observation_direct(1, 1).unwrap();
    assert_eq!(store.observation_ready_frontier(0, 2).unwrap(), 1);
    assert!(store.mark_observation_ready(2, 2).is_err());
    assert!(store.mark_observation_direct(2, 2).is_err());
    assert!(
        !store
            .observation_event_witness(2, 2)
            .unwrap()
            .unwrap()
            .native_direct
    );
    assert!(
        store.observe_journal(Arc::new(Observer)).is_err(),
        "failed repair remains unavailable"
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER readiness_layout_failure")
        .unwrap();
    assert_eq!(store.observe_journal(Arc::new(Observer)).unwrap(), 2);
    assert_eq!(store.observation_ready_frontier(0, 2).unwrap(), 2);
    let recovered = store.observation_event_witness(2, 2).unwrap().unwrap();
    assert!(!recovered.source_unavailable);
    assert!(
        !observation_receipt::single(&store.lock().unwrap(), &id, &second)
            .unwrap()
            .source_unavailable
    );
    assert_eq!(store.read_events(Some(1), 1).unwrap().events, vec![second]);
    assert!(
        !recovered.native_direct,
        "restart reconstructs committed authority"
    );
}

#[test]
fn t10_t19_t20_readiness_repair_preserves_independent_auxiliary_failure() {
    struct Observer;
    impl crate::durable::presentation::JournalObserver for Observer {
        fn committed(&self, _through: Option<u64>) {}
    }
    let id = ConversationId::new("conv_09276f12-ed32-741a-8cd5-91c38800ef71");
    let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
    store.lock().unwrap().execute_batch(
        "CREATE TRIGGER readiness_failure BEFORE INSERT ON event_observation_readiness BEGIN SELECT RAISE(FAIL,'readiness failure'); END;
         CREATE TRIGGER fallback_failure BEFORE UPDATE OF observation_json ON events BEGIN SELECT RAISE(FAIL,'independent fallback failure'); END;",
    ).unwrap();
    let mut event = envelope(
        &id,
        "both-layout-failures",
        Some(AttemptId::new("readiness-owner")),
        RuntimeEvent::TurnStarted,
    );
    event.turn_id = Some(TurnId::new("1"));
    let committed = store.append_event(event).unwrap();
    assert_eq!(committed.sequence, 1);
    assert!(committed.observation.as_ref().unwrap().source_unavailable);
    let flags = |store: &SqliteConversationStore| -> (bool, bool) {
        store.lock().unwrap().query_row(
            "SELECT observation_readiness_missing,observation_unavailable FROM events WHERE sequence=1",
            [], |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap()
    };
    assert_eq!(flags(&store), (true, true));
    assert!(store.observe_journal(Arc::new(Observer)).is_err());
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER readiness_failure; DROP TRIGGER fallback_failure;")
        .unwrap();
    assert_eq!(store.observe_journal(Arc::new(Observer)).unwrap(), 1);
    assert_eq!(store.observation_ready_frontier(0, 1).unwrap(), 1);
    assert_eq!(flags(&store), (false, true));
    assert!(
        store
            .observation_event_witness(1, 1)
            .unwrap()
            .unwrap()
            .source_unavailable
    );
    assert!(
        observation_receipt::single(&store.lock().unwrap(), &id, &committed)
            .unwrap()
            .source_unavailable
    );
    assert_eq!(store.read_events(None, 1).unwrap().events, vec![committed]);
    let mut healthy = envelope(
        &id,
        "healthy-after-repair",
        Some(AttemptId::new("readiness-owner")),
        RuntimeEvent::TurnStarted,
    );
    healthy.turn_id = Some(TurnId::new("2"));
    let next = store.append_event(healthy).unwrap();
    assert_eq!(next.sequence, 2);
    assert!(!next.observation.as_ref().unwrap().source_unavailable);
    assert!(
        !store
            .observation_event_witness(2, 2)
            .unwrap()
            .unwrap()
            .source_unavailable
    );
}

#[test]
fn t10_t19_t20_transcript_readiness_failure_preserves_native_canonical_commit() {
    struct Observer;
    impl crate::durable::presentation::JournalObserver for Observer {
        fn committed(&self, _through: Option<u64>) {}
    }
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936182");
    let store = SqliteConversationStore::in_memory(id).unwrap();
    store.initialize(&[]).unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER transcript_readiness_failure BEFORE INSERT ON transcript_observation_readiness WHEN NEW.position=1 BEGIN SELECT RAISE(FAIL,'derived transcript readiness failure'); END;").unwrap();
    let first = store
        .append_canonical(&user_message("first", "canonical authority"))
        .unwrap();
    assert_eq!(first.ledger_position, 1);
    assert_eq!(first.transcript_cursor.unwrap().get(), 1);
    assert_eq!(
        store
            .message_ledger_position(&MessageId::new("first"))
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        store.observation_transcript_ready_frontier(0, 1).unwrap(),
        0
    );
    assert!(store.mark_observation_transcript_ready(1).is_err());
    assert!(store.mark_canonical_transcript_ready(1, 1).is_err());
    let second = store
        .append_canonical(&user_message("second", "unrelated native commit"))
        .unwrap();
    assert_eq!(second.transcript_cursor.unwrap().get(), 2);
    store.mark_canonical_transcript_ready(2, 2).unwrap();
    assert_eq!(
        store.observation_transcript_ready_frontier(0, 2).unwrap(),
        0
    );
    assert!(store.observe_journal(Arc::new(Observer)).is_err());
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER transcript_readiness_failure")
        .unwrap();
    assert_eq!(store.observe_journal(Arc::new(Observer)).unwrap(), 0);
    assert_eq!(
        store.observation_transcript_ready_frontier(0, 2).unwrap(),
        2
    );
    let connection = store.lock().unwrap();
    let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT MIN(position) FROM transcript_order INDEXED BY transcript_observation_readiness_missing_idx WHERE observation_readiness_missing=1 AND position>0 AND position<=2", [], |row| row.get(3)).unwrap();
    assert!(
        plan.contains("transcript_observation_readiness_missing_idx"),
        "{plan}"
    );
}

#[test]
fn t10_t15_t20_trace_count_hole_is_unavailable_without_refusing_native_commit() {
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936179");
    let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
    let make = |label: &str| {
        let mut event = envelope(
            &id,
            label,
            Some(AttemptId::new("trace-count")),
            RuntimeEvent::TurnStarted,
        );
        event.turn_id = Some(TurnId::new("1"));
        event
    };
    let first = store.append_event(make("first")).unwrap();
    assert_eq!(store.observation_trace_total(first.sequence).unwrap(), 1);
    store.lock().unwrap().execute_batch("CREATE TRIGGER trace_layout_failure BEFORE UPDATE OF observation_trace_total ON events WHEN NEW.sequence=2 BEGIN SELECT RAISE(FAIL,'derived count failure'); END;").unwrap();
    let second = store.append_event(make("second")).unwrap();
    assert_eq!(
        second.sequence, 2,
        "native fact committed despite derived fault"
    );
    assert!(second.observation.unwrap().trace_total.is_none());
    assert!(store.observation_trace_total(2).is_err());
    assert_eq!(store.observation_trace_total(1).unwrap(), 1);
    let connection = store.lock().unwrap();
    let plan:String=connection.query_row("EXPLAIN QUERY PLAN SELECT sequence FROM events INDEXED BY events_observation_trace_holes_idx WHERE observation_trace_required=1 AND observation_trace_total IS NULL AND sequence<=2",[],|row|row.get(3)).unwrap();
    assert!(
        plan.contains("events_observation_trace_holes_idx"),
        "{plan}"
    );
}

#[test]
fn t12_t19_native_failure_receipt_and_range_preserve_public_normalization() {
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936187");
    let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
    let attempt = AttemptId::new("public-failure-source");
    store
        .append_event(envelope(
            &id,
            "start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let error = crate::model::error::ModelError {
        kind: crate::model::error::ModelErrorKind::ProviderError,
        message: "雪".repeat(1000),
        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
        retry_after_ms: Some(73),
        provider_code: Some("private-provider-code".into()),
        context_overflow: None,
        malformed_tool_proposal: None,
        timeout_phase: None,
        generation: None,
    };
    let event = store
        .append_event(envelope(
            &id,
            &"private-native-id".repeat(100_000),
            Some(attempt.clone()),
            RuntimeEvent::AttemptFailed {
                attempt_id: attempt.clone(),
                error: crate::events::types::AttemptFailure::Model { error },
            },
        ))
        .unwrap();
    let headers = store
        .latest_observation_events(2, &["attempt_failed".into()], None, 32)
        .unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].sequence, 2);
    assert!(
        headers[0].event_id.is_none(),
        "metadata seek never hydrates an enormous original Event ID"
    );
    let receipt = event.observation.as_ref().unwrap();
    assert!(receipt.failure_deferred);
    assert_eq!(receipt.failure_sequence, Some(2));
    assert!(receipt.failure_readable_end > 3000);
    let failure = serde_json::to_string(receipt.failure.as_ref().unwrap()).unwrap();
    assert!(!failure.contains("provider_code") && !failure.contains("private-provider-code"));
    let chunk = store
        .observation_failure_value_read(&attempt, 2, 0, 65_536)
        .unwrap();
    assert!(chunk.next_offset.is_none());
    let failure: serde_json::Value = serde_json::from_slice(&chunk.bytes).unwrap();
    assert_eq!(failure["type"], "model");
    assert_eq!(failure["message"].as_str().unwrap().len(), 3000);
    assert_eq!(failure["retry_after_ms"], "73");
    assert!(failure.get("provider_code").is_none());
    assert!(
        serde_json::to_string(&event)
            .unwrap()
            .contains("private-provider-code"),
        "original native Journal authority is retained, not rewritten by display privacy"
    );
}

#[test]
fn t14_t19_t20_transcript_ready_prefix_fences_prior_terminal_without_position_mirror() {
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936181");
    let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
    store.initialize(&[]).unwrap();
    let attempt = AttemptId::new("gated-terminal");
    store
        .append_event(envelope(
            &id,
            "started",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    store
        .append_event(envelope(
            &id,
            "terminal",
            Some(attempt.clone()),
            RuntimeEvent::AttemptTimedOut {
                attempt_id: attempt,
            },
        ))
        .unwrap();
    let messages = (0..73)
        .map(|index| user_message(&format!("after-terminal-{index}"), "canonical"))
        .collect::<Vec<_>>();
    let batch = store.append_canonical_batch(&messages).unwrap();
    assert_eq!(batch.last().unwrap().transcript_cursor.unwrap().get(), 74);
    store.mark_canonical_transcript_ready(1, 73).unwrap();
    assert_eq!(
        store.observation_transcript_ready_frontier(0, 74).unwrap(),
        0,
        "C release cannot release unrelated prior terminalT"
    );
    store.mark_observation_ready(1, 2).unwrap();
    assert_eq!(store.observation_ready_frontier(0, 2).unwrap(), 2);
    assert_eq!(
        store.observation_transcript_ready_frontier(0, 74).unwrap(),
        0,
        "J andT publication evidence remain distinct"
    );
    store.mark_observation_transcript_ready(1).unwrap();
    assert_eq!(
        store.observation_transcript_ready_frontier(0, 74).unwrap(),
        74
    );
    assert!(store.mark_observation_transcript_ready(75).is_err());
    assert!(store.mark_canonical_transcript_ready(1, 74).is_err());
    let connection = store.lock().unwrap();
    let plan:String=connection.query_row("EXPLAIN QUERY PLAN SELECT MIN(position) FROM transcript_observation_readiness WHERE ready=0 AND position>0 AND position<=74",[],|row|row.get(3)).unwrap();
    assert!(
        plan.contains("transcript_observation_readiness_holes_idx"),
        "{plan}"
    );
}
