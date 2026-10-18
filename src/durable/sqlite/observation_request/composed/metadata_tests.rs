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
                    header: base64::engine::general_purpose::STANDARD.encode(header),
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
                persist_fact(connection, &fact)?;
                if let Some((key_start, key_end)) = key_span
                    && path.last().is_some_and(|part| part.starts_with('@'))
                {
                    max_key_path = max_key_path.max(persist_key_fact(
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
    for raw in [r#"{"duplicate":1,"duplicate":2}"#, r#"[1e400]"#, &deep] {
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
