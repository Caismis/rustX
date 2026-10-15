use super::*;
use crate::runtime::read_layout::{FragmentCapture, TrustedFragment};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;

type Geometry = (u64, u64, u64, u64, FieldGeometry, FieldGeometry);
fn geometry(cert: &CertifiedMessageShape<'_>) -> Geometry {
    (
        cert.first,
        cert.last,
        cert.hole_first,
        cert.hole_last,
        cert.root,
        cert.hole,
    )
}
fn work() -> (usize, usize, usize, usize) {
    (
        super::super::NODE_SELECTIONS.with(|work| work.replace(0)),
        super::super::NODE_LOOKUPS.with(|work| work.replace(0)),
        super::super::CHILD_LOOKUPS.with(|work| work.replace(0)),
        super::super::UNIQUE_KEY_LOOKUPS.with(|work| work.replace(0)),
    )
}
// Independent original descent: never reads the saturated or exact node count.
fn old_last(reader: &MessageShapeReader<'_>, mut id: u64) -> u64 {
    for _ in 0..=256 {
        let layout = &reader.source.layout;
        let node = layout.indexed_node(&reader.reader.connection, id).unwrap();
        if node.children == 0 {
            return id;
        }
        let children = layout
            .indexed_children(&reader.reader.connection, id, node.children - 1, 1)
            .unwrap();
        assert_eq!(children[0].1, node.children - 1);
        id = children[0].0;
    }
    panic!("original fixture must satisfy native depth bound")
}
fn old_geometry(reader: &MessageShapeReader<'_>) -> Option<Geometry> {
    let layout = &reader.source.layout;
    let connection = &reader.reader.connection;
    let first = layout
        .indexed_select(connection, reader.messages, Some(0), "")
        .unwrap();
    let root = layout.indexed_node(connection, first).unwrap();
    let last = old_last(reader, first);
    let mut hole = None;
    for (id, _) in layout.indexed_children(connection, first, 0, 32).unwrap() {
        let node = layout.indexed_node(connection, id).unwrap();
        if node.key_complete && node.key.as_deref() == Some("provenance_source") {
            if hole.is_some() {
                return None;
            }
            hole = Some((id, node));
        }
    }
    let (hole_id, hole) = hole?;
    Some((
        first,
        last,
        hole_id,
        old_last(reader, hole_id),
        (&root).into(),
        (&hole).into(),
    ))
}
fn header(source: &SeekableObservationSource) -> Vec<u8> {
    let chunk = source
        .read(&["messages".into(), "0".into()], 0, 16_384)
        .unwrap();
    assert!(chunk.next_offset.is_none());
    chunk.bytes
}

#[test]
fn t08_t14_t19_t20_small_message_certification_uses_exact_counts_and_one_unique_hole_query() {
    let source=SeekableObservationSource::new(&json!({"messages":[{
        "before":[u64::MAX,-0.0,1.000_125],"provenance_source":{"domain":"original","item":"0"},"tail":[true,null]
    }]})).unwrap();
    let bytes = header(&source);
    let reader = source.message_shape_reader().unwrap();
    work();
    let expected = old_geometry(&reader).unwrap();
    let old_work = work();
    let actual = reader.certify_message_shape(0, &bytes).unwrap().unwrap();
    let actual_work = work();
    assert_eq!(geometry(&actual), expected);
    assert!((1..=32).contains(&actual.root.recursive_nodes));
    assert_eq!(actual.last - actual.first + 1, actual.root.recursive_nodes);
    assert_eq!(
        actual.hole_last - actual.hole_first + 1,
        actual.hole.recursive_nodes
    );
    assert_eq!(
        actual_work,
        (1, 1, 0, 1),
        "one unchanged ordinal query, one root row, one bounded unique-hole query"
    );
    assert!(old_work.1 > actual_work.1 && old_work.2 > actual_work.2);
    eprintln!(
        "native shape SQL (ordinal,node,children,unique): old={old_work:?} new={actual_work:?}"
    );
}

