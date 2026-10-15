// Fixed native read evidence does not replace original paired membership.
fn deny_selected_original_columns(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        AuthAction::Read {
            table_name,
            column_name,
        } if matches!(
            (table_name, column_name),
            ("message_ledger", "message_id" | "message_json")
                | ("inherited_message_membership", "assistant_message_id")
                | (
                    "inherited_turn_provenance" | "inherited_response_provenance",
                    "payload"
                )
        ) =>
        {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }
}
#[test]
fn t08_t12_t14_t19_giant_native_membership_witness_binds_exact_numeric_c() {
    let store = store();
    let original = "native-member🙂水".repeat(70_000);
    let seed = member_completed_lineage(&original);
    store.initialize_lineage(&seed).unwrap();
    store
        .append_canonical(&assistant_message("other-native-position", "Other body"))
        .unwrap();
    let cut = store.observation_read_cut().unwrap();
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_selected_original_columns));
    let process = store
        .observation_transcript_process(&cut, 2)
        .unwrap()
        .unwrap();
    assert_eq!(
        process.outcome,
        crate::durable::reading::TurnProcessOutcome::Completed
    );
    // The sole native member is the completed closing response, so the
    // intermediate process count excludes it while its final source remains.
    assert_eq!(seed.turns()[0].process_message_ids.len(), 1);
    assert_eq!(process.message_count, 0);
    assert!(process.final_message_id.is_none());
    assert!(process.final_message_id_source.is_some());
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>);
    let connection = store.lock().unwrap();
    assert!(connection.execute("UPDATE inherited_message_membership SET assistant_ledger_position=3 WHERE assistant_ledger_position=2",[]).is_err(),"native composite FK remains authoritative");
    connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    connection.execute("UPDATE inherited_message_membership SET assistant_ledger_position=3 WHERE assistant_ledger_position=2",[]).unwrap();
    assert!(
        super::observation_membership::validate(&connection, 3).is_err(),
        "another valid C cannot inherit the old native identity witness"
    );
}
#[test]
fn t10_t14_t19_membership_read_witness_failure_preserves_native_lineage() {
    let store = store();
    store.lock().unwrap().execute_batch("CREATE TRIGGER membership_read_fault BEFORE UPDATE OF observation_membership_witness ON inherited_message_membership BEGIN SELECT RAISE(FAIL,'optional membership witness'); END;").unwrap();
    let seed = member_completed_lineage("native-member-witness-fault");
    store.initialize_lineage(&seed).unwrap();
    assert_eq!(store.load_canonical().unwrap().as_slice(), seed.canonical());
    assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
    assert_eq!(
        store.load_inherited_responses().unwrap(),
        seed.completed_responses()
    );
    assert!(store.lock().unwrap().query_row("SELECT observation_membership_witness IS NULL FROM inherited_message_membership WHERE assistant_ledger_position=2",[],|row|row.get::<_,bool>(0)).unwrap());
    let cut = store.observation_read_cut().unwrap();
    let error = store.observation_transcript_process(&cut, 2).unwrap_err();
    assert!(matches!(
        crate::runtime_client::RuntimeClientError::observation_store_error(error),
        crate::runtime_client::RuntimeClientError::ObservationUnavailable { .. }
    ));
}
fn corrupt_selected_membership_metadata(
    store: &SqliteConversationStore,
    table: &str,
    column: &str,
    key: &str,
) -> rusqlite::types::Value {
    let original: rusqlite::types::Value = store
        .lock()
        .unwrap()
        .query_row(
            &format!("SELECT {column} FROM {table} WHERE {key}=2"),
            [],
            |row| row.get(0),
        )
        .unwrap();
    let oversized = if column == "membership_digest" {
        rusqlite::types::Value::Text("a".repeat(1_048_576))
    } else {
        rusqlite::types::Value::Blob(vec![0; 1_048_576])
    };
    {
        let connection = store.lock().unwrap();
        connection
            .execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        connection
            .execute(
                &format!("UPDATE {table} SET {column}=?1 WHERE {key}=2"),
                [oversized],
            )
            .unwrap();
        connection
            .execute_batch("PRAGMA ignore_check_constraints=OFF")
            .unwrap();
        connection.authorizer(Some(deny_selected_original_columns));
        let plan = connection
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {}",
                super::observation_membership::SELECTED_EVIDENCE
            ))
            .unwrap()
            .query_map([2], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(
            !plan.contains("SCAN ") && !plan.contains("TEMP B-TREE"),
            "{plan}"
        );
    }
    original
}
#[test]
fn t08_t14_t19_membership_sql_bounds_reject_corruption_before_metadata_allocation() {
    let store = store();
    let seed = member_completed_lineage("bounded-membership-metadata");
    store.initialize_lineage(&seed).unwrap();
    let cut = store.observation_read_cut().unwrap();
    let cases = [
        ("message_ledger", "observation_identity_digest", "position"),
        (
            "inherited_message_membership",
            "observation_identity_digest",
            "assistant_ledger_position",
        ),
        (
            "inherited_message_membership",
            "observation_membership_witness",
            "assistant_ledger_position",
        ),
        (
            "inherited_message_membership",
            "membership_digest",
            "assistant_ledger_position",
        ),
    ];
    for (table, column, key) in cases {
        let original = corrupt_selected_membership_metadata(&store, table, column, key);
        let error = store.observation_transcript_process(&cut, 2).unwrap_err();
        assert!(matches!(
            crate::runtime_client::RuntimeClientError::observation_store_error(error),
            crate::runtime_client::RuntimeClientError::ObservationUnavailable { .. }
        ));
        let (rows, bytes) = super::observation_membership::read_work();
        assert_eq!(rows, 1);
        assert!(bytes <= 160, "corrupt {column} materialized {bytes} bytes");
        {
            let connection = store.lock().unwrap();
            connection.authorizer(
                None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
            );
            connection
                .execute(
                    &format!("UPDATE {table} SET {column}=?1 WHERE {key}=2"),
                    [original],
                )
                .unwrap();
        }
        assert_eq!(store.load_canonical().unwrap().as_slice(), seed.canonical());
        assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
        assert_eq!(
            store.load_inherited_responses().unwrap(),
            seed.completed_responses()
        );
        assert_eq!(store.observation_read_cut().unwrap(), cut);
        assert!(
            store
                .observation_transcript_process(&cut, 2)
                .unwrap()
                .is_some()
        );
    }
}
fn selected_delivery_fixture(store: &SqliteConversationStore, identity: &str) -> MessageBlock {
    use crate::message::types::{ContentBlockIndex, ToolCallOccurrenceRef};
    let call = ToolCall {
        id: ToolCallId::new("selected-file-call"),
        tool_id: ToolId::new("selected-file-tool"),
        name: "present".into(),
        arguments: serde_json::json!({}),
    };
    store
        .initialize(&[MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("selected-file-owner"),
            content: vec![AssistantContentBlock::ToolCall(call.clone())],
        })])
        .unwrap();
    MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new(identity),
        occurrence: ToolCallOccurrenceRef::new(
            MessageId::new("selected-file-owner"),
            ContentBlockIndex::new(0),
        ),
        tool_call_id: call.id,
        tool_id: call.tool_id,
        result: ToolExecutionResult {
            deliveries: vec![crate::tools::session_files::SessionFileReference {
                scope: crate::tools::session_files::SessionFileScope {
                    conversation_id: store.conversation_id().clone(),
                    device: "1".into(),
                    inode: "2".into(),
                },
                path: "report.txt".into(),
                name: "report.txt".into(),
                description: Some("Original declaration".into()),
                mime_type: "text/plain".into(),
            }],
            status: ToolExecutionStatus::Success,
            content: vec![crate::tools::types::ToolResultContent::Text(TextBlock {
                text: "original result 水".repeat(90_000),
            })],
            duration_ms: 1,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        },
    })
}
#[test]
fn t08_t12_t14_t19_selected_delivery_uses_numeric_giant_c_without_body_reads() {
    use crate::runtime_client::content::CanonicalContentLocator;
    let store = store();
    let original = "selected-result🙂水".repeat(70_000);
    let message = selected_delivery_fixture(&store, &original);
    store.append_canonical(&message).unwrap();
    let MessageBlock::Tool(tool) = &message else {
        unreachable!()
    };
    let cut = store.observation_read_cut().unwrap();
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_selected_original_columns));
    let locator = CanonicalContentLocator::ByLedgerPosition { position: 2 };
    let selected = store.observation_file_delivery(&locator, 0).unwrap();
    assert_eq!(selected.cut, cut);
    assert_eq!((selected.ledger_position, selected.index), (2, 0));
    assert_eq!(selected.file, tool.result.deliveries[0]);
    let (rows, bytes) = SqliteConversationStore::content_read_work();
    assert!(rows > 0 && rows <= 2 && bytes <= 16_384);
    assert!(matches!(
        store.observation_file_delivery(&locator, 1),
        Err(ConversationStoreError::InvalidContentRange)
    ));
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>);
    assert_eq!(store.load_canonical().unwrap()[1], message);
    store
        .lock()
        .unwrap()
        .execute_batch("DELETE FROM content_segments WHERE source_kind='canonical_file_delivery';")
        .unwrap();
    assert!(matches!(
        store.observation_file_delivery(&locator, 0),
        Err(ConversationStoreError::Storage(_))
    ));
}
#[test]
fn t10_t14_t19_selected_delivery_auxiliary_failure_preserves_original_successful_c() {
    let store = store();
    let message = selected_delivery_fixture(&store, "selected-file-fault");
    store.lock().unwrap().execute_batch("CREATE TRIGGER selected_delivery_fault BEFORE INSERT ON content_segments WHEN NEW.source_kind='canonical_file_delivery' BEGIN SELECT RAISE(FAIL,'optional delivery span'); END;").unwrap();
    store.append_canonical(&message).unwrap();
    assert_eq!(store.load_canonical().unwrap()[1], message);
    let locator =
        crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position: 2 };
    let error = store.observation_file_delivery(&locator, 0).unwrap_err();
    assert!(matches!(
        crate::runtime_client::RuntimeClientError::observation_store_error(error),
        crate::runtime_client::RuntimeClientError::ObservationUnavailable { .. }
    ));
}

