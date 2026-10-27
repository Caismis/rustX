//! Differential oracle: the pre-optimization byte-reading capture algorithm.
//! Kept test-only; both paths authenticate exactly the same encoded facts.
use super::*;
use crate::runtime::read_layout::{FieldPreview, READ_WORK, take_field_preview_work};

fn legacy_capture(
    connection: &Connection,
    source: &SeekableObservationSource,
    reader: &ObservationByteReader<'_>,
    start: u64,
    end: u64,
) -> Result<(u64, u64), ConversationStoreError> {
    let mut failure = None;
    let mut max_path = 0_u64;
    let mut max_key_path = 0_u64;
    let result = source.visit_fields_selecting_previews(
        |_| FieldPreviewPolicy::Inline,
        |path, _, parent, field_start, field_end, children, key_span, preview| {
            let result = (|| {
                if field_start < start || field_end > end {
                    return Ok(());
                }
                let FieldPreview::Inline(value) = preview else {
                    return Err(storage("composed field preview unavailable"));
                };
                max_path = max_path.max(super::super::super::encoded_path(path)?.len() as u64);
                let header = crate::runtime_client::checkpoint::bounded_bytes(value, 16384)
                    .unwrap_or_else(|| b"null".to_vec());
                let span = field_end - field_start;
                let first = source_bytes(reader, field_start, field_start + u64::from(span > 0))?;
                let nodes = if span <= 16384 {
                    super::super::super::original_node_count(&source_bytes(
                        reader,
                        field_start,
                        field_end,
                    )?)?
                } else {
                    33
                };
                let fact = Fact {
                    path: path.to_vec(),
                    parent: parent
                        .map(|(path, _)| {
                            serde_json::from_str(path)
                                .map_err(|_| storage("composed parent path invalid"))
                        })
                        .transpose()?,
                    ordinal: parent.map(|(_, ordinal)| ordinal),
                    start: field_start - start,
                    end: field_end - start,
                    children,
                    nodes,
                    object: first == b"{",
                    container: first == b"{" || first == b"[",
                    header,
                    key: None,
                    name: key_span
                        .filter(|(start, end)| end - start <= 256)
                        .map(|(start, end)| {
                            source_bytes(reader, start, end).and_then(|bytes| {
                                serde_json::from_slice(&bytes)
                                    .map_err(|_| storage("original field key unavailable"))
                            })
                        })
                        .transpose()?,
                };
                let fact = if path.last().is_some_and(|part| part.starts_with('@')) {
                    Fact {
                        key: Some(
                            key_span
                                .map(|(start, end)| end - start)
                                .ok_or_else(|| storage("original aliased key span unavailable"))?,
                        ),
                        ..fact
                    }
                } else {
                    fact
                };
                legacy_persist_fact(connection, &fact)?;
                if let Some((key_start, key_end)) = key_span
                    && path.last().is_some_and(|part| part.starts_with('@'))
                {
                    max_key_path = max_key_path.max(legacy_persist_key_fact(
                        connection,
                        path,
                        key_start - start,
                        key_end - start,
                    )?);
                }
                Ok(())
            })();
            result.map_err(|error| {
                failure = Some(error);
                "composed fact capture failed"
            })
        },
    );
    result.map_err(|error| failure.unwrap_or_else(|| storage(error)))?;
    Ok((max_path, max_key_path))
}
fn legacy_part(
    connection: &Connection,
    source: &SeekableObservationSource,
    reader: &ObservationByteReader<'_>,
    start: u64,
    end: u64,
) -> Result<Fragment, ConversationStoreError> {
    connection.execute_batch("CREATE TEMP TABLE IF NOT EXISTS request_source_fact_spool(key TEXT PRIMARY KEY,value BLOB NOT NULL) WITHOUT ROWID; DELETE FROM request_source_fact_spool;").map_err(|error|storage(error.to_string()))?;
    let (max_path, max_key_path) = legacy_capture(connection, source, reader, start, end)?;
    let mut builder = index::Builder::new(connection);
    let mut statement = connection
        .prepare_cached("SELECT key,value FROM request_source_fact_spool ORDER BY key")
        .map_err(|error| storage(error.to_string()))?;
    let mut rows = statement
        .query([])
        .map_err(|error| storage(error.to_string()))?;
    while let Some(row) = rows.next().map_err(|error| storage(error.to_string()))? {
        builder.push(
            row.get(0).map_err(|error| storage(error.to_string()))?,
            row.get(1).map_err(|error| storage(error.to_string()))?,
        )?;
    }
    let fields = builder
        .finish()?
        .ok_or_else(|| storage("composed fragment has no fields"))?;
    drop(rows);
    drop(statement);
    connection
        .prepare_cached("DELETE FROM request_source_fact_spool")
        .map_err(|error| storage(error.to_string()))?
        .execute([])
        .map_err(|error| storage(error.to_string()))?;
    let mut builder = index::Builder::new(connection);
    let mut offset = start;
    while offset < end {
        let through = (offset + CHUNK).min(end);
        builder.push(
            chunk_key(offset - start),
            source_bytes(reader, offset, through)?,
        )?;
        offset = through;
    }
    let bytes = builder
        .finish()?
        .ok_or_else(|| storage("empty composed byte fragment"))?;
    #[cfg(test)]
    super::super::WORK.with(|work| {
        let mut value = work.get();
        value.new_fragment_bytes += usize::try_from(end - start).unwrap_or(usize::MAX);
        work.set(value);
    });
    Ok(Fragment {
        bytes,
        fields,
        length: end - start,
        max_path,
        max_key_path,
    })
}

