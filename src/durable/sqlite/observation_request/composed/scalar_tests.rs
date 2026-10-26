//! Compare deferred facts with the existing indexed producer, including exact
//! raw spelling and synthetic key paths; page hashes remain the trust boundary.
use super::*;
use crate::runtime::read_layout::{take_field_preview_work, take_file_work};

#[derive(serde::Serialize, schemars::JsonSchema)]
struct RawFixture<'a> {
    messages: Vec<u8>,
    revision: u64,
    #[schemars(with = "serde_json::Value")]
    raw: &'a serde_json::value::RawValue,
}
fn source(raw: &str) -> SeekableObservationSource {
    let raw = serde_json::value::RawValue::from_string(raw.into()).unwrap();
    SeekableObservationSource::new(&RawFixture {
        messages: vec![],
        revision: u64::MAX,
        raw: &raw,
    })
    .unwrap()
}
fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    super::super::create_schema(&connection).unwrap();
    connection
}
fn page_count(connection: &Connection) -> u64 {
    connection
        .query_row("SELECT count(*) FROM request_source_pages", [], |row| {
            row.get(0)
        })
        .unwrap()
}
fn compare(source: &SeekableObservationSource) {
    let old = connection();
    let new = connection();
    let indexed = fragments::whole(&old, source).unwrap();
    take_field_preview_work();
    let scalar = whole(&new, source).unwrap();
    assert_eq!(
        take_field_preview_work().0,
        0,
        "capture derives no previews"
    );
    assert!(matches!(scalar, Scalar::Original { .. }));
    assert!(
        page_count(&new) < page_count(&old),
        "no persisted field index"
    );
    assert_eq!(
        new.query_row(
            "SELECT count(*) FROM sqlite_temp_schema WHERE name='request_source_fact_spool'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        0
    );
    take_file_work();
    let original = Original::load(&new, &scalar).unwrap().unwrap();
    assert_eq!(take_file_work(), (0, 0), "bounded source opens no files");
    let exact = source.indexed_span(0, indexed.length, MAX_BYTES).unwrap();
    assert_eq!(original.bytes(0, scalar.length()).unwrap(), exact);
    assert_eq!(
        fragments::bytes(&old, &indexed, 0, indexed.length).unwrap(),
        exact
    );
    let mut fields = Vec::new();
    source
        .visit_field_geometry(|field| {
            fields.push((field.path.to_vec(), field.start, field.end, field.children));
            if field.path.last().is_some_and(|part| part.starts_with('@')) {
                let mut path = field.path.to_vec();
                path.push("$key".into());
                let (start, end) = field.key_span.unwrap();
                fields.push((path, start, end, 0));
            }
            Ok(())
        })
        .unwrap();
    for (path, start, end, children) in fields {
        let expected = fragments::fact(&old, &indexed, &path).unwrap().unwrap();
        let actual = original.fact(&path).unwrap().unwrap();
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&expected).unwrap(),
            "{path:?}"
        );
        assert_eq!(
            original.bytes(start, end).unwrap(),
            exact[usize::try_from(start).unwrap()..usize::try_from(end).unwrap()],
            "{path:?}"
        );
        for ordinal in 0..=children {
            assert_eq!(
                original.child(&path, ordinal).unwrap(),
                fragments::child(&old, &indexed, &path, ordinal).unwrap(),
                "{path:?} child {ordinal}"
            );
        }
    }
    for path in [
        vec!["missing".into()],
        vec!["revision".into(), "$key".into()],
    ] {
        assert!(original.fact(&path).unwrap().is_none());
    }
}

#[test]
fn original_scalar_matches_indexed_facts_children_keyspans_and_exact_bytes() {
    let fixtures = [
        r#" { "z":1.00, "a":-0.0, "escape":"\u6c34", "u":18446744073709551615, "@0":{"$key":true} } "#.to_owned(),
        r#"{"$key":1,"$key":2,"@0":3,"":4,"nested":[null,true,false,{"a":[]}]}"#.to_owned(),
        serde_json::json!({"dense":(0..70).collect::<Vec<_>>(), "key".repeat(100): [true,false,null], "escaped":"水🙂\n\"\\"}).to_string(),
        serde_json::json!({"wide": (0..80).map(|i| (format!("field_{i}"), serde_json::json!(i))).collect::<serde_json::Map<_, _>>(), "long":"水".repeat(3000)}).to_string(),
    ];
    for raw in fixtures {
        compare(&source(&raw));
    }
}

#[test]
fn original_scalar_boundary_uses_bounded_source_and_preserves_indexed_bytes_above_it() {
    for length in [16_383, 16_384, 16_385] {
        let source = SeekableObservationSource::new(&"a".repeat(length - 2)).unwrap();
        let connection = connection();
        let scalar = whole(&connection, &source).unwrap();
        assert_eq!(scalar.length(), length as u64);
        let exact = source.indexed_span(0, length as u64, 65_536).unwrap();
        match &scalar {
            Scalar::Original { .. } => {
                assert!(length <= 16_384);
                take_file_work();
                let original = Original::load(&connection, &scalar).unwrap().unwrap();
                assert_eq!(take_file_work(), (0, 0));
                assert_eq!(original.bytes(0, length as u64).unwrap(), exact);
                let expected = fragments::whole(&connection, &source).unwrap();
                assert_eq!(
                    serde_json::to_vec(&original.fact(&[]).unwrap()).unwrap(),
                    serde_json::to_vec(&fragments::fact(&connection, &expected, &[]).unwrap())
                        .unwrap()
                );
            }
            Scalar::Indexed { fragment } => {
                assert_eq!(length, 16_385);
                assert!(Original::load(&connection, &scalar).unwrap().is_none());
                assert_eq!(
                    fragments::bytes(&connection, fragment, 0, fragment.length).unwrap(),
                    exact
                );
            }
        }
    }
}