fn escaped_delivery_file(
    root: &std::path::Path,
    conversation: &ConversationId,
) -> crate::tools::session_files::SessionFileReference {
    let component = "\u{1}".repeat(200);
    let mut directory = root.to_owned();
    for _ in 0..15 {
        directory.push(&component);
        std::fs::create_dir(&directory).unwrap();
    }
    let path = directory.join(format!("{component}.txt"));
    std::fs::write(&path, b"original authorized bytes").unwrap();
    crate::tools::session_files::declare(
        root,
        conversation,
        path.strip_prefix(root).unwrap().to_str().unwrap(),
        Some("\u{1}".repeat(512)),
    )
    .unwrap()
}
fn read_selected_delivery_bytes(
    store: &SqliteConversationStore,
    cut: &crate::runtime_client::checkpoint::CheckpointReadCut,
    item: &crate::runtime_client::delivery::RuntimeClientDeliveryItem,
) -> Vec<u8> {
    use base64::Engine as _;
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = crate::runtime_client::history_observation::value_read(
            store,
            cut,
            &item.metadata_domain,
            0,
            &[],
            (offset, 511),
        )
        .unwrap();
        bytes.extend(
            base64::engine::general_purpose::STANDARD
                .decode(chunk.bytes_base64)
                .unwrap(),
        );
        let (rows, materialized) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && materialized <= 65_536);
        match chunk.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    bytes
}
fn assert_delivery_metadata_coordinates(
    store: &SqliteConversationStore,
    cut: &crate::runtime_client::checkpoint::CheckpointReadCut,
    item: &crate::runtime_client::delivery::RuntimeClientDeliveryItem,
) {
    assert!(
        crate::runtime_client::history_observation::value_read(
            store,
            cut,
            &item.metadata_domain,
            1,
            &[],
            (0, 511)
        )
        .is_err()
    );
    assert!(
        crate::runtime_client::history_observation::value_read(
            store,
            cut,
            &item.metadata_domain,
            0,
            &["path".into()],
            (0, 511)
        )
        .is_err()
    );
}
#[test]
fn t08_t12_t14_t19_escaped_delivery_metadata_advances_beyond_inline_budget() {
    let store = store();
    let root = tempfile::tempdir().unwrap();
    let original_file = escaped_delivery_file(root.path(), store.conversation_id());
    let mut message = selected_delivery_fixture(&store, "escaped-selected-file");
    let MessageBlock::Tool(tool) = &mut message else {
        unreachable!()
    };
    tool.result.deliveries = vec![original_file.clone()];
    store.append_canonical(&message).unwrap();
    let cut = occupancy_origin_cut(&store);
    let source = cut.source_cut.as_ref().unwrap();
    let header = store
        .message_header_page(source, Some(1), 1)
        .unwrap()
        .headers
        .into_iter()
        .next()
        .unwrap();
    let descriptor =
        crate::runtime_client::snapshot::RuntimeClientMessageDescriptor::from_header(&header)
            .unwrap();
    let head = descriptor.deliveries.as_ref().unwrap();
    assert_eq!((head.total, head.items.len()), (1, 1));
    let item = &head.items[0];
    assert!(item.file.is_none() && item.metadata_deferred);
    assert!(item.encoded_bytes > 16_384 && item.encoded_bytes < 65_536);
    let page = crate::runtime_client::history_observation::page(&store, &cut, &head.domain, 0, 32)
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].offset, 0);
    let bytes = read_selected_delivery_bytes(&store, &cut, item);
    assert_eq!(bytes.len() as u64, item.encoded_bytes);
    assert_eq!(
        serde_json::from_slice::<crate::tools::session_files::SessionFileReference>(&bytes)
            .unwrap(),
        original_file
    );
    let selected = store
        .observation_file_delivery(&item.locator, item.index)
        .unwrap();
    assert_eq!(selected.file, original_file);
    assert_eq!(
        crate::tools::session_files::read_authorized(root.path(), &selected.file, || Ok(()))
            .unwrap(),
        b"original authorized bytes"
    );
    assert!(
        crate::tools::session_files::read_authorized(root.path(), &selected.file, || Err(
            std::io::ErrorKind::PermissionDenied.into()
        ))
        .is_err()
    );
    assert_delivery_metadata_coordinates(&store, &cut, item);
    let error = store
        .observation_delivery_read(
            source,
            &item.locator,
            item.index,
            item.encoded_bytes + 1,
            511,
        )
        .unwrap_err();
    assert!(matches!(error, ConversationStoreError::InvalidContentRange));
    let mut foreign = cut.clone();
    foreign.conversation_id = ConversationId::generate();
    assert!(
        crate::runtime_client::history_observation::value_read(
            &store,
            &foreign,
            &item.metadata_domain,
            0,
            &[],
            (0, 511)
        )
        .is_err()
    );
    assert_eq!(store.observation_read_cut().unwrap(), *source);
    assert_eq!(store.load_canonical().unwrap()[1], message);
}

