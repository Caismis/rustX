#[test]
fn native_tool_page_resolves_once_without_argument_base64_round_trip() {
    let store = store();
    complete_tool_row_fixture(&store);
    let cut = store.observation_read_cut().unwrap();
    let locator = crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
        message_id: MessageId::new("derived-native-proposal"),
    };
    super::super::content::take_source_work();
    let first = store
        .transcript_tool_headers(&cut, &locator, 0, 32)
        .unwrap();
    assert_eq!(
        super::super::content::take_source_work(),
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
        super::super::content::take_source_work(),
        (1, 0),
        "a later call revalidates its own source"
    );
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
        crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position: 1 };
    let original = reader
        .transcript_tool_headers(&cut, &locator, 0, 32)
        .unwrap();
    let competing = writer.clone();
    super::super::observation_tools::probe_after_associations(move || {
        assert_eq!(competing.lock().unwrap().execute(
            "UPDATE content_frontiers SET bytes=?1 WHERE source_kind='canonical' AND source_id='ledger:1' AND block_index=0 AND representation='canonical_json_v1' AND revision=1",
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
    writer.lock().unwrap().execute("UPDATE content_frontiers SET bytes=?1 WHERE source_kind='canonical' AND source_id='ledger:1' AND block_index=0 AND representation='canonical_json_v1'", [vec![255u8; 13]]).unwrap();
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
    writer.lock().unwrap().execute("UPDATE content_frontiers SET bytes=?1 WHERE source_kind='canonical' AND source_id='ledger:1' AND block_index=0 AND representation='canonical_json_v1'", [br#"{"a":1,"b":2}"#.as_slice()]).unwrap();
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
        crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position: 1 };
    let raw = br#"{ "b":2, "a":1 }"#;
    {
        let connection = store.lock().unwrap();
        connection.execute("INSERT INTO content_segments(source_kind,source_id,block_index,representation,segment,bytes) VALUES('canonical','ledger:1',0,'raw_json_utf8',0,?1)", [raw.as_slice()]).unwrap();
        connection.execute("INSERT INTO content_frontiers(source_kind,source_id,block_index,representation,revision,readable_end) VALUES('canonical','ledger:1',0,'raw_json_utf8',1,?1)", [raw.len() as u64]).unwrap();
    }
    let page = store
        .transcript_tool_headers(&cut, &locator, 0, 32)
        .unwrap();
    assert_eq!(
        page.headers[0].arguments_json.as_deref(),
        Some(std::str::from_utf8(raw).unwrap())
    );
    store.lock().unwrap().execute("DELETE FROM content_frontiers WHERE source_id='ledger:1' AND representation='raw_json_utf8'", []).unwrap();
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
