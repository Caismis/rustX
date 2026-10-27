// Public outlines use bounded original-row evidence; canonical turns remain an oracle.
fn observed_outline_matches_native(store: &SqliteConversationStore) {
    let native = store.conversation_turns(0, 64).unwrap();
    let observed = store.observation_turns(Some(0), 64).unwrap();
    assert_eq!(observed.cut, native.cut);
    assert_eq!(observed.total, native.total as u64);
    assert_eq!(observed.turns.len(), native.turns.len());
    for (actual, original) in observed.turns.iter().zip(native.turns) {
        assert_eq!(actual.id.conversation_id, original.id.conversation_id);
        assert_eq!(actual.id.attempt_id.as_ref(), Some(&original.id.attempt_id));
        assert!(actual.id.attempt_id_source.is_none());
        assert_eq!(actual.cursor, original.cursor);
        assert_eq!(actual.prompt, original.prompt);
        assert_eq!(actual.response, original.response);
    }
}

#[test]
fn t08_t12_t14_t19_observed_turn_previews_preserve_idle_owned_and_inherited_inputs() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    adopt(&store, UserSource::Human, "Open   turn\none", None);
    start(&store, "outline-one");
    commit_text(&store, "outline-one", "outline-first", "First answer");
    observed_outline_matches_native(&store);
    adopt(&store, UserSource::Human, "Steer", Some("outline-one"));
    commit_text(&store, "outline-one", "outline-final", &"水".repeat(120));
    adopt(&store, UserSource::Human, &"🙂".repeat(50), None);
    timeout(&store, "outline-one");
    start(&store, "outline-two");
    commit_text(
        &store,
        "outline-two",
        "outline-second-final",
        "Second answer",
    );
    timeout(&store, "outline-two");
    adopt(&store, UserSource::ExternalSystem, "Notice", None);
    start(&store, "outline-three");
    timeout(&store, "outline-three");
    observed_outline_matches_native(&store);
    let page = store.observation_turns(Some(0), 64).unwrap();
    assert_eq!(page.turns[0].prompt, "Open turn one");
    assert_eq!(page.turns[0].response, format!("{}…", "水".repeat(119)));
    assert_eq!(page.turns[1].prompt, format!("{}…", "🙂".repeat(49)));
    assert!(page.turns[2].prompt.is_empty());
    let frozen = store
        .read_lineage_cut(store.load_head().unwrap().revision)
        .unwrap();
    let seed = LineageSeed::history(frozen.canonical)
        .with_turns(frozen.turns)
        .unwrap()
        .with_completed_responses(frozen.completed_responses)
        .unwrap();
    let inherited = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    inherited.initialize_lineage(&seed).unwrap();
    observed_outline_matches_native(&inherited);
    let copied = inherited.observation_turns(Some(0), 64).unwrap();
    assert_eq!(
        copied
            .turns
            .iter()
            .map(|turn| (&turn.prompt, &turn.response))
            .collect::<Vec<_>>(),
        page.turns
            .iter()
            .map(|turn| (&turn.prompt, &turn.response))
            .collect::<Vec<_>>()
    );
}