#[test]
fn t08_t12_t14_t19_original_nine_deliveries_advance_and_select_last_index() {
    use crate::runtime_client::content::CanonicalContentLocator;
    let store = store();
    let root = tempfile::tempdir().unwrap();
    let files = (0..9)
        .map(|index| {
            let path = format!("original-{index}.txt");
            std::fs::write(root.path().join(&path), format!("original file {index}")).unwrap();
            crate::tools::session_files::declare(root.path(), store.conversation_id(), &path, None)
                .unwrap()
        })
        .collect::<Vec<_>>();
    let mut message = selected_delivery_fixture(&store, "native-nine-deliveries");
    let MessageBlock::Tool(tool) = &mut message else {
        unreachable!()
    };
    tool.result.deliveries.clone_from(&files);
    store.append_canonical(&message).unwrap();
    let cut = occupancy_origin_cut(&store);
    let source = cut.source_cut.as_ref().unwrap();
    let locator = CanonicalContentLocator::ByLedgerPosition { position: 2 };
    let header = store.message_header_page(source, Some(1), 1).unwrap();
    let head = header.headers[0].deliveries.as_ref().unwrap();
    assert_eq!(
        (head.total, head.items.len(), head.next_offset),
        (9, 8, Some(8))
    );
    assert_eq!(
        head.items.iter().map(|item| item.index).collect::<Vec<_>>(),
        (0..8).collect::<Vec<_>>()
    );
    let page = crate::runtime_client::history_observation::page(&store, &cut, &head.domain, 8, 32)
        .unwrap();
    assert_eq!(
        (page.total, page.items.len(), page.next_offset),
        (9, 1, None)
    );
    assert_eq!(page.items[0].offset, 8);
    let tail = store
        .observation_deliveries(source, &locator, 8, 32)
        .unwrap();
    assert_eq!(
        (tail.total, tail.items.len(), tail.next_offset),
        (9, 1, None)
    );
    let item = &tail.items[0];
    assert_eq!((item.index, &item.locator), (8, &locator));
    let bytes = read_selected_delivery_bytes(&store, &cut, item);
    assert_eq!(
        serde_json::from_slice::<crate::tools::session_files::SessionFileReference>(&bytes)
            .unwrap(),
        files[8]
    );
    let selected = store.observation_file_delivery(&locator, 8).unwrap();
    assert_eq!(
        (selected.cut, selected.index, selected.file.clone()),
        (source.clone(), 8, files[8].clone())
    );
    assert_eq!(
        crate::tools::session_files::read_authorized(root.path(), &selected.file, || Ok(()))
            .unwrap(),
        b"original file 8"
    );
    assert!(matches!(
        store.observation_file_delivery(&locator, 9),
        Err(ConversationStoreError::InvalidContentRange)
    ));
    assert_eq!(store.observation_read_cut().unwrap(), *source);
    assert_eq!(store.load_canonical().unwrap()[1], message);
}