#[derive(serde::Serialize, schemars::JsonSchema)]
struct RawFixture<'a> {
    messages: Vec<u8>,
    revision: u64,
    #[schemars(with = "serde_json::Value")]
    raw: &'a serde_json::value::RawValue,
    tail: &'a str,
}
fn source(raw: &str, tail: &str) -> SeekableObservationSource {
    let raw = serde_json::value::RawValue::from_string(raw.into()).unwrap();
    SeekableObservationSource::new(&RawFixture {
        messages: vec![],
        revision: u64::MAX,
        raw: &raw,
        tail,
    })
    .unwrap()
}
fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    super::super::create_schema(&connection).unwrap();
    connection
}
fn rows(connection: &Connection) -> Vec<(Vec<u8>, Vec<u8>)> {
    connection
        .prepare("SELECT hash,bytes FROM request_source_pages ORDER BY hash")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
#[test]
fn metadata_capture_matches_original_facts_roots_and_page_bytes() {
    let fixtures = [
        r#"{"z":1.00,"a":-0.0,"escape":"\u6c34","u":18446744073709551615,"@0":{"$key":true}}"#.to_owned(),
        serde_json::json!({"dense":(0..70).collect::<Vec<_>>(), "long":"水".repeat(6000), "key".repeat(100): [true,false,null]}).to_string(),
    ];
    for raw in fixtures {
        let source = source(&raw, "suffix");
        let reader = source.byte_reader().unwrap();
        let total = source.read(&[], 0, 1).unwrap().readable_end;
        let (start, end, _) = geometry(&source, &["raw".into()]).unwrap();
        for (start, end) in [(0, total), (0, start), (end, total)] {
            let old = connection();
            let new = connection();
            READ_WORK.with(|work| work.set((0, 0)));
            let expected = legacy_part(&old, &source, &reader, start, end).unwrap();
            let old_reads = READ_WORK.with(|work| work.replace((0, 0)));
            let actual = persist_part(&new, &source, &reader, start, end).unwrap();
            let new_reads = READ_WORK.with(std::cell::Cell::get);
            assert!(
                new_reads.0 < old_reads.0,
                "no redundant span probes: {new_reads:?} vs {old_reads:?}"
            );
            assert!(
                new_reads.1 < old_reads.1,
                "less physical source byte reading"
            );
            assert_eq!(
                serde_json::to_vec(&actual).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
            assert_eq!(
                rows(&new),
                rows(&old),
                "all authenticated page bytes, including original chunks"
            );
            assert_eq!(
                bytes(&new, &actual, 0, actual.length.min(65536)).unwrap(),
                bytes(&old, &expected, 0, expected.length.min(65536)).unwrap()
            );
        }
    }
}
#[test]
fn metadata_capture_preserves_duplicate_and_numeric_error_rejection() {
    let deep = format!("{}0{}", "[".repeat(130), "]".repeat(130));
    for raw in [r#"{"duplicate":1,"duplicate":2}"#, "[1e400]", &deep] {
        let source = source(raw, "suffix");
        let reader = source.byte_reader().unwrap();
        let total = source.read(&[], 0, 1).unwrap().readable_end;
        assert!(legacy_part(&connection(), &source, &reader, 0, total).is_err());
        assert!(persist_part(&connection(), &source, &reader, 0, total).is_err());
    }
}
#[test]
fn range_filter_skips_preview_work_but_keeps_selected_facts() {
    let source = source(
        &serde_json::json!((0..70).collect::<Vec<_>>()).to_string(),
        "suffix",
    );
    let reader = source.byte_reader().unwrap();
    let (_, end, _) = geometry(&source, &["raw".into()]).unwrap();
    let total = source.read(&[], 0, 1).unwrap().readable_end;
    take_field_preview_work();
    legacy_part(&connection(), &source, &reader, end, total).unwrap();
    let old = take_field_preview_work();
    persist_part(&connection(), &source, &reader, end, total).unwrap();
    let new = take_field_preview_work();
    assert_eq!(new.0, 1, "only the tail field needs a preview");
    assert!(old.0 > 70);
    assert_eq!(old.0, new.0 + new.1);
}

fn assert_capture_outcome(source: &SeekableObservationSource) -> bool {
    let reader = source.byte_reader().unwrap();
    let total = source.read(&[], 0, 1).unwrap().readable_end;
    let old = connection();
    let new = connection();
    let expected = legacy_part(&old, source, &reader, 0, total);
    let actual = persist_part(&new, source, &reader, 0, total);
    match (expected, actual) {
        (Ok(expected), Ok(actual)) => {
            assert_eq!(
                serde_json::to_vec(&actual).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
            assert_eq!(
                rows(&new),
                rows(&old),
                "identical authenticated pages prove nodes, previews, paths and byte spans"
            );
            true
        }
        (Err(expected), Err(actual)) => {
            assert_eq!(
                std::mem::discriminant(&actual),
                std::mem::discriminant(&expected)
            );
            false
        }
        (expected, actual) => panic!("capture outcomes differ: {expected:?} vs {actual:?}"),
    }
}
fn assert_validation_spans(source: &SeekableObservationSource, paths: &[&[&str]]) {
    let total = source.read(&[], 0, 1).unwrap().readable_end;
    let expected_bytes = paths
        .iter()
        .map(|path| {
            let path = path
                .iter()
                .map(|part| (*part).to_owned())
                .collect::<Vec<_>>();
            let (start, end, _) = geometry(source, &path).unwrap();
            usize::try_from(end - start).unwrap()
        })
        .sum::<usize>();
    READ_WORK.with(|work| work.set((0, 0)));
    source
        .visit_field_metadata(0, total, |_, _| Ok(()))
        .unwrap();
    let preview_reads = READ_WORK.with(|work| work.replace((0, 0)));
    let connection = connection();
    let mut spool = FactSpool::new(&connection);
    capture_facts(&mut spool, source, &source.byte_reader().unwrap(), 0, total).unwrap();
    let actual = READ_WORK.with(|work| work.replace((0, 0)));
    assert_eq!(
        actual,
        (
            preview_reads.0 + paths.len(),
            preview_reads.1 + expected_bytes
        ),
        "only maximal covered bounded subtrees need additional byte validation"
    );
}
fn dense_subtree() -> serde_json::Value {
    serde_json::json!({"branch":{"rows":(0..70).map(|n| {
        serde_json::json!({"n":n,"@0":{"$key":true},"escaped":"水🙂\"\\\n"})
    }).collect::<Vec<_>>()}})
}
#[test]
fn nested_bounded_subtrees_reuse_validation_without_changing_fragments() {
    let dense = dense_subtree();
    let small = source(&dense.to_string(), "suffix");
    assert!(small.read(&[], 0, 1).unwrap().readable_end < 16_384);
    assert!(assert_capture_outcome(&small));
    assert_validation_spans(&small, &[&[]]);

    let mixed = source(
        &serde_json::json!({"left":dense,"padding":"x".repeat(17_000),"right":dense}).to_string(),
        "suffix",
    );
    assert!(mixed.read(&[], 0, 1).unwrap().readable_end > 16_384);
    assert!(assert_capture_outcome(&mixed));
    assert_validation_spans(&mixed, &[&["raw", "left"], &["raw", "right"]]);

    // Reserved keys receive distinct ordinal aliases. Count the original
    // entries even though serde Value previews normalize equal object keys.
    let aliases = format!(
        "{{{}}}",
        (0..40)
            .map(|n| format!("\"@0\":{n}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let aliases = source(&aliases, "suffix");
    assert!(assert_capture_outcome(&aliases));
    assert_validation_spans(&aliases, &[&[]]);
}
#[test]
fn bounded_validation_still_rejects_hidden_errors_and_uncovered_siblings() {
    let dense = dense_subtree().to_string();
    let deep = format!("{}0{}", "[".repeat(130), "]".repeat(130));
    for invalid in [r#"{"duplicate":1,"duplicate":2}"#, "[1e400]", &deep] {
        let small = source(
            &format!("{{\"first\":{dense},\"late\":{invalid}}}"),
            "suffix",
        );
        assert!(small.read(&[], 0, 1).unwrap().readable_end < 16_384);
        assert!(!assert_capture_outcome(&small));
        let mixed = source(
            &format!(
                "{{\"left\":{dense},\"padding\":\"{}\",\"right\":{invalid}}}",
                "x".repeat(17_000)
            ),
            "suffix",
        );
        assert!(!assert_capture_outcome(&mixed));
    }
}
#[test]
fn bounded_validation_preserves_byte_node_and_serde_depth_boundaries() {
    let dense = dense_subtree().to_string();
    let base = source(&dense, "").read(&[], 0, 1).unwrap().readable_end;
    for length in [16_383, 16_384, 16_385] {
        let fixture = source(&dense, &"x".repeat(usize::try_from(length - base).unwrap()));
        assert_eq!(fixture.read(&[], 0, 1).unwrap().readable_end, length);
        assert!(assert_capture_outcome(&fixture));
        let path: &[&str] = if length <= 16_384 { &[] } else { &["raw"] };
        assert_validation_spans(&fixture, &[path]);
    }
    for children in [30, 31, 32, 33] {
        let fixture = source(
            &serde_json::json!((0..children).collect::<Vec<_>>()).to_string(),
            "",
        );
        assert!(assert_capture_outcome(&fixture));
    }
    let mut outcomes = Vec::new();
    for depth in 123..=130 {
        let fixture = source(&format!("{}0{}", "[".repeat(depth), "]".repeat(depth)), "");
        outcomes.push(assert_capture_outcome(&fixture));
    }
    assert!(outcomes.contains(&true) && outcomes.contains(&false));
}

// Independent original SQL fact encoding/spooling oracle.
fn legacy_persist_key_fact(
    connection: &Connection,
    path: &[String],
    start: u64,
    end: u64,
) -> Result<u64, ConversationStoreError> {
    let mut path = path.to_vec();
    path.push("$key".into());
    let length = super::super::super::encoded_path(&path)?.len() as u64;
    legacy_persist_fact(
        connection,
        &Fact {
            path,
            parent: None,
            ordinal: None,
            start,
            end,
            children: 0,
            nodes: 1,
            object: false,
            container: false,
            header: b"\"\"".to_vec(),
            key: None,
            name: None,
        },
    )?;
    Ok(length)
}

fn legacy_persist_fact(connection: &Connection, fact: &Fact) -> Result<(), ConversationStoreError> {
    let encoded = super::super::super::field_path(&fact.path)?;
    let parent = fact
        .parent
        .as_ref()
        .map(|path| super::super::super::encoded_path(path))
        .transpose()?;
    legacy_persist_encoded_fact(
        connection,
        fact,
        &encoded,
        parent.as_deref().zip(fact.ordinal),
    )
}

fn legacy_persist_encoded_fact(
    connection: &Connection,
    fact: &Fact,
    encoded: &str,
    parent: Option<(&str, u64)>,
) -> Result<(), ConversationStoreError> {
    // The source traversal supplies canonical JSON encoding. Preserve the
    // request locator's independent component/count/encoded-byte limits.
    if fact.path.len() > 256
        || fact.path.iter().any(|part| part.len() > 256)
        || encoded.len() > 4096
    {
        return Err(ConversationStoreError::InvalidReference(
            "request field locator exceeds bound".into(),
        ));
    }
    let bytes = super::super::record::encode(fact, 40960)?;
    let field = format!("F{encoded}");
    let mut statement = connection
        .prepare_cached("INSERT INTO request_source_fact_spool(key,value) VALUES(?1,?2)")
        .map_err(|error| storage(error.to_string()))?;
    statement
        .execute(params![field, bytes])
        .map_err(|error| storage(error.to_string()))?;
    if let Some((parent, ordinal)) = parent {
        // The field index owns the complete Fact. Pagination needs only its path;
        // the authenticated child key already owns the parent/ordinal relation.
        statement
            .execute(params![
                format!("C{parent}:{ordinal:020}"),
                &field.as_bytes()[1..]
            ])
            .map_err(|error| storage(error.to_string()))?;
    }
    Ok(())
}
