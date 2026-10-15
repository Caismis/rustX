use super::*;
use crate::runtime::read_layout::{FieldPreview, FieldPreviewPolicy};
use serde::ser::{SerializeSeq as _, SerializeStruct as _};

#[derive(Serialize, schemars::JsonSchema)]
struct Original {
    fresh: u64,
    items: Vec<Value>,
    tail: String,
}
struct Reused<'a> {
    fresh: u64,
    items: &'a [Arc<TrustedFragment>],
    capture: &'a FragmentCapture,
    tail: &'a str,
}
impl schemars::JsonSchema for Reused<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        <Original as schemars::JsonSchema>::schema_name()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <Original as schemars::JsonSchema>::json_schema(generator)
    }
}
struct Items<'a>(&'a Reused<'a>);
impl Serialize for Items<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut items = serializer.serialize_seq(Some(self.0.items.len()))?;
        for item in self.0.items {
            items.serialize_element(&item.emitted(self.0.capture))?;
        }
        items.end()
    }
}
impl Serialize for Reused<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct("Original", 3)?;
        fields.serialize_field("fresh", &self.fresh)?;
        fields.serialize_field("items", &Items(self))?;
        fields.serialize_field("tail", self.tail)?;
        fields.end()
    }
}

pub(in crate::runtime::read_layout) fn original_bytes(
    source: &SeekableObservationSource,
    path: &[String],
) -> Vec<u8> {
    let mut result = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = source.read(path, offset, 16_384).unwrap();
        result.extend_from_slice(&chunk.bytes);
        if let Some(next) = chunk.next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            assert_eq!(result.len() as u64, chunk.readable_end);
            return result;
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(in crate::runtime::read_layout) struct Field {
    pub(in crate::runtime::read_layout) path: Vec<String>,
    pub(in crate::runtime::read_layout) encoded: String,
    pub(in crate::runtime::read_layout) parent: Option<(String, u64)>,
    pub(in crate::runtime::read_layout) start: u64,
    pub(in crate::runtime::read_layout) end: u64,
    pub(in crate::runtime::read_layout) children: u64,
    pub(in crate::runtime::read_layout) key: Option<(u64, u64)>,
    pub(in crate::runtime::read_layout) preview: Option<Value>,
    pub(in crate::runtime::read_layout) span_nodes: Option<u64>,
}
pub(in crate::runtime::read_layout) fn fields(
    source: &SeekableObservationSource,
    policy: FieldPreviewPolicy,
) -> Vec<Field> {
    let mut result = Vec::new();
    source
        .visit_fields_selecting_previews(
            |_| policy,
            |path, encoded, parent, start, end, children, key, preview| {
                let (preview, span_nodes) = match preview {
                    FieldPreview::Inline(value) => (Some(value.clone()), None),
                    FieldPreview::OriginalSpan { recursive_nodes } => (None, Some(recursive_nodes)),
                    FieldPreview::Skipped => (None, None),
                };
                result.push(Field {
                    path: path.to_vec(),
                    encoded: encoded.into(),
                    parent: parent.map(|(parent, ordinal)| (parent.into(), ordinal)),
                    start,
                    end,
                    children,
                    key,
                    preview,
                    span_nodes,
                });
                Ok(())
            },
        )
        .unwrap();
    result
}
fn fragment_fixture() -> (Value, Arc<TrustedFragment>) {
    let value = serde_json::json!({
        "exact":u64::MAX,"negative_zero":-0.0,"fraction":1.000_125,
        "escaped":"水🙂\n\"\\","raw":"{\"n\":1.000,\"u\":18446744073709551615}",
        "array":[null,true,{"nested":[1,2,3]}],
        "@0":{"$key":"literal reserved names"},"$key":"original literal",
        "wide":(0..8).map(|value|serde_json::json!({"value":value})).collect::<Vec<_>>()
    });
    let source = SeekableObservationSource::new(&value).unwrap();
    let directory = source.layout.directory.path().to_owned();
    let fragment = source.capture_fragment(&[], 65_536, 512).unwrap().unwrap();
    assert!(fragment.owned_bytes() <= 65_536 && fragment.node_count() <= 512);
    assert_eq!(fragment.raw_bytes(), original_bytes(&source, &[]));
    drop(source);
    assert!(
        !directory.exists(),
        "fragments retain no previous layout/file owner"
    );
    (value, fragment)
}

#[test]
fn t08_t14_t19_t20_trusted_fragments_rebase_all_fields_and_ancestor_previews_without_parser_rows() {
    let (value, fragment) = fragment_fixture();
    let original = Original {
        fresh: u64::MAX,
        items: vec![value.clone(), value],
        tail: "fresh水\n\"".into(),
    };
    super::super::NODE_WRITES.with(|work| work.set((0, 0)));
    let expected = SeekableObservationSource::new(&original).unwrap();
    let baseline = super::super::NODE_WRITES.with(std::cell::Cell::get);
    let capture = FragmentCapture::new(32);
    let fragments = [Arc::clone(&fragment), Arc::clone(&fragment)];
    super::super::NODE_WRITES.with(|work| work.set((0, 0)));
    FRAGMENT_WORK.with(|work| work.set((0, 0)));
    let actual = SeekableObservationSource::new_with_fragments(
        &Reused {
            fresh: original.fresh,
            items: &fragments,
            capture: &capture,
            tail: &original.tail,
        },
        &capture,
    )
    .unwrap();
    let writes = super::super::NODE_WRITES.with(std::cell::Cell::get);
    let (imports, avoided) = FRAGMENT_WORK.with(std::cell::Cell::get);
    assert_eq!(imports, 2);
    assert_eq!(avoided, 2 * (fragment.node_count() - 1));
    assert_eq!(writes, (baseline.0 - avoided, baseline.1));
    assert!(capture.metadata_bytes() <= capture.metadata_limit());
    assert_eq!(original_bytes(&actual, &[]), original_bytes(&expected, &[]));
    for policy in [
        FieldPreviewPolicy::Inline,
        FieldPreviewPolicy::OriginalSpanIfSmall,
        FieldPreviewPolicy::Skip,
    ] {
        let expected_fields = fields(&expected, policy);
        assert_eq!(fields(&actual, policy), expected_fields);
        for field in &expected_fields {
            assert_eq!(
                original_bytes(&actual, &field.path),
                original_bytes(&expected, &field.path)
            );
            let page = actual.page(&field.path, 0, 32).unwrap();
            let expected = expected.page(&field.path, 0, 32).unwrap();
            assert_eq!(
                serde_json::to_value(
                    page.fields
                        .iter()
                        .map(|field| &field.value)
                        .collect::<Vec<_>>()
                )
                .unwrap(),
                serde_json::to_value(
                    expected
                        .fields
                        .iter()
                        .map(|field| &field.value)
                        .collect::<Vec<_>>()
                )
                .unwrap()
            );
            assert_eq!(
                (page.total, page.next_offset),
                (expected.total, expected.next_offset)
            );
        }
    }
}

#[test]
fn t08_t19_t20_trusted_fragment_cache_limits_fall_back_to_original_parsing_and_exact_bytes() {
    let (value, fragment) = fragment_fixture();
    let original = Original {
        fresh: 7,
        items: vec![value.clone(), value],
        tail: "tail".into(),
    };
    let expected = SeekableObservationSource::new(&original).unwrap();
    assert!(
        expected
            .capture_fragment(&["items".into(), "0".into()], 1, 512)
            .unwrap()
            .is_none()
    );
    assert!(
        expected
            .capture_fragment(
                &["items".into(), "0".into()],
                65_536,
                fragment.node_count() - 1
            )
            .unwrap()
            .is_none()
    );
    for maximum in [0, 1] {
        let capture = FragmentCapture::new(maximum);
        let fragments = [Arc::clone(&fragment), Arc::clone(&fragment)];
        FRAGMENT_WORK.with(|work| work.set((0, 0)));
        let actual = SeekableObservationSource::new_with_fragments(
            &Reused {
                fresh: original.fresh,
                items: &fragments,
                capture: &capture,
                tail: &original.tail,
            },
            &capture,
        )
        .unwrap();
        assert_eq!(FRAGMENT_WORK.with(std::cell::Cell::get).0, maximum);
        assert_eq!(original_bytes(&actual, &[]), original_bytes(&expected, &[]));
        assert_eq!(
            fields(&actual, FieldPreviewPolicy::Inline),
            fields(&expected, FieldPreviewPolicy::Inline)
        );
        assert!(capture.metadata_bytes() <= capture.metadata_limit());
    }
    assert!(FragmentCapture::new(usize::MAX).metadata_limit() <= MAX_FRAGMENT_METADATA_BYTES);
}

#[test]
fn t08_t14_t20_small_fragment_previews_are_not_retained_but_inline_views_remain_exact() {
    let value = Value::Object(
        (0..10)
            .map(|ordinal| {
                (
                    format!("original-{ordinal}"),
                    Value::String("static水".repeat(32)),
                )
            })
            .collect(),
    );
    let source = SeekableObservationSource::new(&value).unwrap();
    let mut eager_preview_bytes = 0;
    source
        .visit_fields(|_, _, _, _, _, _, _, preview| {
            eager_preview_bytes += value_owned(preview);
            Ok(())
        })
        .unwrap();
    let fragment = source.capture_fragment(&[], 65_536, 512).unwrap().unwrap();
    assert!(fragment.nodes.iter().all(|node| node.preview.is_none()));
    assert!(
        eager_preview_bytes > fragment.owned_bytes() / 2,
        "the old eager previews were a material duplicated allocation"
    );
    let eager_owned_bytes = fragment.owned_bytes() + eager_preview_bytes;
    assert!(fragment.owned_bytes() < eager_owned_bytes);
    eprintln!(
        "small-fragment retained={} eager_preview_heap={} eager_total={} nodes={}",
        fragment.owned_bytes(),
        eager_preview_bytes,
        eager_owned_bytes,
        fragment.node_count()
    );
    let expected = SeekableObservationSource::new(&Original {
        fresh: u64::MAX,
        items: vec![value],
        tail: "current source metadata".into(),
    })
    .unwrap();
    let capture = FragmentCapture::new(1);
    let fragments = [fragment];
    let actual = SeekableObservationSource::new_with_fragments(
        &Reused {
            fresh: u64::MAX,
            items: &fragments,
            capture: &capture,
            tail: "current source metadata",
        },
        &capture,
    )
    .unwrap();
    assert_eq!(original_bytes(&actual, &[]), original_bytes(&expected, &[]));
    for policy in [
        FieldPreviewPolicy::Inline,
        FieldPreviewPolicy::OriginalSpanIfSmall,
        FieldPreviewPolicy::Skip,
    ] {
        assert_eq!(fields(&actual, policy), fields(&expected, policy));
    }
}
