use super::*;

#[derive(serde::Serialize, schemars::JsonSchema)]
struct CaptureFixture {
    request_id: String,
    admitted_time: String,
    provenance_source: serde_json::Value,
    tools: Vec<serde_json::Value>,
    messages: Vec<serde_json::Value>,
}
fn fixture(tools: usize, width: usize) -> CaptureFixture {
    let properties = (0..width)
        .map(|index| {
            let key = match index % 3 {
                0 => format!("@reserved-{index}"),
                1 => format!("escaped-\"\\{index}"),
                _ => format!("水🙂-{index}"),
            };
            (
                key,
                serde_json::json!({
                    "type": "string",
                    "description": "exact immutable schema description 水🙂 \"\\".repeat(2),
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let required = properties.keys().cloned().collect::<Vec<_>>();
    CaptureFixture {
        request_id: "request-0000000001".into(),
        admitted_time: "2026-10-19 00:00:00".into(),
        provenance_source: serde_json::json!({"owner":"owner-A","revision":7}),
        tools: (0..tools)
            .map(|index| {
                serde_json::json!({
                    "tool_id": format!("tool-{index}"),
                    "name": format!("native-tool-{index}"),
                    "description": {"text":"original allowlisted definition", "truncated":false},
                    "input_schema": {"value": {
                        "type":"object", "properties":properties, "required":required,
                    }, "truncated":false},
                })
            })
            .collect(),
        messages: Vec::new(),
    }
}
fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(
        "CREATE TABLE request_source_pages(hash BLOB PRIMARY KEY,bytes BLOB NOT NULL) WITHOUT ROWID",
    ).unwrap();
    connection
}
fn pages(connection: &Connection) -> Vec<(Vec<u8>, Vec<u8>)> {
    connection
        .prepare("SELECT hash,bytes FROM request_source_pages ORDER BY hash")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn legacy_scalar(
    connection: &Connection,
    source: &SeekableObservationSource,
) -> (Fragment, u64, u64) {
    let (start, end, _) = geometry(source, &["messages".into()]).unwrap();
    let length = source.read(&[], 0, 1).unwrap().readable_end;
    let reader = source.byte_reader().unwrap();
    (
        persist_part(connection, source, &reader, 0, length).unwrap(),
        start,
        end,
    )
}
fn node_count(value: &serde_json::Value) -> usize {
    1 + match value {
        serde_json::Value::Array(values) => values.iter().map(node_count).sum::<usize>(),
        serde_json::Value::Object(values) => values.values().map(node_count).sum::<usize>(),
        _ => 0,
    }
}
#[test]
fn scalar_geometry_reuses_immutable_index_with_exact_fields_bytes_and_pages() {
    for (tools, width, fields) in [(1, 0, 20), (7, 64, 1884)] {
        let value = fixture(tools, width);
        assert_eq!(node_count(&serde_json::to_value(&value).unwrap()), fields);
        let source = SeekableObservationSource::new(&value).unwrap();
        let old = connection();
        let new = connection();
        crate::runtime::read_layout::take_field_preview_work();
        let expected = legacy_scalar(&old, &source);
        let old_work = crate::runtime::read_layout::take_field_preview_work();
        let actual = scalar(&new, &source).unwrap();
        let new_work = crate::runtime::read_layout::take_field_preview_work();
        assert_eq!(actual.1, expected.1);
        assert_eq!(actual.2, expected.2);
        assert_eq!(
            serde_json::to_vec(&actual.0).unwrap(),
            serde_json::to_vec(&expected.0).unwrap()
        );
        assert_eq!(
            pages(&new),
            pages(&old),
            "every persisted byte/fact page and hash must be identical"
        );
        assert_eq!(old_work, (fields, fields));
        assert_eq!(
            new_work,
            (fields, 0),
            "capture every original fact while avoiding the extra full geometry walk"
        );
        eprintln!(
            "scalar geometry fields={fields}, old visits={}, new visits={}, avoided={fields}",
            old_work.0 + old_work.1,
            new_work.0 + new_work.1
        );
    }
}
#[test]
fn scalar_geometry_keeps_changed_definition_identity_time_and_old_source_separate() {
    let before = fixture(1, 3);
    let mut after = fixture(1, 64);
    after.request_id = "request-0000000010-fresh".into();
    after.admitted_time = "2026-10-19 00:00:01".into();
    after.provenance_source["owner"] = "owner-B".into();
    assert_eq!(before.tools[0]["tool_id"], after.tools[0]["tool_id"]);
    let original = SeekableObservationSource::new(&before).unwrap();
    let current = SeekableObservationSource::new(&after).unwrap();
    let path = ["messages".into()];
    let original_bounds = original.original_root_field_geometry("messages").unwrap();
    let current_bounds = current.original_root_field_geometry("messages").unwrap();
    assert_ne!(original_bounds, current_bounds);
    for (source, value, bounds) in [
        (&original, &before, original_bounds),
        (&current, &after, current_bounds),
    ] {
        assert_eq!(bounds, geometry(source, &path).unwrap());
        let mut raw = Vec::new();
        source.copy_original_bytes_to(&mut raw).unwrap();
        assert_eq!(raw, serde_json::to_vec(value).unwrap());
        assert_eq!(
            &raw[usize::try_from(bounds.0).unwrap()..usize::try_from(bounds.1).unwrap()],
            b"[]"
        );
        let connection = connection();
        let captured = scalar(&connection, source).unwrap();
        assert_eq!((captured.1, captured.2), (bounds.0, bounds.1));
    }
    assert_eq!(
        original.original_root_field_geometry("messages").unwrap(),
        original_bounds
    );
}

#[test]
fn scalar_geometry_measures_repeated_standard_public_tool_capture() {
    // Public Tools projection from the archived local fake-provider623 cohort.
    // It contains no provider/private request configuration or authorization key.
    let tools: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("standard_public_tools_fixture.json")).unwrap();
    assert_eq!(tools.len(), 7);
    assert_eq!(node_count(&serde_json::Value::Array(tools.clone())), 305);
    let mut value = fixture(0, 0);
    value.tools = tools;
    let source = SeekableObservationSource::new(&value).unwrap();
    let old = connection();
    let new = connection();
    crate::runtime::read_layout::take_field_preview_work();
    let expected = legacy_scalar(&old, &source);
    let old_work = crate::runtime::read_layout::take_field_preview_work();
    let actual = scalar(&new, &source).unwrap();
    let new_work = crate::runtime::read_layout::take_field_preview_work();
    assert_eq!(old_work, (312, 312));
    assert_eq!(new_work, (312, 0));
    assert_eq!((actual.1, actual.2), (expected.1, expected.2));
    assert_eq!(
        serde_json::to_vec(&actual.0).unwrap(),
        serde_json::to_vec(&expected.0).unwrap()
    );
    assert_eq!(pages(&new), pages(&old));
    // Quantify the remaining repeated Tools fact capture before selecting any
    // cross-request cache or its memory budget. This does not reuse those facts.
    let measured = connection();
    let length = source.read(&[], 0, 1).unwrap().readable_end;
    let reader = source.byte_reader().unwrap();
    let mut spool = FactSpool::new(&measured);
    capture_facts(&mut spool, &source, &reader, 0, length).unwrap();
    let mut statement = measured
        .prepare("SELECT key,value FROM request_source_fact_spool")
        .unwrap();
    let mut rows = statement.query([]).unwrap();
    let (mut count, mut encoded_bytes) = (0, 0);
    while let Some(row) = rows.next().unwrap() {
        let key: String = row.get(0).unwrap();
        let bytes: Vec<u8> = row.get(1).unwrap();
        if key.starts_with("F[\"tools\"")
            || key.starts_with("C[\"tools\"")
            || bytes.as_slice() == b"[\"tools\"]"
        {
            count += 1;
            encoded_bytes += key.len() + bytes.len();
        }
    }
    assert!(count >= 610);
    assert!(encoded_bytes > 65_536);
    eprintln!(
        "standard public catalog:312 geometry visits avoided per scalar; remaining Tools facts={count}, encoded key+value bytes={encoded_bytes}; exact field/byte pages unchanged"
    );
}
