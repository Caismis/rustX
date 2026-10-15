// These exercise actual native RequestStart commits; the comparison producer
// never reads a cache entry or a previously captured request-field SQL row.
fn fragment_fixture() -> SqliteConversationStore {
    let store = store();
    let mut messages = (0..12)
        .map(|index| {
            user_message(
                &format!("fragment-canonical-{index}"),
                &format!("original 水🙂 round {index}\n\"\\"),
            )
        })
        .collect::<Vec<_>>();
    let mut media = user_message("fragment-media", "original media owner");
    let MessageBlock::User(user) = &mut media else {
        unreachable!()
    };
    user.content.push(UserContentBlock::Image(
        crate::message::content::ImageReference {
            artifact_id: crate::runtime::identity::ArtifactId::new("fragment-image"),
            alt: Some("original exact image alt".into()),
        },
    ));
    messages.push(media);
    store.initialize(&messages).unwrap();
    store
}
fn fragment_snapshot(store: &SqliteConversationStore, turn: u32, retry: u32) -> RequestSnapshot {
    let mut snapshot = progress_request_snapshot(
        "fragment-owner",
        &turn.to_string(),
        retry,
        store.load_head().unwrap().revision,
        Vec::new(),
    );
    snapshot.invocation.model = format!("original-model-{turn}");
    freeze_test_input(store, &mut snapshot, &[]);
    snapshot
}
fn fragment_root_bytes(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    sequence: u64,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = store
            .request_value_read(&snapshot.request_id, sequence, &[], offset, 65_536)
            .unwrap();
        let (rows, materialized) = content::last_read_work();
        assert!(rows <= 2 && materialized <= 131_072);
        assert_eq!(chunk.end_offset, offset + chunk.bytes.len() as u64);
        bytes.extend_from_slice(&chunk.bytes);
        if let Some(next) = chunk.next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            return bytes;
        }
    }
}
fn assert_fragment_oracle(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    sequence: u64,
) {
    let input = store
        .reconstruct_model_request(&snapshot.request_id)
        .unwrap();
    let connection = store.lock().unwrap();
    let expected = observation_request::legacy_request_source_bytes(
        &connection,
        sequence,
        store.conversation_id(),
        &input.messages,
        snapshot,
    )
    .unwrap();
    let fields = observation_request::legacy_field_headers_oracle(
        &connection,
        sequence,
        store.conversation_id(),
        &input.messages,
        snapshot,
    )
    .unwrap();
    for (path, start, end, children, header) in &fields {
        let actual = observation_request::field_from(
            &connection,
            "request_observation_fields",
            &sequence.to_string(),
            path,
        )
        .unwrap();
        assert_eq!(
            &actual,
            &(*start, *end, *children, header.clone()),
            "{path:?}"
        );
        let mut offset = 0;
        let mut bytes = Vec::new();
        loop {
            let chunk =
                observation_request::read_at(&connection, sequence, sequence, path, offset, 257)
                    .unwrap();
            assert!(content::last_read_work().0 <= 2);
            bytes.extend_from_slice(&chunk.bytes);
            if let Some(next) = chunk.next_offset {
                assert!(next > offset);
                offset = next;
            } else {
                break;
            }
        }
        assert_eq!(
            bytes,
            expected[usize::try_from(*start).unwrap()..usize::try_from(*end).unwrap()],
            "{path:?}"
        );
    }
    let mut offset = 0;
    loop {
        let page = observation_request::page_at(
            &connection,
            sequence,
            sequence,
            &["messages".into()],
            offset,
            8,
        )
        .unwrap();
        assert_eq!(page.total, snapshot.input.len() as u64);
        assert!(page.fields.len() <= 8);
        for field in &page.fields {
            let (_, start, end, children, header) = fields
                .iter()
                .find(|(path, ..)| path == &field.path)
                .unwrap();
            assert_eq!(
                field.value,
                serde_json::from_slice::<serde_json::Value>(header).unwrap()
            );
            assert_eq!(field.encoded_bytes, end - start);
            assert_eq!(field.children_total, *children);
        }
        if let Some(next) = page.next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            break;
        }
    }
    drop(connection);
    assert_eq!(fragment_root_bytes(store, snapshot, sequence), expected);
}
#[test]
fn t08_t12_t14_t19_t20_canonical_fragments_match_full_source_every_field_and_original_range() {
    let store = fragment_fixture();
    let canonical = store.load_canonical().unwrap();
    let first = fragment_snapshot(&store, 1, 0);
    let started = store
        .commit_model_turn_start(&[], &first, Utc::now())
        .unwrap()
        .started;
    let stats = store.conn.request_fragments.lock().unwrap().stats();
    eprintln!("canonical fragment stats (entries,resident,hits,captures,peak): {stats:?}");
    assert!(
        stats.0 >= 12 && stats.3 >= 12,
        "actual canonical prototypes: {stats:?}"
    );
    assert_eq!(stats.2, 0);
    assert_fragment_oracle(&store, &first, started.sequence);
    let second = fragment_snapshot(&store, 2, 0);
    observation_request::take_header_constructions();
    let next = store
        .commit_model_turn_start(&[], &second, Utc::now())
        .unwrap()
        .started;
    assert!(observation_request::take_header_constructions() < second.input.len());
    let stats = store.conn.request_fragments.lock().unwrap().stats();
    eprintln!("canonical fragment stats (entries,resident,hits,captures,peak): {stats:?}");
    assert!(stats.2 >= 12, "actual repeated original reuse: {stats:?}");
    assert!(stats.1 <= 8 * 1024 * 1024 && stats.4 <= 8 * 1024 * 1024);
    assert_fragment_oracle(&store, &second, next.sequence);
    assert_ne!(first.request_id, second.request_id);
    let replay = store
        .commit_model_turn_start(&[], &second, Utc::now())
        .unwrap();
    assert_eq!(replay.started, next);
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(store.presentation_frontier().unwrap(), next.sequence);
    store.conn.request_fragments.lock().unwrap().evict();
    assert_eq!(store.conn.request_fragments.lock().unwrap().stats().0, 0);
    assert_fragment_oracle(&store, &second, next.sequence);
    store.conn.request_fragments.lock().unwrap().disable();
    let third = fragment_snapshot(&store, 3, 0);
    let last = store
        .commit_model_turn_start(&[], &third, Utc::now())
        .unwrap()
        .started;
    assert_fragment_oracle(&store, &third, last.sequence);
}
#[test]
fn t10_t19_canonical_fragment_admission_requires_optional_and_native_transaction_commit() {
    for (trigger, native_failure) in [
        (
            "CREATE TEMP TRIGGER refuse_fragment_layout BEFORE INSERT ON content_frontiers WHEN NEW.source_kind='trace_request' BEGIN SELECT RAISE(FAIL,'optional layout after capture'); END;",
            false,
        ),
        (
            "CREATE TEMP TRIGGER refuse_after_fragment_capture BEFORE UPDATE OF logical_step_sequence ON rustx_store BEGIN SELECT RAISE(FAIL,'native transition after optional capture'); END;",
            true,
        ),
    ] {
        let store = fragment_fixture();
        let canonical = store.load_canonical().unwrap();
        let snapshot = fragment_snapshot(&store, 1, 0);
        store.lock().unwrap().execute_batch(trigger).unwrap();
        let committed = store.commit_model_turn_start(&[], &snapshot, Utc::now());
        assert_eq!(committed.is_err(), native_failure);
        assert_eq!(store.conn.request_fragments.lock().unwrap().stats().0, 0);
        assert_eq!(store.conn.request_fragments.lock().unwrap().stats().2, 0);
        if let Ok(committed) = committed {
            assert_eq!(
                store.load_request_snapshot(&snapshot.request_id).unwrap(),
                snapshot
            );
            assert!(
                store
                    .request_summary_header(&snapshot.request_id, committed.started.sequence)
                    .is_err()
            );
            store
                .lock()
                .unwrap()
                .execute_batch("DROP TRIGGER refuse_fragment_layout")
                .unwrap();
        } else {
            assert_eq!(store.presentation_frontier().unwrap(), 0);
            store
                .lock()
                .unwrap()
                .execute_batch("DROP TRIGGER refuse_after_fragment_capture")
                .unwrap();
        }
        assert_eq!(store.load_canonical().unwrap(), canonical);
        let healthy = fragment_snapshot(&store, 2, 0);
        observation_request::take_header_constructions();
        let next = store
            .commit_model_turn_start(&[], &healthy, Utc::now())
            .unwrap()
            .started;
        assert_eq!(
            observation_request::take_header_constructions(),
            healthy.input.len()
        );
        assert_fragment_oracle(&store, &healthy, next.sequence);
        assert!(store.conn.request_fragments.lock().unwrap().stats().0 > 0);
    }
}
#[test]
fn t14_t19_canonical_fragment_hits_cannot_bypass_native_identity_or_current_header_integrity() {
    let store = fragment_fixture();
    let first = fragment_snapshot(&store, 1, 0);
    store
        .commit_model_turn_start(&[], &first, Utc::now())
        .unwrap();
    let canonical = store.load_canonical().unwrap();
    let before = store.presentation_frontier().unwrap();
    let cache_before = store.conn.request_fragments.lock().unwrap().stats();
    let mut foreign = fragment_snapshot(&store, 2, 0);
    let crate::model::projected::FrozenModelInput::Original(original) = &mut foreign.input[0]
    else {
        unreachable!()
    };
    original.conversation_id = ConversationId::new("conv_19156353-274c-78d1-9d13-38cecfaf8340");
    assert!(matches!(
        store.commit_model_turn_start(&[], &foreign, Utc::now()),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(
        store.conn.request_fragments.lock().unwrap().stats(),
        cache_before
    );
    assert_eq!(store.presentation_frontier().unwrap(), before);
    let second = fragment_snapshot(&store, 3, 0);
    store.lock().unwrap().execute_batch(
        "CREATE TEMP TRIGGER corrupt_fragment_original AFTER INSERT ON request_snapshots BEGIN UPDATE message_ledger SET message_json=json_set(message_json,'$.content[0].text','changed original C') WHERE position=1; END;"
    ).unwrap();
    assert!(matches!(
        store.commit_model_turn_start(&[], &second, Utc::now()),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER corrupt_fragment_original")
        .unwrap();
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(
        store.conn.request_fragments.lock().unwrap().stats(),
        cache_before
    );
    let header: Vec<u8> = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT header_json FROM observation_headers WHERE ledger_position=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE observation_headers SET header_json=X'7b' WHERE ledger_position=1",
            [],
        )
        .unwrap();
    let third = fragment_snapshot(&store, 4, 0);
    let committed = store
        .commit_model_turn_start(&[], &third, Utc::now())
        .unwrap();
    assert!(
        store
            .request_summary_header(&third.request_id, committed.started.sequence)
            .is_err()
    );
    assert_eq!(
        store.conn.request_fragments.lock().unwrap().stats().2,
        cache_before.2
    );
    assert_eq!(
        store.load_request_snapshot(&third.request_id).unwrap(),
        third
    );
    assert_eq!(store.load_canonical().unwrap(), canonical);
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE observation_headers SET header_json=?1 WHERE ledger_position=1",
            [header],
        )
        .unwrap();
    let fourth = fragment_snapshot(&store, 5, 0);
    let completed = store
        .commit_model_turn_start(&[], &fourth, Utc::now())
        .unwrap();
    assert_fragment_oracle(&store, &fourth, completed.started.sequence);
}
#[test]
fn t12_t19_canonical_fragments_preserve_reordered_repeated_and_compacted_native_occurrences() {
    use crate::scripted_suites::support::context::{checkpoint_commit_fixture, checkpoint_fixture};
    let store = fragment_fixture();
    let initial = fragment_snapshot(&store, 1, 0);
    store
        .commit_model_turn_start(&[], &initial, Utc::now())
        .unwrap();
    let mut reordered = fragment_snapshot(&store, 2, 0);
    let history = store.projected_history().unwrap();
    let mut messages = history.messages;
    messages.reverse();
    messages.push(messages[0].clone());
    let request = ModelRequest {
        invocation: reordered.invocation.clone(),
        messages,
        tools: reordered.tool_definitions.clone(),
        effective_system_prompt: reordered.effective_system_prompt.clone(),
        continuation: None,
        continuation_source: None,
        images: crate::model::images::ResolvedImages::new(),
    };
    reordered
        .freeze_input(store.conversation_id(), &request)
        .unwrap();
    let before = store.conn.request_fragments.lock().unwrap().stats();
    let journal = store.presentation_frontier().unwrap();
    let original = fragment_root_bytes(&store, &initial, journal);
    assert!(
        matches!(
            store.commit_model_turn_start(&[], &reordered, Utc::now()),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "native selection order/occurrence admission stays authoritative"
    );
    assert_eq!(store.conn.request_fragments.lock().unwrap().stats(), before);
    assert_eq!(store.presentation_frontier().unwrap(), journal);
    assert_eq!(fragment_root_bytes(&store, &initial, journal), original);
    let checkpoint = checkpoint_fixture(
        &store,
        "fragment-compaction",
        8,
        "actual Derived model input",
    );
    store
        .commit_projection(checkpoint_commit_fixture(&store, checkpoint, None))
        .unwrap();
    let compacted = fragment_snapshot(&store, 3, 0);
    assert!(
        compacted
            .input
            .iter()
            .any(|input| matches!(input, crate::model::projected::FrozenModelInput::Derived(_)))
    );
    observation_request::take_header_constructions();
    let next = store
        .commit_model_turn_start(&[], &compacted, Utc::now())
        .unwrap();
    let constructions = observation_request::take_header_constructions();
    assert!(
        constructions > 0 && constructions < compacted.input.len(),
        "Derived rebuilt, eligible Originals reused: {constructions}"
    );
    assert_fragment_oracle(&store, &compacted, next.started.sequence);
    let retry = fragment_snapshot(&store, 3, 1);
    let retried = store
        .commit_model_turn_start(&[], &retry, Utc::now())
        .unwrap();
    assert_ne!(retry.request_id, compacted.request_id);
    assert_fragment_oracle(&store, &retry, retried.started.sequence);
}

// Exact canonical text workload from the native reading fixture. This is a
// counter/oracle proof, not an execution or wall-clock performance benchmark.
fn reading_fragment_fixture() -> SqliteConversationStore {
    let store = store();
    let mut messages = Vec::with_capacity(600);
    for index in 0..300 {
        messages.push(user_message(
            &format!("reading-user-{index}"),
            &format!("Reading {index}"),
        ));
        messages.push(assistant_message(
            &format!("reading-assistant-{index}"),
            &format!("Native reading answer {index}\n\nReadable history paragraph."),
        ));
    }
    store.initialize(&messages).unwrap();
    store
}
fn assert_reading_fragment_source(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    sequence: u64,
) {
    let input = store
        .reconstruct_model_request(&snapshot.request_id)
        .unwrap();
    let expected = observation_request::legacy_request_source_bytes(
        &store.lock().unwrap(),
        sequence,
        store.conversation_id(),
        &input.messages,
        snapshot,
    )
    .unwrap();
    assert_eq!(fragment_root_bytes(store, snapshot, sequence), expected);
    let connection = store.lock().unwrap();
    let mut offset = 0;
    loop {
        let page = observation_request::page_at(
            &connection,
            sequence,
            sequence,
            &["messages".into()],
            offset,
            32,
        )
        .unwrap();
        assert_eq!(page.total, 600);
        assert!(!page.fields.is_empty() && page.fields.len() <= 32);
        for field in page.fields {
            let (start, end, _, _) = observation_request::field_from(
                &connection,
                "request_observation_fields",
                &sequence.to_string(),
                &field.path,
            )
            .unwrap();
            assert_eq!(end - start, field.encoded_bytes);
            let start = usize::try_from(start).unwrap();
            let end = usize::try_from(end).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&expected[start..end]).unwrap();
            assert_eq!(field.value, value);
        }
        let Some(next) = page.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
}
#[test]
fn t20_reading_300_pairs_report_actual_canonical_fragment_role_coverage_and_capture_work() {
    let store = reading_fragment_fixture();
    let canonical = store.load_canonical().unwrap();
    let mut previous = 0;
    for turn in 1..=20 {
        let snapshot = fragment_snapshot(&store, turn, 0);
        observation_request::take_header_constructions();
        crate::runtime::read_layout::take_fragment_work();
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        let cache = store.conn.request_fragments.lock().unwrap();
        let stats = cache.stats();
        let sizes = cache.entry_sizes();
        let users = sizes
            .iter()
            .filter(|(position, _)| position % 2 == 1)
            .count();
        eprintln!(
            "reading cache turn={turn} users={users} assistants={} stats={stats:?} header_constructions={} capture_work={:?}",
            sizes.len() - users,
            observation_request::take_header_constructions(),
            crate::runtime::read_layout::take_fragment_work()
        );
        assert!(stats.1 <= 8 * 1024 * 1024 && stats.4 <= 8 * 1024 * 1024);
        assert!(sizes.iter().all(|(_, bytes)| *bytes <= 65_536));
        if stats.0 == 600 || stats.0 == previous {
            break;
        }
        previous = stats.0;
    }
    let sizes = store.conn.request_fragments.lock().unwrap().entry_sizes();
    let before = store.conn.request_fragments.lock().unwrap().stats();
    assert!(sizes.iter().any(|(position, _)| position % 2 == 1));
    assert!(sizes.iter().any(|(position, _)| position % 2 == 0));
    let snapshot = fragment_snapshot(&store, 21, 0);
    observation_request::take_header_constructions();
    crate::runtime::read_layout::take_fragment_work();
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    let work = crate::runtime::read_layout::take_fragment_work();
    let after = store.conn.request_fragments.lock().unwrap().stats();
    assert_eq!(
        after.2 - before.2,
        sizes.len(),
        "every admitted original reused"
    );
    assert_eq!(
        observation_request::take_header_constructions(),
        600 - sizes.len()
    );
    eprintln!("reading cache final entries={sizes:?} stats={after:?} capture_work={work:?}");
    assert_reading_fragment_source(&store, &snapshot, started.sequence);
    assert_eq!(store.load_canonical().unwrap(), canonical);
}

include!("request_shape_tests.rs");

include!("request_page_metadata_tests.rs");

include!("request_hole_tests.rs");

include!("request_root_finalization_tests.rs");
