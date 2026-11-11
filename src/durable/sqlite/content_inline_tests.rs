use super::*;

#[test]
fn immutable_inline_reads_keep_the_selected_revision_when_another_frontier_exists() {
    let connection = connection();
    let key = ("canonical", "revision-isolation", 0, Rep::Utf8Text);
    let bytes = vec![b'a'; 65_550];
    put(&connection, key, 1, &bytes, false).unwrap();
    connection.execute(
        "INSERT INTO content_frontiers(source_kind,source_id,block_index,representation,revision,readable_end,bytes) VALUES('canonical',(SELECT -id FROM content_source_ids WHERE alias='revision-isolation'),0,'t',2,65550,?1)",
        [&vec![b'b'; 65_536]],
    ).unwrap();
    for (start, end) in [(0, 16), (65_530, 65_545)] {
        assert_eq!(
            range_bytes(&connection, key, start, end).unwrap(),
            vec![b'a'; usize::try_from(end - start).unwrap()],
        );
        let expected: Vec<u8> = (start..end)
            .map(|offset| if offset < 65_536 { b'b' } else { b'a' })
            .collect();
        assert_eq!(
            range_bytes_at(&connection, key, 2, start, end).unwrap(),
            expected
        );
        assert!(last_read_work().0 <= 2);
    }
    connection
        .execute(
            "DELETE FROM content_frontiers WHERE source_id=(SELECT -id FROM content_source_ids WHERE alias='revision-isolation') AND revision=1",
            [],
        )
        .unwrap();
    assert!(
        range_bytes(&connection, key, 0, 16).is_err(),
        "a missing selected frontier cannot read another revision"
    );
    assert_eq!(
        range_bytes_at(&connection, key, 2, 0, 16).unwrap(),
        vec![b'b'; 16]
    );
}

fn connection() -> Connection {
    let mut connection = Connection::open_in_memory().unwrap();
    crate::durable::sqlite::schema::create_schema(&mut connection).unwrap();
    connection
}

