//! Physical co-location must preserve native and observation authority separately.
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::{observation, query_work};
use crate::durable::{CanonicalStore, ConversationStore, InboxStore, ObservationStore};
use rusqlite::params;

#[test]
fn transcript_carrier_keeps_native_identity_separate_from_observation() {
    let store = store();
    store
        .initialize(&[user_message("one", "first"), user_message("two", "second")])
        .unwrap();
    let cut = store.observation_read_cut().unwrap();
    let connection = store.lock().unwrap();
    let rows: (usize, usize) = connection
        .query_row(
            "SELECT count(*),count(header_json) FROM transcript_entries WHERE position>0",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        rows,
        (2, 2),
        "one physical carrier per native transcript seat"
    );
    connection.execute("UPDATE transcript_entries SET native_reference_id='damaged-native-id' WHERE ledger_position=1", []).unwrap();
    let page =
        observation::messages(&connection, store.conversation_id(), &cut, Some(0), 2).unwrap();
    assert_eq!(
        page.headers[0]
            .identity
            .as_ref()
            .map(crate::runtime::identity::MessageId::as_str),
        Some("one")
    );
    connection
        .execute(
            "UPDATE transcript_entries SET header_json=NULL WHERE ledger_position=2",
            [],
        )
        .unwrap();
    assert!(observation::messages(&connection, store.conversation_id(), &cut, Some(0), 2).is_err());
    drop(connection);
    assert_eq!(store.load_canonical().unwrap().len(), 2);
}

