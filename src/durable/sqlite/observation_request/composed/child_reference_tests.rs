use super::{Connection, SeekableObservationSource, child, child_key, fact, field_key, index};
use crate::durable::ConversationStoreError;
use std::collections::BTreeMap;

#[derive(serde::Serialize, schemars::JsonSchema)]
struct Fixture {
    messages: Vec<u8>,
    native: u64,
    opaque: serde_json::Value,
}
fn fixture() -> Fixture {
    Fixture {
        messages: Vec::new(),
        native: u64::MAX,
        opaque: serde_json::json!({
            "escaped\"\\\n水🙂": [1.25, -0.0, {"unsafe":u64::MAX}],
            "raw": " {\"value\":1.00} ",
            "long": "λ".repeat(8_000),
            "large-key-水🙂".repeat(40): {"nested":[true,false,null]},
        }),
    }
}
fn entries(connection: &Connection, root: &index::Reference) -> Vec<(String, Vec<u8>)> {
    let raw = index::load_blob(connection, root.hash).unwrap();
    let page = index::inspect_page(&raw);
    match page["type"].as_str().unwrap() {
        "leaf" => page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry[0].as_str().unwrap().to_owned(),
                    serde_json::from_value(entry[1].clone()).unwrap(),
                )
            })
            .collect(),
        "branch" => page["children"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|child| {
                let child: index::Reference = serde_json::from_value(child.clone()).unwrap();
                assert!(child.depth < root.depth);
                entries(connection, &child)
            })
            .collect(),
        other => panic!("unexpected page kind {other}"),
    }
}

#[test]
fn t08_t15_t20_child_references_keep_one_exact_fact_with_the_original_read_bound() {
    let value = fixture();
    let source = SeekableObservationSource::new(&value).unwrap();
    let connection = Connection::open_in_memory().unwrap();
    super::super::create_schema(&connection).unwrap();
    super::super::install_workspace(&connection).unwrap();
    let fragment = super::whole(&connection, &source).unwrap();
    let rows: BTreeMap<_, _> = entries(&connection, &fragment.fields).into_iter().collect();
    let mut children = 0;
    source
        .visit_fields(|path, _, parent, start, end, count, key, preview| {
            let stored = fact(&connection, &fragment, path).unwrap().unwrap();
            assert_eq!(
                (stored.start, stored.end, stored.children),
                (start, end, count)
            );
            let expected = crate::runtime_client::checkpoint::bounded_bytes(preview, 16_384)
                .unwrap_or_else(|| b"null".to_vec());
            assert_eq!(
                super::super::read::fragment_field(&connection, &fragment, path)
                    .unwrap()
                    .3,
                expected
            );
            assert_eq!(
                rows[&field_key(path).unwrap()],
                super::super::record::encode(&stored, 40960).unwrap()
            );
            if let Some((parent, ordinal)) = parent {
                let parent: Vec<String> = serde_json::from_str(parent).unwrap();
                let pointer = &rows[&child_key(&parent, ordinal).unwrap()];
                assert_eq!(pointer, &serde_json::to_vec(path).unwrap());
                super::super::take_work();
                assert_eq!(
                    child(&connection, &fragment, &parent, ordinal)
                        .unwrap()
                        .unwrap(),
                    path
                );
                let work = super::super::take_work();
                assert_eq!(work.index_pages, usize::from(fragment.fields.depth) + 1);
                assert!(work.max_index_page <= 65_536);
                assert_eq!(work.header_decodes, 0);
                children += 1;
            }
            if let Some((start, end)) = key.filter(|_| path.last().unwrap().starts_with('@')) {
                assert_eq!(stored.key, Some(end - start));
                let mut key_path = path.to_vec();
                key_path.push("$key".into());
                let synthetic = fact(&connection, &fragment, &key_path).unwrap().unwrap();
                assert_eq!((synthetic.start, synthetic.end), (start, end));
                assert_eq!(
                    super::super::read::fragment_field(&connection, &fragment, &key_path)
                        .unwrap()
                        .3,
                    b"\"\""
                );
            }
            Ok(())
        })
        .unwrap();
    assert!(children > 10);
    let mut original = Vec::new();
    crate::runtime_client::structural_json::to_writer(&mut original, &value).unwrap();
    assert_eq!(
        super::bytes(&connection, &fragment, 0, fragment.length).unwrap(),
        original
    );
}

#[test]
fn t08_t12_t19_authenticated_child_paths_reject_malformed_legacy_and_reparented_metadata() {
    let connection = Connection::open_in_memory().unwrap();
    super::super::create_schema(&connection).unwrap();
    super::super::install_workspace(&connection).unwrap();
    let source = SeekableObservationSource::new(&fixture()).unwrap();
    let mut fragment = super::whole(&connection, &source).unwrap();
    let parent = vec!["parent".to_owned()];
    let mut bad = vec![
        br#"{"path":["parent","child"],"header":"bnVsbA=="}"#.to_vec(),
        br#" ["parent","child"] "#.to_vec(),
        br#"["other","child"]"#.to_vec(),
        br#"["parent"]"#.to_vec(),
        br#"["parent","child","grandchild"]"#.to_vec(),
        vec![b' '; 4097],
    ];
    bad.push(serde_json::to_vec(&["parent".to_owned(), "x".repeat(257)]).unwrap());
    for value in bad {
        let mut builder = index::Builder::new(&connection);
        builder.push(child_key(&parent, 3).unwrap(), value).unwrap();
        fragment.fields = builder.finish().unwrap().unwrap();
        super::super::take_work();
        assert!(matches!(
            child(&connection, &fragment, &parent, 3),
            Err(ConversationStoreError::Storage(_))
        ));
        let work = super::super::take_work();
        assert_eq!(work.index_pages, 1);
        assert!(work.max_index_page <= 65_536);
    }
    let mut builder = index::Builder::new(&connection);
    let expected = vec!["parent".to_owned(), "水🙂\"\\\n".to_owned()];
    builder
        .push(
            child_key(&parent, 3).unwrap(),
            serde_json::to_vec(&expected).unwrap(),
        )
        .unwrap();
    fragment.fields = builder.finish().unwrap().unwrap();
    assert_eq!(
        child(&connection, &fragment, &parent, 3).unwrap(),
        Some(expected)
    );
    connection
        .execute(
            "UPDATE request_source_pages SET bytes=x'7b7d' WHERE hash=?1",
            [fragment.fields.hash.as_slice()],
        )
        .unwrap();
    assert!(matches!(
        child(&connection, &fragment, &parent, 3),
        Err(ConversationStoreError::Storage(_))
    ));
}