fn stored_rows(connection: &Connection, id: &str) -> (u64, u64) {
    connection.query_row(
        "SELECT (SELECT count(*) FROM content_frontiers WHERE source_id=?1),(SELECT count(*) FROM content_segments WHERE source_id=?1)",
        [keys::read(connection,"canonical",id).unwrap()], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap()
}

#[test]
fn sqlite_tail_append_keeps_binary_bytes_all_encodings_and_historical_prefixes() {
    for encoding in ["UTF-8", "UTF-16le", "UTF-16be"] {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(&format!("PRAGMA encoding='{encoding}'"))
            .unwrap();
        crate::durable::sqlite::schema::create_schema(&mut connection).unwrap();
        let key = ("live", "binary-tail", 0, Rep::RawJsonUtf8);
        let mut expected = Vec::new();
        let mut cuts = Vec::new();
        for (index, length) in [0, 1, 255, 65_279, 2, 65_537, 3].into_iter().enumerate() {
            let suffix: Vec<u8> = (0..length)
                .map(|n| u8::try_from(n % 256).unwrap())
                .collect();
            let revision = index as u64 + 1;
            put(&connection, key, revision, &suffix, true).unwrap();
            expected.extend(suffix);
            cuts.push((revision, expected.len()));
            for &(cut, end) in &cuts {
                for offset in (0..end).step_by(32_771) {
                    let finish = end.min(offset + 65_536);
                    assert_eq!(
                        range_bytes_at(&connection, key, cut, offset as u64, finish as u64)
                            .unwrap_or_else(|error| panic!("{encoding} revision={revision} cut={cut} range={offset}..{finish}: {error:?}")),
                        expected[offset..finish]
                    );
                    assert!(last_read_work().0 <= SEGMENT_BYTES / STREAM_SEGMENT_BYTES + 1);
                }
            }
        }
        assert_eq!(
            stored_rows(&connection, "binary-tail"),
            (7, expected.len().div_ceil(STREAM_SEGMENT_BYTES) as u64)
        );
    }
}

#[test]
fn sqlite_tail_append_rejects_missing_or_wrong_length_without_publishing_frontier() {
    let connection = connection();
    let key = ("live", "guarded-tail", 0, Rep::Utf8Text);
    put(&connection, key, 1, b"original", true).unwrap();
    for damage in [
        "DELETE FROM content_segments",
        "UPDATE content_segments SET bytes=x'01'",
        "UPDATE content_segments SET bytes='original'",
    ] {
        connection.execute_batch("SAVEPOINT damage").unwrap();
        connection.execute_batch(damage).unwrap();
        assert!(put(&connection, key, 2, b" suffix", true).is_err());
        assert_eq!(stored_rows(&connection, "guarded-tail").0, 1);
        connection
            .execute_batch("ROLLBACK TO damage; RELEASE damage")
            .unwrap();
        assert_eq!(
            range_bytes_at(&connection, key, 1, 0, 8).unwrap(),
            b"original"
        );
    }
    connection.execute_batch("CREATE TEMP TRIGGER reject_tail BEFORE UPDATE ON content_segments BEGIN SELECT RAISE(FAIL,'tail failure'); END; SAVEPOINT failed").unwrap();
    assert!(put(&connection, key, 2, b" suffix", true).is_err());
    connection
        .execute_batch("ROLLBACK TO failed; RELEASE failed")
        .unwrap();
    assert_eq!(stored_rows(&connection, "guarded-tail"), (1, 1));
    assert_eq!(
        range_bytes_at(&connection, key, 1, 0, 8).unwrap(),
        b"original"
    );
}

#[test]
fn append_prior_rejects_regressed_missing_and_inflated_frontiers_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("frontier.sqlite");
    let key = ("live", "guarded-frontier", 0, Rep::Utf8Text);
    let other = (key.0, key.1, 1, key.3);
    let bytes = vec![b'x'; STREAM_SEGMENT_BYTES];
    {
        let mut connection = Connection::open(&path).unwrap();
        crate::durable::sqlite::schema::create_schema(&mut connection).unwrap();
        let transaction = connection.transaction().unwrap();
        put(&transaction, key, 1, &bytes, true).unwrap();
        put(&transaction, other, 3, b"other block", true).unwrap();
        put(&transaction, key, 5, b"", true).unwrap();
        transaction.commit().unwrap();
    }
    let connection = Connection::open(&path).unwrap();
    register_tail_function(&connection).unwrap();
    assert!(put(&connection, key, 4, b"out of order", true).is_err());
    for damage in [
        "UPDATE content_frontiers SET readable_end=100 WHERE block_index=0 AND revision=5",
        "UPDATE content_frontiers SET readable_end=16384 WHERE block_index=0 AND revision=5",
        "DELETE FROM content_frontiers WHERE block_index=0",
        "DELETE FROM content_segments WHERE block_index=0",
    ] {
        connection.execute_batch("SAVEPOINT damage").unwrap();
        connection.execute_batch(damage).unwrap();
        for suffix in [b"".as_slice(), b"new"] {
            assert!(put(&connection, key, 6, suffix, true).is_err(), "{damage}");
        }
        connection
            .execute_batch("ROLLBACK TO damage; RELEASE damage")
            .unwrap();
    }
    put(&connection, key, 6, b"next", true).unwrap();
    assert_eq!(stored_rows(&connection, key.1), (4, 3));
    for revision in [1, 5, 6] {
        assert_eq!(
            range_bytes_at(&connection, key, revision, 0, bytes.len() as u64).unwrap(),
            bytes
        );
    }
    assert_eq!(
        range_bytes_at(
            &connection,
            key,
            6,
            bytes.len() as u64,
            bytes.len() as u64 + 4
        )
        .unwrap(),
        b"next"
    );
    assert_eq!(
        range_bytes_at(&connection, other, 3, 0, 11).unwrap(),
        b"other block"
    );
}