#[test]
fn original_scalar_rejects_duplicate_numeric_depth_and_locator_errors_before_writing() {
    let deep = format!("{}0{}", "[".repeat(130), "]".repeat(130));
    let key = "k".repeat(240);
    let mut long_path = "0".to_owned();
    for _ in 0..18 {
        long_path = format!("{{\"{key}\":{long_path}}}");
    }
    for raw in [
        r#"{"duplicate":1,"duplicate":2}"#,
        "[1e400]",
        &deep,
        &long_path,
    ] {
        let source = source(raw);
        assert!(
            fragments::whole(&connection(), &source).is_err(),
            "indexed admits {raw}"
        );
        let connection = connection();
        assert!(
            whole(&connection, &source).is_err(),
            "original admits {raw}"
        );
        assert_eq!(
            page_count(&connection),
            0,
            "validate before first byte page write"
        );
    }
}

#[test]
fn original_scalar_authenticates_pages_and_gates_malformed_extents_and_sql_cells() {
    let connection = connection();
    let scalar = whole(&connection, &source(r#"{"a":1,"b":[2,3]}"#)).unwrap();
    let Scalar::Original { bytes, length } = &scalar else {
        panic!("expected original");
    };
    for extent in [0, length - 1, length + 1, 16_385] {
        let changed = Scalar::Original {
            bytes: bytes.clone(),
            length: extent,
        };
        assert!(Original::load(&connection, &changed).is_err());
    }
    for (sql, read_bytes) in [
        ("UPDATE request_source_pages SET bytes=x'7b7d'", 2),
        ("UPDATE request_source_pages SET bytes=zeroblob(65537)", 0),
        ("UPDATE request_source_pages SET bytes='{}'", 0),
        ("DELETE FROM request_source_pages", 0),
    ] {
        connection
            .execute_batch("PRAGMA ignore_check_constraints=ON; SAVEPOINT corrupt_scalar")
            .unwrap();
        connection.execute_batch(sql).unwrap();
        super::super::take_work();
        assert!(Original::load(&connection, &scalar).is_err(), "{sql}");
        assert_eq!(super::super::take_work().index_bytes, read_bytes, "{sql}");
        connection.execute_batch("ROLLBACK TO corrupt_scalar; RELEASE corrupt_scalar; PRAGMA ignore_check_constraints=OFF").unwrap();
        assert!(Original::load(&connection, &scalar).is_ok());
    }
    // Recomputed hashes do not bypass byte syntax, path or numeric validation.
    for invalid in [b"[1e400]".as_slice(), br#"{"a":1,"a":2}"#, b"{}garbage"] {
        let mut builder = index::Builder::new(&connection);
        builder
            .push(format!("{:020}", 0), invalid.to_vec())
            .unwrap();
        let changed = Scalar::Original {
            bytes: builder.finish().unwrap().unwrap(),
            length: invalid.len() as u64,
        };
        assert!(Original::load(&connection, &changed).is_err());
    }
}

#[test]
fn original_scalar_savepoint_rollback_and_reopen_preserve_exact_original() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scalar.sqlite");
    let source = source(r#"{"a":1.00,"b":"\u6c34","$key":{"@0":-0.0}}"#);
    let expected = source
        .indexed_span(0, source.read(&[], 0, 1).unwrap().readable_end, MAX_BYTES)
        .unwrap();
    let stored;
    {
        let connection = Connection::open(&path).unwrap();
        super::super::create_schema(&connection).unwrap();
        connection
            .execute_batch("SAVEPOINT scalar_capture")
            .unwrap();
        whole(&connection, &source).unwrap();
        assert!(page_count(&connection) > 0);
        connection
            .execute_batch("ROLLBACK TO scalar_capture; RELEASE scalar_capture")
            .unwrap();
        assert_eq!(page_count(&connection), 0);
        let scalar = whole(&connection, &source).unwrap();
        stored = serde_json::to_vec(&scalar).unwrap();
    }
    let connection = Connection::open(&path).unwrap();
    let scalar: Scalar = serde_json::from_slice(&stored).unwrap();
    let original = Original::load(&connection, &scalar).unwrap().unwrap();
    assert_eq!(original.bytes(0, scalar.length()).unwrap(), expected);
    let indexed = fragments::whole(&connection, &source).unwrap();
    assert_eq!(
        serde_json::to_vec(
            &original
                .fact(&["raw".into(), "@2".into(), "$key".into()])
                .unwrap()
        )
        .unwrap(),
        serde_json::to_vec(
            &fragments::fact(
                &connection,
                &indexed,
                &["raw".into(), "@2".into(), "$key".into()]
            )
            .unwrap()
        )
        .unwrap()
    );
}
