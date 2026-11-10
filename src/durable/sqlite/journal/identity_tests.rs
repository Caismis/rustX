//! Public aliases retain exact identity while ordinary appends use sequence.
use super::*;
use crate::durable::sqlite::test_support::{envelope, store};
use crate::durable::{ConversationStore, InboxStore, JournalStore};

fn append(
    store: &SqliteConversationStore,
    id: &str,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    let mut event = envelope(store.conversation_id(), id, None, RuntimeEvent::TurnStarted);
    event.turn_id = Some(crate::runtime::identity::TurnId::new(
        (store.event_high_watermark()? + 1).to_string(),
    ));
    store.append_event(event)
}

#[test]
fn numeric_identity_keeps_custom_aliases_and_future_default_collisions() {
    let store = store();
    let first = append(&store, "").unwrap();
    assert_eq!(
        first.event_id.as_str(),
        format!("{}-event-1", store.conversation_id())
    );
    let second = append(&store, "producer🙂").unwrap();
    let future = format!("{}-event-4", store.conversation_id());
    let third = append(&store, &future).unwrap();
    assert_eq!(third.sequence, 3);
    // The custom third row has reserved the fourth row's default spelling.
    assert!(append(&store, "").is_err());
    assert_eq!(store.event_high_watermark().unwrap(), 3);
    assert!(append(&store, first.event_id.as_str()).is_err());
    assert!(append(&store, second.event_id.as_str()).is_err());
    let mut connection = store.lock().unwrap();
    let transaction = connection.transaction().unwrap();
    for event in [&first, &second, &third] {
        let found = find_event_by_id(&transaction, &event.event_id)
            .unwrap()
            .unwrap();
        assert_eq!(found.event_id, event.event_id);
        assert_eq!(found.sequence, event.sequence);
    }
    for id in [
        "",
        "producer",
        "foreign-event-1",
        "missing-event-0001",
        "missing-event-9223372036854775808",
    ] {
        assert!(
            find_event_by_id(&transaction, &EventId::new(id))
                .unwrap()
                .is_none()
        );
    }
    transaction
        .execute_batch("SAVEPOINT corrupt_alias")
        .unwrap();
    transaction
        .execute(
            "UPDATE events SET event_id=?1 WHERE sequence=?2",
            params![first.event_id.as_str(), second.sequence],
        )
        .unwrap();
    assert!(find_event_by_id(&transaction, &first.event_id).is_err());
    transaction
        .execute_batch("ROLLBACK TO corrupt_alias; RELEASE corrupt_alias")
        .unwrap();
    transaction.rollback().unwrap();
    drop(connection);
    assert_eq!(store.read_events(None, 8).unwrap().events.len(), 3);
}

#[test]
fn default_event_alias_tree_stays_empty_and_identity_seeks_stay_bounded() {
    use rusqlite::StatementStatus;
    let store = store();
    for history in [8, 64, 512, 4096] {
        while store.event_high_watermark().unwrap() < history {
            append(&store, "").unwrap();
        }
        let connection = store.lock().unwrap();
        let cells: u64 = connection
            .query_row(
                "SELECT coalesce(sum(ncell),0) FROM dbstat WHERE name='events_alias_idx'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            cells, 0,
            "H={history}: default events must not enter the alias tree"
        );
        let default = format!("{}-event-{history}", store.conversation_id());
        let mut seek = connection.prepare(EVENT_SEQUENCE_SQL).unwrap();
        let found: u64 = seek
            .query_row(params![default, history], |row| row.get(0))
            .unwrap();
        assert_eq!(found, history);
        assert_eq!(seek.get_status(StatementStatus::FullscanStep), 0);
        assert_eq!(seek.get_status(StatementStatus::Sort), 0);
        assert!(seek.get_status(StatementStatus::VmStep) < 80);
    }
}

#[test]
fn terminal_guard_keeps_precedence_over_a_repeated_event_alias() {
    let store = store();
    let attempt = crate::runtime::identity::AttemptId::new("terminal-owner");
    store
        .append_event(envelope(
            store.conversation_id(),
            "start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let terminal = envelope(
        store.conversation_id(),
        "terminal",
        Some(attempt.clone()),
        RuntimeEvent::AttemptTimedOut {
            attempt_id: attempt,
        },
    );
    store.append_event(terminal.clone()).unwrap();
    assert!(matches!(
        store.append_event(terminal),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
}

#[test]
fn transcript_identity_lookup_preserves_decode_error_domain() {
    for id in ["", "producer-transcript"] {
        let store = store();
        let event = append(&store, id).unwrap();
        let connection = store.lock().unwrap();
        connection
            .execute(
                "UPDATE events SET event_json='{}' WHERE sequence=?1",
                [event.sequence],
            )
            .unwrap();
        let error = crate::durable::sqlite::canonical::transcript::load_transcript_event(
            &connection,
            "attempt_terminal",
            event.event_id.as_str(),
        )
        .unwrap_err();
        let expected = decode::<RuntimeEventEnvelope>("{}", "transcript event").unwrap_err();
        assert_eq!(format!("{error:?}"), format!("{expected:?}"));
    }
}

#[test]
fn physical_settlement_identity_lookup_preserves_decode_error_domain() {
    use crate::durable::sqlite::test_support::{own_normal_subagent, success_publication};
    use crate::runtime::identity::{AgentId, SubagentId};

    let store = store();
    let child = AgentId::new("decode-domain-child");
    let subagent = SubagentId::for_conversation(store.conversation_id(), 1);
    own_normal_subagent(&store, &subagent, &child);
    let (notice, report, terminal) = success_publication(&store, &subagent, &child, "result");
    let terminal_id = terminal.event_id.clone();
    store
        .accept_subagent_terminal(Some(notice), report, terminal)
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE events SET event_json='{}' WHERE event_id=?1",
            [terminal_id.as_str()],
        )
        .unwrap();
    let proof = envelope(
        store.conversation_id(),
        crate::runtime::subagent::physical_settlement_event_id(&subagent).as_str(),
        None,
        RuntimeEvent::SubagentPhysicalSettlementProven {
            subagent_id: subagent,
            child_agent_id: child,
        },
    );
    let error = store.append_event(proof).unwrap_err();
    let expected =
        decode::<RuntimeEventEnvelope>("{}", "physical settlement terminal").unwrap_err();
    assert_eq!(format!("{error:?}"), format!("{expected:?}"));
}
