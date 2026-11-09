use super::*;
use crate::durable::sqlite::{SqliteConversationStore, test_support::*};
use crate::durable::{CanonicalStore, ConversationStore, JournalStore, RequestStore};
use crate::events::types::RuntimeEvent;
use crate::model::{ModelRequestOwner, finish::ModelFinishReason};
use crate::runtime::identity::ConversationId;
use chrono::Utc;

fn encoded<T: Serialize + schemars::JsonSchema>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    crate::observation::structural_json::to_writer(&mut bytes, value).unwrap();
    bytes
}

fn fixture(
    store: &SqliteConversationStore,
    failure: Option<&str>,
    measured: bool,
) -> (u64, u64, Vec<Vec<u8>>) {
    store
        .initialize(&[user_message("terminal-base", "base")])
        .unwrap();
    let mut snapshot = progress_request_snapshot(
        "terminal-owner",
        "1",
        0,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    freeze_test_input(store, &mut snapshot, &[]);
    let start = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started
        .sequence;
    let usage = measured.then_some(crate::model::ModelUsage {
        input_tokens: Some(u64::MAX),
        output_tokens: Some(3),
        total_tokens: None,
        details: None,
    });
    let generation = measured.then_some(crate::model::generation_evidence::GenerationEvidence {
        dispatch_after_start_ms: Some(2),
        first_output_ms: Some(3),
        last_output_ms: Some(4),
        terminal_ms: 9,
    });
    let owner = ModelRequestOwner::Agent {
        identity: snapshot.identity.clone(),
    };
    let event = if let Some(message) = failure {
        let mut error = crate::model::ModelError::timeout(
            crate::model::deadline::ModelTimeoutPhase::ResponseStart,
        );
        error.message = message.into();
        RuntimeEvent::ModelRequestFailed {
            owner,
            request_id: snapshot.request_id.clone(),
            error,
            usage,
            generation,
        }
    } else {
        RuntimeEvent::ModelRequestCompleted {
            owner,
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage,
            generation,
        }
    };
    let source = crate::observation::trace::frozen_request_outcome(&event).unwrap();
    let expected = vec![
        encoded(&source),
        encoded(&source.failure),
        encoded(&source.failure.as_ref().map(BorrowedFailure::message)),
        encoded(source.usage),
        encoded(&source.generation),
    ];
    let mut event = envelope(
        store.conversation_id(),
        "terminal-outcome",
        Some(snapshot.identity.attempt_id),
        event,
    );
    event.turn_id = Some(snapshot.identity.turn);
    let terminal = store.append_event(event).unwrap().sequence;
    (start, terminal, expected)
}

fn read_fields(store: &SqliteConversationStore, start: u64, terminal: u64, expected: &[Vec<u8>]) {
    let paths = [
        vec![],
        vec!["failure".into()],
        vec!["failure".into(), "message".into(), "text".into()],
        vec!["usage".into()],
        vec!["generation".into()],
    ];
    let connection = store.lock().unwrap();
    assert!(
        outcome_at(&connection, start, terminal - 1)
            .unwrap()
            .is_none()
    );
    for (path, expected) in paths.iter().zip(expected) {
        let mut output = Vec::new();
        let mut offset = 0;
        loop {
            let chunk =
                outcome_read_at(&connection, start, terminal, path, offset, 65_536).unwrap();
            assert_eq!(usize::try_from(chunk.readable_end).unwrap(), expected.len());
            output.extend(chunk.bytes);
            let Some(next) = chunk.next_offset else { break };
            assert_eq!(next, chunk.end_offset);
            offset = next;
        }
        assert_eq!(&output, expected);
        for offset in [0, 1, 65_520, 65_535, 65_536, expected.len() as u64] {
            if offset > expected.len() as u64 {
                continue;
            }
            let chunk = outcome_read_at(&connection, start, terminal, path, offset, 31).unwrap();
            let end = (usize::try_from(offset).unwrap() + 31).min(expected.len());
            assert_eq!(chunk.bytes, expected[usize::try_from(offset).unwrap()..end]);
            assert_eq!(usize::try_from(chunk.end_offset).unwrap(), end);
            assert_eq!(
                chunk.next_offset,
                (end < expected.len()).then_some(end as u64)
            );
        }
        let empty =
            outcome_read_at(&connection, start, terminal, path, expected.len() as u64, 1).unwrap();
        assert!(empty.bytes.is_empty());
        assert_eq!(empty.next_offset, None);
        for limit in [0, 65_537] {
            assert!(matches!(
                outcome_read_at(&connection, start, terminal, path, 0, limit),
                Err(ConversationStoreError::InvalidContentRange)
            ));
        }
        assert!(matches!(
            outcome_read_at(
                &connection,
                start,
                terminal,
                path,
                expected.len() as u64 + 1,
                1
            ),
            Err(ConversationStoreError::InvalidContentRange)
        ));
    }
}

#[test]
fn terminal_fields_share_one_exact_body_including_null_and_large_failure_ranges() {
    let large = "\u{feff}escaped \"\\\n🙂水".repeat(12_000);
    for (failure, measured) in [
        (None, false),
        (None, true),
        (Some(""), false),
        (Some(large.as_str()), true),
    ] {
        let store = store();
        let (start, terminal, expected) = fixture(&store, failure, measured);
        read_fields(&store, start, terminal, &expected);
        let connection = store.lock().unwrap();
        let (rows, maximum): (u64, u64) = connection.query_row(
            "SELECT count(*),max(block_index) FROM content_frontiers WHERE source_kind='trace_terminal'", [], |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!((rows, maximum), (1, 0));
        let bytes: Vec<u8> = connection.query_row("SELECT observation_terminal_header FROM request_snapshots WHERE started_sequence=?1", [start], |row| row.get(0)).unwrap();
        let stored: StoredOutcome = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            usize::try_from(stored.header.encoded_bytes).unwrap(),
            expected[0].len()
        );
        assert_eq!(
            stored.header.failure_deferred,
            failure.is_some_and(|message| message.chars().nth(256).is_some())
        );
    }
}

#[test]
fn terminal_ranges_reopen_and_detect_current_body_and_geometry_failure_after_rollback() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("terminal.sqlite");
    let id = ConversationId::generate();
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    let (start, terminal, expected) = fixture(&store, Some("original failure🙂"), true);
    drop(store);
    let reader = SqliteConversationStore::open_existing(id.clone(), &path).unwrap();
    read_fields(&reader, start, terminal, &expected);
    drop(reader);
    let store = SqliteConversationStore::open(id, &path).unwrap();
    {
        let connection = store.lock().unwrap();
        connection.execute_batch("SAVEPOINT missing_body; DELETE FROM content_frontiers WHERE source_kind='trace_terminal';").unwrap();
        assert!(outcome_read_at(&connection, start, terminal, &[], 0, 1).is_err());
        connection
            .execute_batch("ROLLBACK TO missing_body; RELEASE missing_body;")
            .unwrap();
        let bytes: Vec<u8> = connection.query_row("SELECT observation_terminal_header FROM request_snapshots WHERE started_sequence=?1", [start], |row| row.get(0)).unwrap();
        let mut stored: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        stored["fields"][2]["end"] = serde_json::json!(u64::MAX);
        connection
            .execute_batch("SAVEPOINT invalid_geometry")
            .unwrap();
        connection.execute("UPDATE request_snapshots SET observation_terminal_header=?2 WHERE started_sequence=?1", params![start,serde_json::to_vec(&stored).unwrap()]).unwrap();
        assert!(outcome_read_at(&connection, start, terminal, &["usage".into()], 0, 1).is_err());
        connection
            .execute_batch("ROLLBACK TO invalid_geometry; RELEASE invalid_geometry;")
            .unwrap();
        // Optional outcome fields stay independent from the original event body.
        connection
            .authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| {
                if matches!(
                    context.action,
                    rusqlite::hooks::AuthAction::Read {
                        table_name: "events",
                        column_name: "event_json"
                    }
                ) {
                    rusqlite::hooks::Authorization::Deny
                } else {
                    rusqlite::hooks::Authorization::Allow
                }
            }))
            .unwrap();
    }
    read_fields(&store, start, terminal, &expected);
}