fn deny_outline_original_bodies(
    context: rusqlite::hooks::AuthContext<'_>,
) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        AuthAction::Read {
            table_name,
            column_name,
        } if matches!(
            (table_name, column_name),
            ("events", "event_json" | "attempt_id")
                | ("message_ledger", "message_json")
                | ("request_snapshots", "snapshot_json")
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

fn original_outline_owner(
    store: &SqliteConversationStore,
    source: &crate::runtime_client::response::AttemptIdentitySource,
    through: u64,
) -> String {
    let crate::runtime_client::response::AttemptIdentitySource::Journal { field, .. } = source
    else {
        panic!("local outline must name its original Start J");
    };
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = store
            .event_value_read(field.sequence, through, &field.path, offset, 65_536)
            .unwrap();
        assert!(chunk.bytes.len() <= 65_536);
        bytes.extend(chunk.bytes);
        match chunk.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn t08_t12_t14_t19_giant_local_outline_and_window_keep_the_original_cut() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize(&[]).unwrap();
    let original = "\u{feff}owner🙂水".repeat(80_000);
    start(&store, &original);
    let opened = store.observation_turns(Some(0), 64).unwrap();
    assert_eq!(opened.total, 1);
    assert!(opened.turns[0].cursor.is_none());
    assert!(opened.turns[0].id.attempt_id.is_none());
    let source = opened.turns[0].id.attempt_id_source.as_ref().unwrap();
    assert_eq!(
        original_outline_owner(&store, source, opened.cut.journal),
        original
    );
    commit_text(
        &store,
        &original,
        "outline-giant-first",
        &"first body 水".repeat(100_000),
    );
    let before = store.observation_turns(Some(0), 64).unwrap();
    let at = ConversationWindowAt::TurnOrdinal {
        ordinal: 1,
        cut: before.cut.clone(),
    };
    let window = store.read_window_headers(&at, 32).unwrap();
    assert_eq!(window.target.as_ref(), Some(&before.turns[0].id));
    assert!(before.turns[0].response.is_empty());
    commit_text(
        &store,
        &original,
        "outline-giant-final",
        "Final native answer",
    );
    timeout(&store, &original);
    for index in 1..64 {
        let attempt = format!("outline-following-{index}");
        start(&store, &attempt);
        timeout(&store, &attempt);
    }
    store
        .lock()
        .unwrap()
        .authorizer(Some(deny_outline_original_bodies))
        .unwrap();
    let after = store.observation_turns(Some(0), 64).unwrap();
    assert_eq!((after.total, after.turns.len()), (64, 64));
    assert_eq!(after.turns[0].response, "Final native answer");
    assert_eq!(after.turns[0].cursor, before.turns[0].cursor);
    assert_eq!(after.turns[0].id, before.turns[0].id);
    assert!(serde_json::to_vec(&after).unwrap().len() < 65_536);
    let next = store.observation_turns(Some(1), 1).unwrap();
    assert_eq!((next.offset, next.turns[0].ordinal), (1, 2));
    assert_eq!(
        next.turns[0].id.attempt_id.as_ref().unwrap().as_str(),
        "outline-following-1"
    );
    let old = super::observation_turn::selected(
        &store.lock().unwrap(),
        store.conversation_id(),
        &before.cut,
        1,
    )
    .unwrap();
    assert_eq!(old, before.turns[0]);
    store
        .lock()
        .unwrap()
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>)
        .unwrap();
    assert_eq!(store.read_window_headers(&at, 32).unwrap(), window);
    assert!(matches!(
        store.observation_turns(Some(9_007_199_254_740_992), 64),
        Err(crate::durable::ConversationStoreError::InvalidContentRange)
    ));
}

#[test]
fn t10_t14_t19_turn_header_fault_preserves_original_j_c_t_and_bootstrap() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize(&[]).unwrap();
    start(&store, "outline-fault");
    commit_text(&store, "outline-fault", "outline-fault-c", "Original body");
    let before = store.observation_turns(Some(0), 64).unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER turn_header_fault BEFORE UPDATE OF observation_turn_header ON events BEGIN SELECT RAISE(FAIL,'optional turn header'); END;").unwrap();
    let terminal = store
        .append_event(event(
            &store,
            "outline-fault",
            RuntimeEvent::AttemptTimedOut {
                attempt_id: AttemptId::new("outline-fault"),
            },
        ))
        .unwrap();
    assert!(matches!(
        terminal.event,
        RuntimeEvent::AttemptTimedOut { .. }
    ));
    assert!(terminal.sequence > before.cut.journal);
    assert_eq!(
        store.load_canonical().unwrap(),
        vec![message("outline-fault-c", "Original body")]
    );
    assert!(
        store
            .load_transcript_page(None, 64)
            .unwrap()
            .entries
            .iter()
            .any(|entry| matches!(
                entry.item,
                crate::durable::TranscriptItem::AttemptTerminal { .. }
            ))
    );
    assert!(matches!(
        store.observation_turns(Some(0), 64),
        Err(crate::durable::ConversationStoreError::Storage(_))
    ));
    assert_eq!(
        super::observation_turn::selected(
            &store.lock().unwrap(),
            store.conversation_id(),
            &before.cut,
            1
        )
        .unwrap(),
        before.turns[0]
    );
    let seed = LineageSeed::history(vec![message("seeded-outline", "Retained")])
        .with_turns(vec![TurnReadingProvenance {
            id: ConversationTurnId {
                conversation_id: ConversationId::generate(),
                attempt_id: AttemptId::new("inherited-outline"),
            },
            process_message_ids: vec![MessageId::new("seeded-outline")],
            preceding_message_id: None,
            prompt_message_id: None,
            outcome: InheritedTurnOutcome::TimedOut,
            started_at: None,
            ended_at: Some(Utc::now()),
            execution: None,
        }])
        .unwrap();
    let inherited = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    inherited.lock().unwrap().execute_batch("CREATE TRIGGER inherited_outline_fault BEFORE UPDATE OF observation_turn_display ON inherited_turn_provenance BEGIN SELECT RAISE(FAIL,'optional inherited outline'); END;").unwrap();
    inherited.initialize_lineage(&seed).unwrap();
    assert_eq!(
        inherited.load_canonical().unwrap().as_slice(),
        seed.canonical()
    );
    assert_eq!(inherited.load_inherited_turns().unwrap(), seed.turns());
    assert!(matches!(
        inherited.observation_turns(Some(0), 64),
        Err(crate::durable::ConversationStoreError::Storage(_))
    ));
}

