//! The persisted recovery frontier is exact, atomic and fails closed.
use super::super::test_support::*;
use super::super::*;
use crate::durable::CanonicalStore;
use crate::durable::sqlite::tests::observation_response_tests::response_start;
use crate::runtime::recovery::RecoveryEvidence;

/// Three completed tool turns followed by one open attempt.
fn history() -> SqliteConversationStore {
    let store = store();
    store.initialize(&[]).unwrap();
    super::super::append_completed_turns(&store, 0, 3, true);
    response_start(&store, "open");
    store
}

fn frontier(store: &SqliteConversationStore) -> serde_json::Value {
    let connection = store.lock().unwrap();
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT recovery_frontier FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn replace_frontier(store: &SqliteConversationStore, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut value = frontier(store);
    edit(&mut value);
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE rustx_store SET recovery_frontier=?1 WHERE id=1",
            [serde_json::to_vec(&value).unwrap()],
        )
        .unwrap();
}

fn next_event_sequence(store: &SqliteConversationStore) -> u64 {
    store
        .lock()
        .unwrap()
        .query_row(
            "SELECT next_event_sequence FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn rejected(store: &SqliteConversationStore, detail: &str) {
    let error = RecoveryEvidence::reconstruct(store)
        .unwrap_err()
        .to_string();
    assert!(error.contains(detail), "{error}");
}

#[test]
fn every_native_append_advances_the_frontier_exactly() {
    let store = history();
    let value = frontier(&store);
    assert_eq!(value["through"], next_event_sequence(&store));
    assert_eq!(value["attempts"].as_object().unwrap().len(), 1);
    assert!(value["repairs"].as_array().unwrap().is_empty());
    // The debug oracle recomputes the whole fold inside reconstruction.
    RecoveryEvidence::reconstruct(&store).unwrap();
}

#[test]
fn the_frontier_commits_and_rolls_back_with_its_fact() {
    let store = history();
    let before = frontier(&store);
    let sequence = next_event_sequence(&store);
    for commit in [false, true] {
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        let attempt = AttemptId::new(format!("atomic-{commit}"));
        super::super::journal::persist_event_tx(
            &store.conn.event_guard,
            &transaction,
            store.conversation_id(),
            envelope(
                store.conversation_id(),
                "",
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt,
                },
            ),
        )
        .unwrap();
        let inside: Vec<u8> = transaction
            .query_row(
                "SELECT recovery_frontier FROM rustx_store WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let inside: serde_json::Value = serde_json::from_slice(&inside).unwrap();
        assert_eq!(inside["through"], sequence + 1);
        if commit {
            transaction.commit().unwrap();
        } else {
            drop(transaction);
        }
        drop(connection);
        if commit {
            assert_eq!(frontier(&store)["through"], sequence + 1);
            assert_eq!(next_event_sequence(&store), sequence + 1);
        } else {
            assert_eq!(frontier(&store), before);
            assert_eq!(next_event_sequence(&store), sequence);
        }
    }
    RecoveryEvidence::reconstruct(&store).unwrap();
}

#[test]
fn a_frontier_ahead_of_the_journal_fails_closed() {
    let store = history();
    let ahead = next_event_sequence(&store) + 3;
    replace_frontier(&store, |value| value["through"] = ahead.into());
    rejected(&store, "frontier is ahead of the Journal");
}

#[test]
fn a_frontier_naming_another_row_fails_closed() {
    let store = history();
    replace_frontier(&store, |value| value["through_event"] = "other".into());
    rejected(&store, "anchor row identity");
    replace_frontier(&store, |value| {
        value["through_event"] = serde_json::Value::Null;
    });
    rejected(&store, "anchor identity");
}

#[test]
fn a_missing_anchor_row_fails_closed() {
    let store = history();
    let through = next_event_sequence(&store);
    {
        let connection = store.lock().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        connection
            .execute("DELETE FROM events WHERE sequence=?1", [through])
            .unwrap();
    }
    rejected(&store, "anchor row is missing");
}

#[test]
fn open_work_must_name_its_exact_journal_fact() {
    let store = history();
    replace_frontier(&store, |value| {
        let attempts = value["attempts"].as_object_mut().unwrap();
        let open = attempts.values_mut().next().unwrap();
        open["started"] = 1.into();
    });
    rejected(&store, "open attempt start");
}

#[test]
fn a_malformed_frontier_fails_closed() {
    let store = history();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE rustx_store SET recovery_frontier=?1 WHERE id=1",
            [b"{\"through\":".to_vec()],
        )
        .unwrap();
    rejected(&store, "recovery frontier is malformed");
}

#[test]
#[should_panic(expected = "disagrees with the full Journal fold")]
fn tampered_fold_state_is_caught_by_the_debug_oracle() {
    let store = history();
    replace_frontier(&store, |value| value["highest_attempt_ordinal"] = 99.into());
    let _ = RecoveryEvidence::reconstruct(&store);
}

#[test]
#[should_panic(expected = "disagrees with the full Journal fold")]
fn a_journal_row_missing_behind_the_frontier_is_caught_by_the_debug_oracle() {
    let store = history();
    {
        // The first completed attempt's terminal fact disappears: the full
        // fold would reopen that attempt, the frontier has it closed.
        let connection = store.lock().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        let terminal: u64 = connection
            .query_row(
                "SELECT min(sequence) FROM events WHERE json_extract(event_json,'$.event.type')='attempt_completed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute("DELETE FROM events WHERE sequence=?1", [terminal])
            .unwrap();
    }
    let _ = RecoveryEvidence::reconstruct(&store);
}
