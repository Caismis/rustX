use super::*;
use crate::runtime::read_layout::take_field_preview_work;

#[derive(serde::Serialize, schemars::JsonSchema)]
struct Split<'a> {
    #[schemars(with = "serde_json::Value")]
    prefix: &'a serde_json::value::RawValue,
    provenance_source: serde_json::Value,
    #[schemars(with = "serde_json::Value")]
    suffix: &'a serde_json::value::RawValue,
}
fn source(prefix: &str, suffix: &str) -> SeekableObservationSource {
    let prefix = serde_json::value::RawValue::from_string(prefix.into()).unwrap();
    let suffix = serde_json::value::RawValue::from_string(suffix.into()).unwrap();
    SeekableObservationSource::new(&Split {
        prefix: &prefix,
        provenance_source: serde_json::json!({"sequence":"1","rank":"0","nested":[1,2]}),
        suffix: &suffix,
    })
    .unwrap()
}
fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    super::super::create_schema(&connection).unwrap();
    super::super::install_workspace(&connection).unwrap();
    connection
}
fn original_parts(
    connection: &Connection,
    source: &SeekableObservationSource,
) -> Result<(Fragment, Fragment), ConversationStoreError> {
    let (start, end, _) = geometry(source, &["provenance_source".into()])?;
    let total = source.read(&[], 0, 1).unwrap().readable_end;
    let reader = source.byte_reader().unwrap();
    Ok((
        persist_part(connection, source, &reader, 0, start)?,
        persist_part(connection, source, &reader, end, total)?,
    ))
}
fn fused_parts(
    connection: &Connection,
    source: &SeekableObservationSource,
) -> Result<(Fragment, Fragment), ConversationStoreError> {
    let total = source.read(&[], 0, 1).unwrap().readable_end;
    let (prefix, suffix) = persist_provenance_parts(
        connection,
        source,
        &source.byte_reader().unwrap(),
        total,
        true,
    )?;
    Ok((prefix.unwrap(), suffix))
}

#[test]
fn request_fragment_split_visits_geometry_once_and_preserves_both_authenticated_parts() {
    let dense = serde_json::json!(
        (0..350)
            .map(|n| { serde_json::json!({"n":n,"@0":{"$key":true},"escaped":"水🙂\"\\\n"}) })
            .collect::<Vec<_>>()
    )
    .to_string();
    for (prefix, suffix) in [
        (
            r#"{"z":1.00,"@0":{"$key":-0.0},"u":18446744073709551615}"#,
            r#"{"tail":[true,false,null]}"#,
        ),
        (dense.as_str(), dense.as_str()),
    ] {
        let source = source(prefix, suffix);
        let old = connection();
        let new = connection();
        take_field_preview_work();
        let expected = original_parts(&old, &source).unwrap();
        let old_work = take_field_preview_work();
        let actual = fused_parts(&new, &source).unwrap();
        let new_work = take_field_preview_work();
        assert_eq!(old_work, (0, new_work.1 * 3));
        assert_eq!(new_work.0, 0);
        assert!(new_work.1 > 0);
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        for (actual, expected) in [(&actual.0, &expected.0), (&actual.1, &expected.1)] {
            for offset in (0..actual.length).step_by(8192) {
                let end = (offset + 8192).min(actual.length);
                assert_eq!(
                    bytes(&new, actual, offset, end).unwrap(),
                    bytes(&old, expected, offset, end).unwrap()
                );
            }
        }
        assert_eq!(
            new.query_row(
                "SELECT count(*) FROM request_source_fact_spool",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
        let suffix_only = connection();
        let total = source.read(&[], 0, 1).unwrap().readable_end;
        take_field_preview_work();
        let (prefix, suffix) = persist_provenance_parts(
            &suffix_only,
            &source,
            &source.byte_reader().unwrap(),
            total,
            false,
        )
        .unwrap();
        assert!(prefix.is_none());
        assert_eq!(take_field_preview_work(), new_work);
        assert_eq!(
            serde_json::to_vec(&suffix).unwrap(),
            serde_json::to_vec(&expected.1).unwrap()
        );
    }
}

#[test]
fn request_fragment_split_keeps_numeric_depth_and_alias_contract() {
    let deep = format!("{}0{}", "[".repeat(130), "]".repeat(130));
    for invalid in [r#"{"duplicate":1,"duplicate":2}"#, "[1e400]", deep.as_str()] {
        for (prefix, suffix) in [(invalid, "null"), ("null", invalid)] {
            let source = source(prefix, suffix);
            let old = original_parts(&connection(), &source).unwrap_err();
            let new = fused_parts(&connection(), &source).unwrap_err();
            assert_eq!(std::mem::discriminant(&new), std::mem::discriminant(&old));
        }
    }
    // Reserved duplicate keys have distinct ordinal aliases; they must not be
    // mistaken for duplicate ordinary paths by either routed spool.
    let aliases = r#"{"@0":1,"@0":2,"$key":3}"#;
    let source = source(aliases, aliases);
    assert_eq!(
        serde_json::to_vec(&fused_parts(&connection(), &source).unwrap()).unwrap(),
        serde_json::to_vec(&original_parts(&connection(), &source).unwrap()).unwrap()
    );
}

#[test]
fn request_fragment_split_spill_failure_rolls_back_prefix_pages_and_suffix_workspace() {
    let dense = serde_json::json!((0..350).collect::<Vec<_>>()).to_string();
    let source = source(&dense, &dense);
    let connection = connection();
    connection
        .execute_batch(
            "CREATE TEMP TRIGGER reject_suffix BEFORE INSERT ON request_source_fact_spool
         WHEN NEW.key='F[\"suffix\"]' BEGIN SELECT RAISE(FAIL,'suffix spool unavailable'); END;",
        )
        .unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    assert!(fused_parts(&transaction, &source).is_err());
    assert!(
        transaction
            .query_row("SELECT count(*) FROM request_source_pages", [], |row| row
                .get::<_, u64>(
                0
            ))
            .unwrap()
            > 0,
        "the first spool completed before the second spilled"
    );
    transaction.rollback().unwrap();
    for table in ["request_source_pages", "request_source_fact_spool"] {
        assert_eq!(
            connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
    connection
        .execute_batch("DROP TRIGGER reject_suffix")
        .unwrap();
    assert!(fused_parts(&connection, &source).is_ok());
}
