#[test]
fn native_tool_page_resolves_once_without_argument_base64_round_trip() {
    let store = store();
    complete_tool_row_fixture(&store);
    let cut = store.observation_read_cut().unwrap();
    let locator = crate::observation::content::CanonicalContentLocator::ByMessageId {
        message_id: MessageId::new("derived-native-proposal"),
    };
    crate::durable::sqlite::content::take_source_work();
    let first = store
        .transcript_tool_headers(&cut, &locator, 0, 32)
        .unwrap();
    assert_eq!(
        crate::durable::sqlite::content::take_source_work(),
        (1, 0),
        "one source resolution serves association, raw/canonical descriptors and argument bytes"
    );
    assert_eq!(
        first.headers[0].arguments_json.as_deref(),
        Some(r#"{"a":1,"b":2}"#)
    );
    assert!(first.headers[0].started.is_some());
    assert!(first.headers[0].terminal.is_some());
    assert!(first.headers[0].committed_result.is_some());
    assert!(store.lock().unwrap().is_autocommit());
    let second = store
        .transcript_tool_headers(&cut, &locator, 0, 32)
        .unwrap();
    assert_eq!(second, first);
    assert_eq!(
        crate::durable::sqlite::content::take_source_work(),
        (1, 0),
        "a later call revalidates its own source"
    );
}

#[test]
fn native_tool_page_reuses_plans_and_bounds_vm_work_across_history() {
    use rusqlite::StatementStatus;
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let store = store();
    complete_tool_row_fixture(&store);
    let cut = store.observation_read_cut().unwrap();
    let locator = crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
        position: 1,
    };
    let expected = store.transcript_tool_headers(&cut, &locator, 0, 32).unwrap();
    let sql = [
        super::super::observation_tools::TOOL_NAME_SQL,
        super::super::observation_tools::TOOL_WITNESS_SQL,
        super::super::observation_tools::TOOL_RESULT_SQL,
    ];
    let mut previous = 0;
    let mut reference = None;
    for history in [8, 64, 512] {
        for index in previous..history {
            let attempt = response_start(&store, &format!("plan-history-{index}"));
            response_message(&store, &attempt, &format!("plan-message-{index}"));
            response_complete(&store, &attempt);
        }
        previous = history;
        let prepares = Arc::new(AtomicUsize::new(0));
        {
            let connection = store.lock().unwrap();
            connection.flush_prepared_statement_cache();
            let count = Arc::clone(&prepares);
            connection.authorizer(Some(move |context: AuthContext<'_>| {
                if matches!(context.action,
                    AuthAction::Read { table_name: "canonical_tool_calls", column_name: "observation_name" }
                    | AuthAction::Read { table_name: "events", column_name: "observation_tool_phase" }
                    | AuthAction::Read { table_name: "transcript_entries", column_name: "tool_result_json" }) {
                    count.fetch_add(1, Ordering::SeqCst);
                }
                Authorization::Allow
            })).unwrap();
        }
        assert_eq!(store.transcript_tool_headers(&cut, &locator, 0, 32).unwrap(), expected);
        assert!(prepares.swap(0, Ordering::SeqCst) > 0);
        let counts = |reset: bool| {
            let connection = store.lock().unwrap();
            sql.map(|query| {
                let statement = connection.prepare_cached(query).unwrap();
                let values = (statement.get_status(StatementStatus::Run), statement.get_status(StatementStatus::VmStep));
                if reset {
                    statement.reset_status(StatementStatus::Run);
                    statement.reset_status(StatementStatus::VmStep);
                }
                values
            })
        };
        let cold = counts(true);
        assert_eq!([cold[0].0, cold[1].0, cold[2].0], [1, 2, 1]);
        assert_eq!(store.transcript_tool_headers(&cut, &locator, 0, 32).unwrap(), expected);
        assert_eq!(prepares.swap(0, Ordering::SeqCst), 0, "H={history}");
        assert_eq!(counts(true), cold, "H={history}");
        if let Some(reference) = reference {
            assert_eq!(cold, reference, "H={history}");
        } else {
            reference = Some(cold);
        }
        store.lock().unwrap().authorizer(None::<fn(AuthContext<'_>) -> Authorization>).unwrap();
    }
}

#[test]
fn native_tool_page_keeps_one_snapshot_and_rechecks_after_external_changes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native-tool-content-snapshot.sqlite");
    let conversation = ConversationId::generate();
    let writer = Arc::new(SqliteConversationStore::open(conversation.clone(), &path).unwrap());
    complete_tool_row_fixture(&writer);
    let reader = SqliteConversationStore::open_existing(conversation, &path).unwrap();
    let cut = reader.observation_read_cut().unwrap();
    let locator =
        crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position: 1 };
    let original = reader
        .transcript_tool_headers(&cut, &locator, 0, 32)
        .unwrap();
    let competing = writer.clone();
    super::super::observation_tools::probe_after_associations(move || {
        assert_eq!(competing.lock().unwrap().execute(
            "UPDATE content_frontiers SET bytes=?1 WHERE source_kind='canonical' AND source_id=5 AND block_index=0 AND representation='j' AND revision=1",
            [br#"{"b":2,"a":1}"#.as_slice()],
        ).unwrap(), 1);
    });
    assert_eq!(
        reader
            .transcript_tool_headers(&cut, &locator, 0, 32)
            .unwrap(),
        original,
        "the native page's read transaction covers both associations and argument bytes"
    );
    let changed = reader
        .transcript_tool_headers(&cut, &locator, 0, 32)
        .unwrap();
    assert_eq!(
        changed.headers[0].arguments_json.as_deref(),
        Some(r#"{"b":2,"a":1}"#)
    );
    assert_ne!(changed, original);
    writer.lock().unwrap().execute("UPDATE content_frontiers SET bytes=?1 WHERE source_kind='canonical' AND source_id=5 AND block_index=0 AND representation='j'", [vec![255u8; 13]]).unwrap();
    assert!(
        reader
            .transcript_tool_headers(&cut, &locator, 0, 32)
            .is_err(),
        "actual argument UTF-8 is still checked"
    );
    assert!(
        reader.lock().unwrap().is_autocommit(),
        "the failed page rolls back its read transaction"
    );
    writer.lock().unwrap().execute("UPDATE content_frontiers SET bytes=?1 WHERE source_kind='canonical' AND source_id=5 AND block_index=0 AND representation='j'", [br#"{"a":1,"b":2}"#.as_slice()]).unwrap();
    assert_eq!(
        reader
            .transcript_tool_headers(&cut, &locator, 0, 32)
            .unwrap(),
        original
    );
}

#[test]
fn native_tool_page_retains_raw_priority_missing_frontier_and_cut_errors() {
    let store = store();
    complete_tool_row_fixture(&store);
    let cut = store.observation_read_cut().unwrap();
    let locator =
        crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position: 1 };
    let raw = br#"{ "b":2, "a":1 }"#;
    {
        let connection = store.lock().unwrap();
        connection.execute("INSERT INTO content_segments(source_kind,source_id,block_index,representation,segment,bytes) VALUES('canonical',5,0,'r',0,?1)", [raw.as_slice()]).unwrap();
        connection.execute("INSERT INTO content_frontiers(source_kind,source_id,block_index,representation,revision,readable_end) VALUES('canonical',5,0,'r',1,?1)", [raw.len() as u64]).unwrap();
    }
    let page = store
        .transcript_tool_headers(&cut, &locator, 0, 32)
        .unwrap();
    assert_eq!(
        page.headers[0].arguments_json.as_deref(),
        Some(std::str::from_utf8(raw).unwrap())
    );
    store.lock().unwrap().execute("DELETE FROM content_frontiers WHERE source_id=5 AND representation='r'", []).unwrap();
    let error = store
        .transcript_tool_headers(&cut, &locator, 0, 32)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("retained original raw arguments frontier unavailable")
    );
    assert!(store.lock().unwrap().is_autocommit());
    let mut foreign = cut.clone();
    foreign.read.conversation_id = ConversationId::generate();
    let error = store
        .transcript_tool_headers(&foreign, &locator, 0, 32)
        .unwrap_err();
    assert!(
        !error
            .to_string()
            .contains("retained original raw arguments frontier unavailable"),
        "cut failure still precedes the bad argument frontier"
    );
    assert!(store.lock().unwrap().is_autocommit());
}