#[test]
fn turn_start_materializes_ordinal_header_once_and_reads_empty_prompt_once() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize(&[]).unwrap();
    store.lock().unwrap().execute_batch(
        "CREATE TEMP TABLE turn_writes(ordinal INTEGER,anchor INTEGER,header BLOB);
         CREATE TEMP TRIGGER count_turn_write AFTER UPDATE OF observation_turn_ordinal,observation_turn_anchor,observation_turn_header ON events
         BEGIN INSERT INTO turn_writes VALUES(NEW.observation_turn_ordinal,NEW.observation_turn_anchor,NEW.observation_turn_header); END;"
    ).unwrap();
    super::observation_turn::take_prompt_reads();
    start(&store, "one-turn-write");
    assert_eq!(super::observation_turn::take_prompt_reads(), 1);
    let connection = store.lock().unwrap();
    let rows: Vec<(u64, u64, Vec<u8>)> = connection
        .prepare("SELECT ordinal,anchor,header FROM turn_writes")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].0, rows[0].1), (1, 1));
    let header: serde_json::Value = serde_json::from_slice(&rows[0].2).unwrap();
    assert_eq!(header["turn"]["ordinal"], 1);
    assert_eq!(header["turn"]["prompt"], "");
    drop(connection);
    observed_outline_matches_native(&store);
    let old = store.observation_turns(Some(0), 64).unwrap();
    commit_text(
        &store,
        "one-turn-write",
        "one-turn-answer",
        "Answer before adoption",
    );
    assert_eq!(
        super::observation_turn::selected(
            &store.lock().unwrap(),
            store.conversation_id(),
            &old.cut,
            1,
        )
        .unwrap(),
        old.turns[0],
        "ordinary canonical growth preserves the earlier outline",
    );
    adopt(
        &store,
        UserSource::Human,
        "Later prompt",
        Some("one-turn-write"),
    );
    assert_eq!(
        store.observation_turns(Some(0), 64).unwrap().turns[0].prompt,
        "Later prompt"
    );
    // Adoption retires pending ownership and advances reading_mutation_revision.
    // Its earlier outline is deliberately expired, not an append-only cut.
    assert!(matches!(
        super::observation_turn::selected(
            &store.lock().unwrap(),
            store.conversation_id(),
            &old.cut,
            1,
        ),
        Err(crate::durable::ConversationStoreError::ObservationCutExpired),
    ));
}

#[test]
fn turn_start_ordinal_and_header_failure_keeps_native_and_other_layouts() {
    for column in ["observation_turn_ordinal", "observation_turn_header"] {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize(&[]).unwrap();
        store.lock().unwrap().execute_batch(&format!(
            "CREATE TRIGGER fail_turn BEFORE UPDATE OF {column} ON events BEGIN SELECT RAISE(FAIL,'optional turn'); END;"
        )).unwrap();
        start(&store, "atomic-turn-fault");
        let connection = store.lock().unwrap();
        let state: (bool,bool,bool,bool,bool) = connection.query_row(
            "SELECT observation_turn_ordinal IS NULL,observation_turn_header IS NULL,observation_turn_anchor IS NULL,observation_witness IS NOT NULL,observation_statistics IS NOT NULL FROM events WHERE sequence=1", [],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        ).unwrap();
        assert_eq!(state, (true, true, true, true, true));
        drop(connection);
        assert_eq!(store.read_events(None, 64).unwrap().events.len(), 1);
        assert_eq!(store.conversation_turns(0, 64).unwrap().turns.len(), 1);
        assert!(store.observation_turns(Some(0), 64).is_err());
    }
}