#[test]
fn t08_t14_t19_t20_saturated_message_count_retains_original_bounded_descent() {
    let source = SeekableObservationSource::new(&json!({"messages":[{
        "provenance_source":{"domain":"original"},"wide":(0..80).collect::<Vec<_>>()
    }]}))
    .unwrap();
    let bytes = header(&source);
    let reader = source.message_shape_reader().unwrap();
    let expected = old_geometry(&reader).unwrap();
    work();
    let actual = reader.certify_message_shape(0, &bytes).unwrap().unwrap();
    let actual_work = work();
    assert_eq!(geometry(&actual), expected);
    assert_eq!(actual.root.recursive_nodes, 33);
    assert!(actual.last - actual.first + 1 > 32);
    assert!(
        actual_work.2 > 0,
        "saturated count must use actual last-child descent"
    );
    assert_eq!(actual_work.3, 1);
}

struct RawRequest(Box<serde_json::value::RawValue>);
impl Serialize for RawRequest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl schemars::JsonSchema for RawRequest {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RawRequestFixture".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <Value as schemars::JsonSchema>::json_schema(generator)
    }
}
struct ImportedArray<'a>(&'a Arc<TrustedFragment>, &'a FragmentCapture);
impl Serialize for ImportedArray<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let mut fields = serializer.serialize_map(Some(1))?;
        fields.serialize_entry("messages", &self.0.emitted(self.1))?;
        fields.end()
    }
}
impl schemars::JsonSchema for ImportedArray<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ImportedArrayFixture".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <Value as schemars::JsonSchema>::json_schema(generator)
    }
}

#[test]
fn t08_t14_t19_t20_unique_hole_query_rejects_duplicate_decoded_keys_in_native_and_imported_roots() {
    for raw in [
        r#"{"messages":[{"before":true,"provenance_source":{"x":1},"provenance_\u0073ource":{"x":2},"tail":false}]}"#,
        r#"{"messages":[{"before":true,"tail":false}]}"#,
    ] {
        let source = SeekableObservationSource::new(&RawRequest(
            serde_json::value::RawValue::from_string(raw.into()).unwrap(),
        ))
        .unwrap();
        let bytes = header(&source);
        let reader = source.message_shape_reader().unwrap();
        assert!(old_geometry(&reader).is_none());
        work();
        assert!(reader.certify_message_shape(0, &bytes).unwrap().is_none());
        assert_eq!(
            work().3,
            1,
            "native absence/duplicates use one LIMIT2 query"
        );
        let fragment = source
            .capture_fragment(&["messages".into()], 65_536, 512)
            .unwrap()
            .unwrap();
        let capture = FragmentCapture::new(1);
        let imported = SeekableObservationSource::new_with_fragments(
            &ImportedArray(&fragment, &capture),
            &capture,
        )
        .unwrap();
        let reader = imported.message_shape_reader().unwrap();
        assert!(old_geometry(&reader).is_none());
        work();
        assert!(reader.certify_message_shape(0, &bytes).unwrap().is_none());
        assert_eq!(
            work(),
            (0, 0, 0, 0),
            "imported unique-child checks borrow certified keys without SQL or descent"
        );
    }
}

#[test]
fn t08_t14_t19_t20_imported_small_message_certification_preserves_geometry_without_any_sql_queries()
{
    let source = SeekableObservationSource::new(&json!({"messages":[{
        "before":[u64::MAX,-0.0],"provenance_source":{"domain":"original"},"tail":[true]
    }]}))
    .unwrap();
    let bytes = header(&source);
    let expected = old_geometry(&source.message_shape_reader().unwrap()).unwrap();
    let fragment = source
        .capture_fragment(&["messages".into()], 65_536, 512)
        .unwrap()
        .unwrap();
    let capture = FragmentCapture::new(1);
    let imported = SeekableObservationSource::new_with_fragments(
        &ImportedArray(&fragment, &capture),
        &capture,
    )
    .unwrap();
    let reader = imported.message_shape_reader().unwrap();
    work();
    let actual = reader.certify_message_shape(0, &bytes).unwrap().unwrap();
    assert_eq!(geometry(&actual), expected);
    assert_eq!(work(), (0, 0, 0, 0));
    assert_eq!(header(&imported), bytes);
}