#[test]
fn append_prior_vm_work_is_independent_of_frontier_history() {
    use rusqlite::StatementStatus::VmStep;
    const OLD_MAX: &str = "SELECT COALESCE(MAX(readable_end),0) FROM content_frontiers
        WHERE source_kind=?1 AND source_id=?2 AND block_index=?3 AND representation=?4";
    let mut counts = Vec::new();
    for count in [1u64, 16, 256, 4_096, 16_384] {
        let connection = connection();
        let key = ("live", "selected", 0, Rep::Utf8Text);
        let source_id = keys::write(&connection, key.0, key.1).unwrap();
        keys::write(&connection, "live", "unrelated").unwrap();
        let bindings = params![key.0, source_id, key.2, key.3.key()];
        connection
            .execute(
                "WITH RECURSIVE frames(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM frames WHERE n<?1)
             INSERT INTO content_frontiers
             SELECT 'live',(SELECT -id FROM content_source_ids WHERE alias='unrelated'),0,'t',1,1,NULL
             UNION ALL SELECT 'live',(SELECT -id FROM content_source_ids WHERE alias='selected'),0,'t',n*3,(n+1)/2,NULL FROM frames",
                [count],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO content_segments VALUES('live',(SELECT -id FROM content_source_ids WHERE alias='selected'),0,'t',0,zeroblob(?1))",
                [count.div_ceil(2)],
            )
            .unwrap();
        let mut old = connection.prepare(OLD_MAX).unwrap();
        let expected: u64 = old.query_row(bindings, |row| row.get(0)).unwrap();
        let old_steps = old.get_status(VmStep);
        connection
            .prepare_cached(APPEND_PRIOR_SQL)
            .unwrap()
            .reset_status(VmStep);
        assert_eq!(
            append_prior(&connection, key, 3 * count + 1, STREAM_SEGMENT_BYTES as u64).unwrap(),
            expected
        );
        let steps = connection
            .prepare_cached(APPEND_PRIOR_SQL)
            .unwrap()
            .get_status(VmStep);
        counts.push((count, old_steps, steps));
    }
    eprintln!(
        "SQLite {} frontier/old/new VM steps: {counts:?}",
        rusqlite::version()
    );
    assert!(counts.windows(2).all(|pair| pair[0].2 == pair[1].2));
    let (_, old, current) = counts.last().unwrap();
    assert!(*old > 100 * current);
}

