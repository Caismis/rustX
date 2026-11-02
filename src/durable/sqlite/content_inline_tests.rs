use super::*;

#[test]
fn immutable_inline_reads_keep_the_selected_revision_when_another_frontier_exists() {
    let connection = connection();
    let key = ("canonical", "revision-isolation", 0, Rep::Utf8Text);
    let bytes = vec![b'a'; 65_550];
    put(&connection, key, 1, &bytes, false).unwrap();
    connection.execute(
        "INSERT INTO content_frontiers(source_kind,source_id,block_index,representation,revision,readable_end,bytes) VALUES('canonical','revision-isolation',0,'t',2,65550,?1)",
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
            "DELETE FROM content_frontiers WHERE source_id='revision-isolation' AND revision=1",
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
    super::super::create_schema(&mut connection).unwrap();
    connection
}

fn stored_rows(connection: &Connection, id: &str) -> (u64, u64) {
    connection.query_row(
        "SELECT (SELECT count(*) FROM content_frontiers WHERE source_id=?1),(SELECT count(*) FROM content_segments WHERE source_id=?1)",
        [id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap()
}

#[test]
fn sqlite_tail_append_keeps_binary_bytes_all_encodings_and_historical_prefixes() {
    for encoding in ["UTF-8", "UTF-16le", "UTF-16be"] {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(&format!("PRAGMA encoding='{encoding}'"))
            .unwrap();
        super::super::create_schema(&mut connection).unwrap();
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
                    assert!(last_read_work().0 <= 2);
                }
            }
        }
        assert_eq!(stored_rows(&connection, "binary-tail"), (7, 3));
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
    let (count, bytes, distinct): (u64,u64,u64) = connection.query_row("SELECT count(*),sum(length(representation)),count(DISTINCT representation) FROM content_frontiers WHERE source_id='same-source'", [], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
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
    crate::runtime_client::structural_json::to_writer(&mut expected, &value).unwrap();
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
        connection.execute_batch(&format!("SAVEPOINT corrupt; UPDATE content_frontiers SET {assignment} WHERE source_id='selected'")).unwrap();
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
    connection.execute_batch("CREATE TEMP TRIGGER reject_inline BEFORE INSERT ON content_frontiers WHEN NEW.source_id='rollback' BEGIN SELECT RAISE(FAIL,'frontier fault'); END; SAVEPOINT capture").unwrap();
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
    let revisions: Vec<(u64,u64)> = connection.prepare("SELECT revision,readable_end FROM content_frontiers WHERE source_id='stream' ORDER BY revision").unwrap().query_map([], |row| Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
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
        .execute("DELETE FROM content_frontiers WHERE source_id='raw'", [])
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
    let overflow_rows = text.len().div_ceil(SEGMENT_BYTES) - 1
        + raw.len().div_ceil(SEGMENT_BYTES)
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
        .prepare("SELECT representation FROM content_segments WHERE source_kind='audit' AND source_id='retained-audit' AND segment=0 ORDER BY representation")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(first_chunks, ["r"]);
    let inline: Vec<(String, bool)> = connection
        .prepare("SELECT representation,bytes IS NOT NULL FROM content_frontiers WHERE source_kind='audit' AND source_id='retained-audit' ORDER BY representation")
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
            assert!(rows <= 2 && bytes == end - start && bytes <= SEGMENT_BYTES);
        }
    }
}