#[test]
fn missing_optional_header_does_not_rollback_native_transcript_or_message() {
    let store = store();
    store.initialize(&[]).unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER reject_optional_header BEFORE UPDATE OF header_json ON transcript_entries BEGIN SELECT RAISE(ABORT,'header domain unavailable'); END").unwrap();
    store
        .append_canonical(&user_message("native", "durable"))
        .unwrap();
    assert_eq!(store.load_canonical().unwrap().len(), 1);
    assert_eq!(
        store.load_transcript_page(None, 8).unwrap().entries.len(),
        1
    );
    let connection = store.lock().unwrap();
    let missing: bool = connection
        .query_row(
            "SELECT header_json IS NULL FROM transcript_entries WHERE native_reference_id='native'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(missing);
}

#[test]
fn transcript_carrier_header_reads_and_append_sql_are_bounded_by_delta() {
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    for history in [8, 64, 512, 2048] {
        let store = store();
        let messages = (0..history)
            .map(|i| {
                let id = format!("m-{i:06}");
                if i % 2 == 0 {
                    request_context_message(&id, "body")
                } else {
                    user_message(&id, "body")
                }
            })
            .collect::<Vec<_>>();
        store.initialize(&messages).unwrap();
        let connection = store.lock().unwrap();
        reads.push((
            history,
            query_work::measure(
                &connection,
                observation::MESSAGE_HEADERS_SQL,
                params![history - 8, history],
            ),
        ));
        query_work::capture(&connection);
        drop(connection);
        store
            .append_canonical(&user_message("next", "body"))
            .unwrap();
        store
            .append_canonical(&request_context_message("next-hidden", "body"))
            .unwrap();
        let work = query_work::finish(&store.lock().unwrap());
        let selected = work
            .into_iter()
            .filter(|(sql, _)| sql.contains("transcript_entries"))
            .map(|(sql, w)| {
                assert_eq!(w.scans, 0, "{sql}");
                assert_eq!(w.sorts, 0, "{sql}");
                (sql, (w.calls, w.steps))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert!(!selected.is_empty());
        writes.push(selected);
    }
    query_work::assert_flat(&reads, 100, 8, 128 * 1024);
    for work in &writes[1..] {
        assert_eq!(work, &writes[0]);
    }
}

#[test]
fn surface_operations_store_one_numeric_ledger_binding_and_keep_old_revisions() {
    let store = store();
    store.initialize(&[user_message("one", "first")]).unwrap();
    store
        .append_canonical(&user_message("two", "second"))
        .unwrap();
    let connection = store.lock().unwrap();
    let bindings = connection
        .prepare("SELECT revision,ledger_position FROM surface_ops ORDER BY revision")
        .unwrap()
        .query_map([], |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(bindings, vec![(1, 1), (2, 2)]);
    let old = crate::durable::sqlite::canonical::ledger::reconstruct_surface(
        &connection,
        crate::conversation::SurfaceRevision::new(1),
    )
    .unwrap();
    assert_eq!(old, vec![crate::runtime::identity::MessageId::new("one")]);
    let columns = connection
        .prepare("SELECT name FROM pragma_table_info('surface_ops') ORDER BY cid")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        columns,
        ["revision", "compaction_generation", "ledger_position"]
    );
}

#[test]
fn context_carriers_never_publish_a_negative_transcript_cursor() {
    let store = store();
    store.initialize(&[]).unwrap();
    let context = request_context_message("hidden", "runtime context");
    store.append_canonical(&context).unwrap();
    let cut = store.observation_read_cut().unwrap();
    let connection = store.lock().unwrap();
    let header = observation::message(&connection, context.id())
        .unwrap()
        .unwrap();
    assert!(header.transcript_cursor.is_none());
    assert_eq!(header.ledger_position, Some(1));
    assert!(observation::messages(&connection, store.conversation_id(), &cut, Some(0), 8).is_ok());
    let negative:bool=connection.query_row("SELECT position<0 AND native_reference_kind IS NULL FROM transcript_entries WHERE ledger_position=1",[],|row|row.get(0)).unwrap();
    assert!(negative);
    drop(connection);
    assert!(
        store
            .load_transcript_page(None, 8)
            .unwrap()
            .entries
            .is_empty()
    );
    store
        .append_canonical(&user_message("visible", "user"))
        .unwrap();
    assert_eq!(
        store.load_transcript_page(None, 8).unwrap().entries.len(),
        1
    );
}

#[test]
fn hidden_context_carriers_do_not_encode_inbound_sequences_in_rowids() {
    for sequence in [1_i64 << 62, i64::MAX] {
        let store = store();
        store.initialize(&[]).unwrap();
        store
            .append_canonical(&request_context_message("canonical-before", "context"))
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE rustx_store SET next_inbound_sequence=?1 WHERE id=1",
                [sequence - 1],
            )
            .unwrap();
        let mut pending = draft("pending context");
        pending.kind = crate::message::types::InboundKind::Context(
            crate::message::types::ContextKind::RuntimeToolObservation,
        );
        pending.source = crate::message::types::UserSource::Runtime;
        let accepted = store.accept_inbound(pending).unwrap();
        assert_eq!(accepted.sequence.get(), u64::try_from(sequence).unwrap());
        assert!(accepted.transcript_cursor.is_none());
        let receipt = accepted
            .pending_observation
            .as_ref()
            .expect("large valid inbound sequences must retain observation evidence");
        assert_eq!(
            receipt.selected.as_ref().unwrap().sequence,
            u64::try_from(sequence).unwrap()
        );
        let carrier: i64 = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT position FROM transcript_entries WHERE reference_id=?1",
                [accepted.message_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(carrier < 0);
        store.adopt_pending_batch(accepted.sequence, None).unwrap();
        store
            .append_canonical(&request_context_message("canonical-after", "context"))
            .unwrap();
        let connection = store.lock().unwrap();
        let adopted: (i64, bool) = connection
            .query_row(
                "SELECT position,ledger_position=2 FROM transcript_entries WHERE reference_id=?1",
                [accepted.message_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(adopted, (carrier, true));
        let count: u64 = connection
            .query_row(
                "SELECT count(*) FROM transcript_entries WHERE position<0 AND header_json IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
        drop(connection);
        assert!(
            store
                .load_transcript_page(None, 8)
                .unwrap()
                .entries
                .is_empty()
        );
    }
}