#[test]
fn compact_content_representation_keys_keep_original_public_wire_names() {
    let connection = connection();
    for (index, (rep, wire)) in [
        (Rep::Utf8Text, "utf8_text"),
        (Rep::RawJsonUtf8, "raw_json_utf8"),
        (Rep::BinaryBytes, "binary_bytes"),
        (Rep::CanonicalJsonV1, "canonical_json_v1"),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(serde_json::to_value(rep).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<Rep>(serde_json::json!(wire)).unwrap(),
            rep
        );
        put(
            &connection,
            ("codec", "same-source", 0, rep),
            1,
            &[u8::try_from(index).unwrap()],
            false,
        )
        .unwrap();
        assert_eq!(
            range_bytes(&connection, ("codec", "same-source", 0, rep), 0, 1).unwrap(),
            [u8::try_from(index).unwrap()]
        );
    }
    assert!(
        connection
            .query_row(
                "SELECT rustx_append_content(zeroblob(65536),x'00')",
                [],
                |row| row.get::<_, Vec<u8>>(0)
            )
            .is_err()
    );
    assert!(
        connection
            .query_row("SELECT rustx_append_content('text',x'00')", [], |row| row
                .get::<_, Vec<
                u8,
            >>(
                0
            ))
            .is_err()
    );
    let (count, bytes, distinct): (u64,u64,u64) = connection.query_row("SELECT count(*),sum(length(representation)),count(DISTINCT representation) FROM content_frontiers WHERE source_id=(SELECT -id FROM content_source_ids WHERE alias='same-source')", [], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!((count, bytes, distinct), (4, 4, 4));
}

#[test]
fn immutable_inline_first_preserves_empty_boundary_and_two_slice_reads() {
    let connection = connection();
    for length in [0usize, 1, 65_535, 65_536, 65_537, 131_073] {
        let id = length.to_string();
        let bytes: Vec<u8> = (0..length)
            .map(|n| u8::try_from(n % 251).unwrap())
            .collect();
        let key = ("canonical", id.as_str(), 0, Rep::Utf8Text);
        let before = connection.total_changes();
        put(&connection, key, 1, &bytes, false).unwrap();
        let overflow = length.saturating_sub(1) / SEGMENT_BYTES;
        assert_eq!(stored_rows(&connection, &id), (1, overflow as u64));
        assert_eq!(connection.total_changes() - before, 1 + overflow as u64);
        for offset in [0, length.min(65_535), length.min(65_536), length] {
            let end = length.min(offset + SEGMENT_BYTES);
            let actual = range_bytes(&connection, key, offset as u64, end as u64).unwrap();
            assert_eq!(actual, bytes[offset..end]);
            assert_eq!(
                actual.capacity(),
                end - offset,
                "one bounded output allocation even across physical stores"
            );
            let expected_rows = if offset == end {
                0
            } else {
                (end - 1) / SEGMENT_BYTES - offset / SEGMENT_BYTES + 1
            };
            assert_eq!(last_read_work(), (expected_rows, end - offset));
        }
    }
}

#[test]
fn immutable_inline_first_json_native_and_pending_keep_exact_bytes() {
    #[derive(serde::Serialize, schemars::JsonSchema)]
    struct Native {
        sequence: u64,
        text: String,
        opaque: serde_json::Value,
    }
    let connection = connection();
    let value = Native {
        sequence: u64::MAX,
        text: "水🙂\\\"\n".repeat(20_000),
        opaque: serde_json::json!({"float": -0.0, "integer": u64::MAX}),
    };
    let mut expected = Vec::new();
    crate::observation::structural_json::to_writer(&mut expected, &value).unwrap();
    assert_eq!(
        index_structural_value(&connection, "native", "native", 0, &value).unwrap(),
        expected.len() as u64
    );
    super::super::content_encoding::put_native(
        &connection,
        ("pending", "7", 0, Rep::CanonicalJsonV1),
        9,
        &value,
    )
    .unwrap();
    for offset in (0..expected.len()).step_by(31_337) {
        let end = (offset + 31_337).min(expected.len());
        assert_eq!(
            range_bytes(
                &connection,
                ("native", "native", 0, Rep::CanonicalJsonV1),
                offset as u64,
                end as u64
            )
            .unwrap(),
            expected[offset..end]
        );
        assert_eq!(
            pending_range_bytes(&connection, 7, 9, offset as u64, end as u64).unwrap(),
            expected[offset..end]
        );
    }
    let json = serde_json::json!({"text": "水🙂", "float": -0.0});
    let mut expected = Vec::new();
    crate::runtime::read_layout::canonical::to_writer(&mut expected, &json).unwrap();
    super::super::content_encoding::put_json(
        &connection,
        ("canonical", "json", 0, Rep::CanonicalJsonV1),
        1,
        &json,
        false,
    )
    .unwrap();
    assert_eq!(stored_rows(&connection, "json"), (1, 0));
    assert_eq!(
        range_bytes(
            &connection,
            ("canonical", "json", 0, Rep::CanonicalJsonV1),
            0,
            expected.len() as u64
        )
        .unwrap(),
        expected
    );
}

#[test]
fn immutable_inline_corruption_and_rollback_do_not_change_other_sources() {
    let connection = connection();
    for id in ["selected", "independent"] {
        put(
            &connection,
            ("canonical", id, 0, Rep::Utf8Text),
            1,
            b"original",
            false,
        )
        .unwrap();
    }
    for assignment in ["bytes=NULL", "bytes=x'ff'", "bytes=x''"] {
        connection.execute_batch(&format!("SAVEPOINT corrupt; UPDATE content_frontiers SET {assignment} WHERE source_id=(SELECT -id FROM content_source_ids WHERE alias='selected')")).unwrap();
        assert!(
            range_bytes(
                &connection,
                ("canonical", "selected", 0, Rep::Utf8Text),
                0,
                8
            )
            .is_err()
        );
        assert_eq!(
            range_bytes(
                &connection,
                ("canonical", "independent", 0, Rep::Utf8Text),
                0,
                8
            )
            .unwrap(),
            b"original"
        );
        connection
            .execute_batch("ROLLBACK TO corrupt; RELEASE corrupt")
            .unwrap();
    }
    connection.execute_batch("CREATE TEMP TRIGGER reject_inline BEFORE INSERT ON content_frontiers WHEN NEW.source_id=(SELECT -id FROM content_source_ids WHERE alias='rollback') BEGIN SELECT RAISE(FAIL,'frontier fault'); END; SAVEPOINT capture").unwrap();
    assert!(
        put(
            &connection,
            ("canonical", "rollback", 0, Rep::Utf8Text),
            1,
            &vec![b'x'; 131_073],
            false
        )
        .is_err()
    );
    connection
        .execute_batch("ROLLBACK TO capture; RELEASE capture")
        .unwrap();
    assert_eq!(stored_rows(&connection, "rollback"), (0, 0));
    assert_eq!(
        range_bytes(
            &connection,
            ("canonical", "selected", 0, Rep::Utf8Text),
            0,
            8
        )
        .unwrap(),
        b"original"
    );
}

#[test]
fn inline_layout_keeps_live_revisions_and_independent_raw_evidence() {
    let connection = connection();
    let live = ("live", "stream", 0, Rep::Utf8Text);
    put(&connection, live, 1, b"first", true).unwrap();
    put(&connection, live, 2, b" suffix", true).unwrap();
    assert_eq!(stored_rows(&connection, "stream"), (2, 1));
    assert_eq!(range_bytes(&connection, live, 0, 5).unwrap(), b"first");
    assert_eq!(
        range_bytes(&connection, live, 0, 12).unwrap(),
        b"first suffix"
    );
    let revisions: Vec<(u64,u64)> = connection.prepare("SELECT revision,readable_end FROM content_frontiers WHERE source_id=(SELECT -id FROM content_source_ids WHERE alias='stream') ORDER BY revision").unwrap().query_map([], |row| Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    assert_eq!(revisions, [(1, 5), (2, 12)]);
    transfer(
        &connection,
        &PublicationStreamId::new("stream"),
        "live",
        "audit",
        "audit",
    )
    .unwrap();
    assert_eq!(stored_rows(&connection, "audit"), (1, 0));
    assert_eq!(
        range_bytes(&connection, ("audit", "audit", 0, Rep::Utf8Text), 0, 12).unwrap(),
        b"first suffix"
    );
    let raw = ("canonical", "raw", 0, Rep::RawJsonUtf8);
    put(&connection, raw, 1, br#"{ "n": 1.00 }"#, false).unwrap();
    assert_eq!(stored_rows(&connection, "raw"), (1, 1));
    connection
        .execute("DELETE FROM content_frontiers WHERE source_id=(SELECT -id FROM content_source_ids WHERE alias='raw')", [])
        .unwrap();
    assert_eq!(
        stored_rows(&connection, "raw"),
        (0, 1),
        "raw bytes remain independent evidence of a missing descriptor"
    );
}

#[test]
fn audit_transfer_preserves_large_text_raw_and_completed_json_across_chunk_boundaries() {
    let connection = connection();
    let stream = PublicationStreamId::new("transfer-boundaries");
    let text_parts = [
        "水🙂".repeat(9_000),
        "tailλ".repeat(14_000),
        "end🙂".repeat(1_000),
    ];
    let text = text_parts.concat().into_bytes();
    let text_key = ("live", stream.as_str(), 0, Rep::Utf8Text);
    for (index, part) in text_parts.iter().enumerate() {
        put(
            &connection,
            text_key,
            index as u64 + 1,
            part.as_bytes(),
            true,
        )
        .unwrap();
    }
    assert_eq!(
        range_bytes_at(&connection, text_key, 1, 0, text_parts[0].len() as u64).unwrap(),
        text_parts[0].as_bytes(),
        "later suffixes do not alter the retained prefix"
    );

    let raw_parts = [
        " { \"body\" : \"".to_owned(),
        "arg水".repeat(25_000),
        "\", \"n\": 1e+003 } \n".to_owned(),
    ];
    let raw = raw_parts.concat().into_bytes();
    let raw_key = ("live", stream.as_str(), 1, Rep::RawJsonUtf8);
    for (index, part) in raw_parts.iter().enumerate() {
        put(
            &connection,
            raw_key,
            index as u64 + 1,
            part.as_bytes(),
            true,
        )
        .unwrap();
    }
    let arguments: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let mut canonical = Vec::new();
    crate::runtime::read_layout::canonical::to_writer(&mut canonical, &arguments).unwrap();
    super::super::content_encoding::put_json(
        &connection,
        ("live", stream.as_str(), 1, Rep::CanonicalJsonV1),
        4,
        &arguments,
        false,
    )
    .unwrap();
    assert_ne!(
        raw, canonical,
        "lexical original and completed JSON stay distinct"
    );
    assert!(text.len() > 131_072 && raw.len() > 131_072 && canonical.len() > 131_072);
    let source_rows = stored_rows(&connection, stream.as_str());
    assert_eq!(source_rows.0, 7);
    let overflow_rows = text.len().div_ceil(STREAM_SEGMENT_BYTES) - 1
        + raw.len().div_ceil(STREAM_SEGMENT_BYTES)
        + canonical.len().div_ceil(SEGMENT_BYTES)
        - 1;

    connection
        .execute_batch("SAVEPOINT audit_transfer")
        .unwrap();
    transfer(&connection, &stream, "live", "audit", "rolled-back-audit").unwrap();
    assert_eq!(
        stored_rows(&connection, "rolled-back-audit"),
        (3, overflow_rows as u64)
    );
    connection
        .execute_batch("ROLLBACK TO audit_transfer; RELEASE audit_transfer")
        .unwrap();
    assert_eq!(stored_rows(&connection, "rolled-back-audit"), (0, 0));
    assert_eq!(stored_rows(&connection, stream.as_str()), source_rows);

    transfer(&connection, &stream, "live", "audit", "retained-audit").unwrap();
    clear_live(&connection, &stream).unwrap();
    assert_eq!(stored_rows(&connection, stream.as_str()), (0, 0));
    assert_eq!(
        stored_rows(&connection, "retained-audit"),
        (3, overflow_rows as u64)
    );
    assert_inline_audit_layout(&connection);
    assert_transferred_windows(&connection, &text, &raw, &canonical);
}

fn assert_inline_audit_layout(connection: &Connection) {
    let first_chunks: Vec<String> = connection
        .prepare("SELECT representation FROM content_segments WHERE source_kind='audit' AND source_id=(SELECT -id FROM content_source_ids WHERE alias='retained-audit') AND segment=0 ORDER BY representation")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(first_chunks, ["r"]);
    let inline: Vec<(String, bool)> = connection
        .prepare("SELECT representation,bytes IS NOT NULL FROM content_frontiers WHERE source_kind='audit' AND source_id=(SELECT -id FROM content_source_ids WHERE alias='retained-audit') ORDER BY representation")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        inline,
        [("j".into(), true), ("r".into(), false), ("t".into(), true),]
    );
}

fn assert_transferred_windows(connection: &Connection, text: &[u8], raw: &[u8], canonical: &[u8]) {
    for (block, representation, expected) in [
        (0, Rep::Utf8Text, text),
        (1, Rep::RawJsonUtf8, raw),
        (1, Rep::CanonicalJsonV1, canonical),
    ] {
        for start in [
            0,
            1,
            65_530,
            65_535,
            65_536,
            65_537,
            131_065,
            expected.len() - 1,
            expected.len(),
        ] {
            let end = (start + SEGMENT_BYTES).min(expected.len());
            assert_eq!(
                range_bytes(
                    connection,
                    ("audit", "retained-audit", block, representation),
                    start as u64,
                    end as u64,
                )
                .unwrap(),
                expected[start..end],
            );
            let (rows, bytes) = last_read_work();
            assert!(
                rows <= SEGMENT_BYTES / segment_bytes("audit", representation) + 1
                    && bytes == end - start
                    && bytes <= SEGMENT_BYTES
            );
        }
    }
}

thread_local! {
    static APPEND_WORK: std::cell::Cell<(usize, usize, usize)> = const {
        std::cell::Cell::new((0, 0, 0))
    };
}

pub(super) fn record_tail_copy(bytes: usize) {
    let (updates, inserts, copied) = APPEND_WORK.get();
    APPEND_WORK.set((updates + 1, inserts, copied + bytes));
}

pub(super) fn record_segment_insert() {
    let (updates, inserts, copied) = APPEND_WORK.get();
    APPEND_WORK.set((updates, inserts + 1, copied));
}

#[derive(Debug)]
struct LayoutWork {
    frames: u64,
    statements: usize,
    rebuilt: usize,
    read_rows: usize,
}

fn stream_layout_work(width: usize, suffix_size: usize) -> LayoutWork {
    const OUTPUT: usize = 256 * 1024;
    let directory = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(directory.path().join("stream.sqlite")).unwrap();
    crate::durable::sqlite::schema::configure_connection(&mut connection, false).unwrap();
    connection
        .execute_batch("PRAGMA wal_autocheckpoint=0")
        .unwrap();
    crate::durable::sqlite::schema::create_schema(&mut connection).unwrap();
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let key = ("live", "layout-probe-stream", 0, Rep::Utf8Text);
    let output: Vec<_> = (0..OUTPUT)
        .map(|offset| u8::try_from(offset % 251).unwrap())
        .collect();
    APPEND_WORK.set((0, 0, 0));
    for (index, suffix) in output.chunks_exact(suffix_size).enumerate() {
        let transaction = connection.transaction().unwrap();
        put_segmented(&transaction, key, index as u64 + 1, suffix, true, width).unwrap();
        transaction.commit().unwrap();
    }
    let (updates, inserts, rebuilt) = APPEND_WORK.replace((0, 0, 0));
    let wal_bytes = std::fs::metadata(directory.path().join("stream.sqlite-wal"))
        .unwrap()
        .len();
    let page_size: u64 = connection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .unwrap();
    assert_eq!(page_size, 2048);
    assert_eq!((wal_bytes - 32) % (page_size + 24), 0);
    let revisions = OUTPUT / suffix_size;
    assert_eq!(
        stored_rows(&connection, key.1),
        (revisions as u64, OUTPUT.div_ceil(width) as u64)
    );
    // Read old cuts after all appends. Include the worst-aligned maximum
    // window and the last bytes of each prefix without replaying old frames.
    let mut read_rows = 0;
    for revision in 1..=revisions {
        let frontier: usize = connection
            .query_row(
                "SELECT readable_end FROM content_frontiers WHERE source_kind='live' AND source_id=?1 AND block_index=0 AND representation='t' AND revision=?2",
                params![keys::read(&connection,key.0,key.1).unwrap(), revision],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(frontier, revision * suffix_size);
        let offset = frontier.saturating_sub(SEGMENT_BYTES);
        assert_eq!(
            range_bytes_segmented(
                &connection,
                key,
                revision as u64,
                offset as u64,
                frontier as u64,
                width as u64
            )
            .unwrap(),
            output[offset..frontier]
        );
        let (rows, bytes) = last_read_work();
        assert!(rows <= SEGMENT_BYTES / width + 1);
        assert_eq!(bytes, frontier - offset);
        read_rows = read_rows.max(rows);
    }
    assert_eq!(
        range_bytes_segmented(
            &connection,
            key,
            revisions as u64,
            1,
            SEGMENT + 1,
            width as u64,
        )
        .unwrap(),
        output[1..=SEGMENT_BYTES]
    );
    let (unaligned_rows, bytes) = last_read_work();
    assert_eq!(unaligned_rows, SEGMENT_BYTES / width + 1);
    assert_eq!(bytes, SEGMENT_BYTES);
    read_rows = read_rows.max(unaligned_rows);
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    LayoutWork {
        frames: (wal_bytes - 32) / (page_size + 24),
        statements: updates + inserts + 2 * revisions,
        rebuilt,
        read_rows,
    }
}

#[test]
fn streaming_segment_layout_balances_wal_work_and_bounded_reads() {
    for suffix in [1_024, 8_192, 65_536] {
        let layouts = [4_096, 8_192, 16_384, 65_536].map(|width| stream_layout_work(width, suffix));
        let [small, chosen, medium, baseline] = &layouts;
        eprintln!(
            "SQLite {} suffix={suffix} 4/8/16/64 KiB: {layouts:?}",
            rusqlite::version()
        );
        assert_eq!(STREAM_SEGMENT_BYTES, 8_192);
        assert!(chosen.read_rows <= 9 && medium.read_rows <= 5 && baseline.read_rows <= 2);
        assert!(chosen.read_rows <= small.read_rows);
        match suffix {
            1_024 => {
                assert_eq!(chosen.statements, baseline.statements);
                assert!(chosen.frames < baseline.frames && chosen.rebuilt < baseline.rebuilt);
            }
            8_192 => {
                assert_eq!(chosen.statements, baseline.statements);
                assert!(chosen.statements < small.statements && chosen.frames < baseline.frames);
                assert_eq!(chosen.rebuilt, 0);
            }
            _ => assert_eq!(chosen.rebuilt, 0),
        }
    }
}

// This isolates the physical Content index, not the other objects a turn writes.
fn frontier_write_cost(history: u64, tool: bool) -> (usize, usize, i32) {
    use rusqlite::StatementStatus;
    use std::collections::BTreeSet;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("frontier.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    crate::durable::sqlite::schema::configure_connection(&mut connection, false).unwrap();
    crate::durable::sqlite::schema::create_schema(&mut connection).unwrap();
    connection
        .execute_batch("PRAGMA wal_autocheckpoint=0")
        .unwrap();
    let insert = |connection: &Connection, turn: u64, start: usize, end: usize| {
        let kinds = [
            "trace_terminal",
            "trace_terminal",
            "canonical",
            "cb",
            "cm",
            "response_identity",
            "attempt_model",
            "trace_request",
        ];
        for (ordinal, kind) in kinds.iter().enumerate().take(end).skip(start) {
            let multiplier =
                if matches!(*kind, "trace_terminal" | "trace_request" | "attempt_model") {
                    8
                } else {
                    2
                };
            let source = turn * multiplier + (ordinal % 2) as u64;
            connection.execute(
                "INSERT INTO content_frontiers(source_kind,source_id,block_index,representation,revision,readable_end,bytes) VALUES(?1,?2,?3,'j',1,16,?4)",
                params![kind,source,(ordinal % 3) as u64,b"0123456789abcdef".as_slice()],
            ).unwrap();
        }
    };
    let transaction = connection.transaction().unwrap();
    for turn in 0..history {
        insert(&transaction, turn, 0, 8);
    }
    transaction.commit().unwrap();
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let goal_root: u32 = connection
        .query_row(
            "SELECT rootpage FROM sqlite_schema WHERE name='content_frontiers_goal_idx'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    for turn in history..history + if tool { 2 } else { 1 } {
        for start in (0..8).step_by(2) {
            let transaction = connection.transaction().unwrap();
            insert(&transaction, turn, start, start + 2);
            transaction.commit().unwrap();
        }
    }
    let wal = std::fs::read(path.with_file_name("frontier.sqlite-wal")).unwrap();
    let pages: Vec<u32> = wal[32..]
        .as_chunks::<2072>()
        .0
        .iter()
        .map(|frame| u32::from_be_bytes(frame[..4].try_into().unwrap()))
        .collect();
    assert!(
        !pages.contains(&goal_root),
        "ordinary content must not write the goal-only index"
    );
    let distinct = pages.iter().copied().collect::<BTreeSet<_>>().len();
    let mut query = connection.prepare("SELECT readable_end FROM content_frontiers WHERE source_kind='canonical' AND source_id=?1 AND block_index=2 AND representation='j' AND revision=1").unwrap();
    assert_eq!(
        query
            .query_row([history * 2], |row| row.get::<_, u64>(0))
            .unwrap(),
        16
    );
    assert_eq!(query.get_status(StatementStatus::FullscanStep), 0);
    assert_eq!(query.get_status(StatementStatus::Sort), 0);
    (
        pages.len(),
        distinct,
        query.get_status(StatementStatus::VmStep),
    )
}

#[test]
fn frontier_index_writes_and_lookup_stay_bounded_with_history() {
    for tool in [false, true] {
        let costs: Vec<_> = [64, 512, 2048]
            .into_iter()
            .map(|h| frontier_write_cost(h, tool))
            .collect();
        eprintln!("frontier index tool={tool}, H=64/512/2048: {costs:?}");
        let (min_frames, max_frames) = costs
            .iter()
            .map(|c| c.0)
            .fold((usize::MAX, 0), |(min, max), n| (min.min(n), max.max(n)));
        let (min_pages, max_pages) = costs
            .iter()
            .map(|c| c.1)
            .fold((usize::MAX, 0), |(min, max), n| (min.min(n), max.max(n)));
        assert!(max_frames - min_frames <= 12, "{costs:?}");
        assert!(max_pages - min_pages <= 8, "{costs:?}");
        assert!(costs.iter().all(|c| c.2 == costs[0].2), "{costs:?}");
    }
}

#[test]
fn goal_cleanup_uses_its_partial_index_without_touching_other_sources() {
    use rusqlite::StatementStatus;

    let connection = connection();
    for id in 0..2048 {
        connection.execute("INSERT INTO content_frontiers(source_kind,source_id,block_index,representation,revision,readable_end) VALUES('canonical',?1,0,'j',1,1)", [id]).unwrap();
    }
    connection.execute("INSERT INTO content_frontiers(source_kind,source_id,block_index,representation,revision,readable_end) VALUES('observation_goal',-1,0,'j',1,1)", []).unwrap();
    let mut statement = connection.prepare("DELETE FROM content_frontiers INDEXED BY content_frontiers_goal_idx WHERE source_kind='observation_goal'").unwrap();
    assert_eq!(statement.execute([]).unwrap(), 1);
    assert_eq!(statement.get_status(StatementStatus::FullscanStep), 0);
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM content_frontiers", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        2048
    );
}