#[test]
fn t08_t12_t14_t19_giant_delivery_alias_is_rejected_before_original_id_lookup() {
    use crate::runtime_client::content::CanonicalContentLocator;
    let store = store();
    let original = "delivery-owner🙂水".repeat(70_000);
    let message = selected_delivery_fixture(&store, &original);
    store.append_canonical(&message).unwrap();
    let cut = store.observation_read_cut().unwrap();
    let alias = CanonicalContentLocator::ByMessageId {
        message_id: MessageId::new(&original),
    };
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_selected_original_columns));
    for error in [
        store.observation_file_delivery(&alias, 0).unwrap_err(),
        store
            .observation_deliveries(&cut, &alias, 0, 8)
            .unwrap_err(),
        store
            .observation_delivery_read(&cut, &alias, 0, 0, 511)
            .unwrap_err(),
    ] {
        assert!(
            matches!(error, ConversationStoreError::InvalidReference(_)),
            "locator must fail validation before the authorizer denies original-ID SQL"
        );
    }
    let numeric = CanonicalContentLocator::ByLedgerPosition { position: 2 };
    assert_eq!(
        store.observation_file_delivery(&numeric, 0).unwrap().cut,
        cut
    );
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>);
    assert_eq!(store.load_canonical().unwrap()[1], message);
}
