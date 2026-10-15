use super::*;
use crate::runtime::read_layout::fragments::tests::{Field, fields, original_bytes};
use serde_json::{Value, json};

fn message(dynamic: &Value) -> Value {
    json!({
        "before": {"escaped/~水\n": [u64::MAX,-0.0,1.000_125], "@literal":{"$key":"original"}},
        "blocks": [{"text":"original","raw":"{\"n\":1.000,\"u\":18446744073709551615}"}],
        "provenance": {"original":"ancestor-conversation","digest":"exact"},
        "provenance_source": dynamic,
        "tail": {"after_hole":[true,null,{"value":"水🙂\\\""}]}
    })
}
fn certify(source: &SeekableObservationSource, item: u64) -> CertifiedMessageShape<'_> {
    source
        .message_shape_reader()
        .unwrap()
        .certify_message_shape(
            item,
            &original_bytes(source, &["messages".into(), item.to_string()]),
        )
        .unwrap()
        .unwrap()
}
fn static_fields(
    source: &SeekableObservationSource,
    shape: &CertifiedMessageShape<'_>,
) -> Vec<Field> {
    let mut result = Vec::new();
    source
        .message_shape_reader()
        .unwrap()
        .visit_message_shape_fields(
            shape,
            |_| FieldPreviewPolicy::OriginalSpanIfSmall,
            |path, encoded, parent, start, end, children, key, preview| {
                result.push(evidence(
                    path,
                    encoded,
                    parent,
                    (start, end, children, key),
                    preview,
                ));
                Ok(())
            },
        )
        .unwrap();
    result
}
fn evidence(
    path: &[String],
    encoded: &str,
    parent: Option<(&str, u64)>,
    coordinates: (u64, u64, u64, Option<(u64, u64)>),
    preview: FieldPreview<'_>,
) -> Field {
    let (start, end, children, key) = coordinates;
    let (preview, span_nodes) = match preview {
        FieldPreview::Inline(value) => (Some(value.clone()), None),
        FieldPreview::OriginalSpan { recursive_nodes } => (None, Some(recursive_nodes)),
        FieldPreview::Skipped => (None, None),
    };
    Field {
        path: path.to_vec(),
        encoded: encoded.into(),
        parent: parent.map(|(path, ordinal)| (path.into(), ordinal)),
        start,
        end,
        children,
        key,
        preview,
        span_nodes,
    }
}
fn rebase(mut field: Field, shape: &CertifiedMessageShape<'_>) -> Field {
    let prefix = vec!["messages".into(), shape.item.to_string()];
    field.path.splice(0..0, prefix.iter().cloned());
    field.encoded = serde_json::to_string(&field.path).unwrap();
    if let Some((parent, _)) = &mut field.parent {
        let mut relative: Vec<String> = serde_json::from_str(parent).unwrap();
        relative.splice(0..0, prefix);
        *parent = serde_json::to_string(&relative).unwrap();
    }
    field.start += shape.root.start;
    field.end += shape.root.start;
    field.key = field
        .key
        .map(|(start, end)| (start + shape.root.start, end + shape.root.start));
    field
}

#[test]
fn t08_t14_t19_t20_certified_message_shape_rebases_static_fields_and_keeps_fresh_hole_facts() {
    let first = SeekableObservationSource::new(
        &json!({"messages":[message(&json!({"domain":"trace.1","item":"0"}))]}),
    )
    .unwrap();
    let second =
        SeekableObservationSource::new(&json!({"before":"different source offset", "messages":[
        message(&json!({"domain":"trace.123456789","item":"0","path":["provenance"]})),
        message(&json!({"domain":"trace.123456789","item":"1","path":["provenance"]}))]}))
        .unwrap();
    let old = certify(&first, 0);
    let shapes = vec![certify(&second, 0), certify(&second, 1)];
    assert_eq!(old.static_digest, shapes[0].static_digest);
    assert_eq!(shapes[0].static_digest, shapes[1].static_digest);
    assert_ne!(
        old.hole.end - old.hole.start,
        shapes[0].hole.end - shapes[0].hole.start
    );
    assert_ne!(old.root.start, shapes[0].root.start);
    let old_fields = static_fields(&first, &old);
    let mut combined = Vec::new();
    second
        .message_shape_reader()
        .unwrap()
        .visit_fields_with_message_shapes(
            &shapes,
            |_| FieldPreviewPolicy::OriginalSpanIfSmall,
            |path, encoded, parent, start, end, children, key, preview| {
                combined.push(evidence(
                    path,
                    encoded,
                    parent,
                    (start, end, children, key),
                    preview,
                ));
                Ok(())
            },
        )
        .unwrap();
    assert!(
        combined
            .iter()
            .any(|field| field.path == ["messages", "1", "provenance_source", "item"])
    );
    assert!(
        !combined
            .iter()
            .any(|field| field.path == ["messages", "1", "tail"])
    );
    for shape in &shapes {
        let current = static_fields(&second, shape);
        assert_eq!(old_fields.len(), current.len());
        let delta = i128::from(shape.hole.end - shape.hole.start)
            - i128::from(old.hole.end - old.hole.start);
        for (old_field, field) in old_fields.iter().zip(&current) {
            assert_eq!(old_field.path, field.path);
            assert_eq!(old_field.parent, field.parent);
            assert_eq!(old_field.preview, field.preview);
            assert_eq!(old_field.span_nodes, field.span_nodes);
            let after = old_field.start >= old.hole.end - old.root.start;
            assert_eq!(
                i128::from(field.start) - i128::from(old_field.start),
                if after { delta } else { 0 }
            );
            let rebased = rebase(field.clone(), shape);
            assert_eq!(
                original_bytes(&second, &rebased.path),
                original_bytes(
                    &first,
                    &[vec!["messages".into(), "0".into()], old_field.path.clone()].concat()
                )
            );
            combined.push(rebased);
        }
    }
    combined.sort_by_key(|field| field.start);
    assert_eq!(
        combined,
        fields(&second, FieldPreviewPolicy::OriginalSpanIfSmall)
    );
}

#[test]
fn t08_t19_t20_message_shape_misses_and_foreign_certificates_preserve_original_field_policy() {
    let source = SeekableObservationSource::new(
        &json!({"messages":[message(&json!({"domain":"original"})),message(&Value::Null)]}),
    )
    .unwrap();
    let before = fields(&source, FieldPreviewPolicy::OriginalSpanIfSmall);
    assert!(
        source
            .message_shape_reader()
            .unwrap()
            .certify_message_shape(0, b"{}")
            .unwrap()
            .is_none()
    );
    let huge = SeekableObservationSource::new(
        &json!({"messages":[{"provenance_source":null,"wide":vec![0;600]}]}),
    )
    .unwrap();
    assert!(
        huge.message_shape_reader()
            .unwrap()
            .certify_message_shape(0, &original_bytes(&huge, &["messages".into(), "0".into()]))
            .unwrap()
            .is_none(),
        "actual >512 nodes cannot use saturated preview count"
    );
    let missing =
        SeekableObservationSource::new(&json!({"messages":[{"body":"no dynamic hole"}]})).unwrap();
    assert!(
        missing
            .message_shape_reader()
            .unwrap()
            .certify_message_shape(0, b"{\"body\":\"no dynamic hole\"}")
            .unwrap()
            .is_none()
    );
    let foreign =
        SeekableObservationSource::new(&json!({"messages":[message(&Value::Null)]})).unwrap();
    let shapes = vec![certify(&source, 0), certify(&source, 1)];
    assert!(
        foreign
            .message_shape_reader()
            .unwrap()
            .visit_fields_with_message_shapes(
                &shapes,
                |_| FieldPreviewPolicy::Skip,
                |_, _, _, _, _, _, _, _| panic!("foreign source must fail before callbacks")
            )
            .is_err()
    );
    let reversed = vec![certify(&source, 1), certify(&source, 0)];
    assert!(
        source
            .message_shape_reader()
            .unwrap()
            .visit_fields_with_message_shapes(
                &reversed,
                |_| FieldPreviewPolicy::Skip,
                |_, _, _, _, _, _, _, _| panic!("unordered shapes must fail before callbacks")
            )
            .is_err()
    );
    assert_eq!(
        fields(&source, FieldPreviewPolicy::OriginalSpanIfSmall),
        before
    );
}

struct ImportedMessages<'a>(
    &'a [std::sync::Arc<super::super::TrustedFragment>],
    &'a super::super::FragmentCapture,
);
impl serde::Serialize for ImportedMessages<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq as _;
        let mut values = serializer.serialize_seq(Some(self.0.len()))?;
        for fragment in self.0 {
            values.serialize_element(&fragment.emitted(self.1))?;
        }
        values.end()
    }
}
struct ImportedRequest<'a>(ImportedMessages<'a>);
impl serde::Serialize for ImportedRequest<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let mut object = serializer.serialize_map(Some(1))?;
        object.serialize_entry("messages", &self.0)?;
        object.end()
    }
}
impl schemars::JsonSchema for ImportedRequest<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ImportedRequestFixture".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <Value as schemars::JsonSchema>::json_schema(generator)
    }
}

#[test]
fn t08_t14_t19_t20_message_shape_skip_preserves_imported_fragment_geometry_without_static_path_work()
 {
    let original=SeekableObservationSource::new(&json!({"messages":[message(&json!({"source":"original","offset":"3"})),message(&json!({"source":"original","offset":"4"}))]})).unwrap();
    let fragments = (0..2)
        .map(|item| {
            original
                .capture_fragment(&["messages".into(), item.to_string()], 65_536, 512)
                .unwrap()
                .unwrap()
        })
        .collect::<Vec<_>>();
    let capture = super::super::FragmentCapture::new(2);
    let reused = SeekableObservationSource::new_with_fragments(
        &ImportedRequest(ImportedMessages(&fragments, &capture)),
        &capture,
    )
    .unwrap();
    assert_eq!(original_bytes(&original, &[]), original_bytes(&reused, &[]));
    let shapes = vec![certify(&reused, 0), certify(&reused, 1)];
    super::super::PATH_WORK.with(|work| work.set((0, 0)));
    let expected = fields(&reused, FieldPreviewPolicy::OriginalSpanIfSmall);
    let all_components = super::super::PATH_WORK.with(std::cell::Cell::get).0;
    super::super::PATH_WORK.with(|work| work.set((0, 0)));
    let mut actual = Vec::new();
    reused
        .message_shape_reader()
        .unwrap()
        .visit_fields_with_message_shapes(
            &shapes,
            |_| FieldPreviewPolicy::OriginalSpanIfSmall,
            |path, encoded, parent, start, end, children, key, preview| {
                actual.push(evidence(
                    path,
                    encoded,
                    parent,
                    (start, end, children, key),
                    preview,
                ));
                Ok(())
            },
        )
        .unwrap();
    let fresh_components = super::super::PATH_WORK.with(std::cell::Cell::get).0;
    for shape in &shapes {
        actual.extend(
            static_fields(&reused, shape)
                .into_iter()
                .map(|field| rebase(field, shape)),
        );
    }
    actual.sort_by_key(|field| field.start);
    assert_eq!(actual, expected);
    assert_eq!(
        fresh_components
            + shapes
                .iter()
                .map(|shape| static_fields(&reused, shape).len())
                .sum::<usize>(),
        all_components
    );
    assert!(fresh_components < all_components / 2);
}

fn bounded_path_source(extra: usize) -> SeekableObservationSource {
    let mut relative = vec!["deep".to_owned()];
    relative.extend(vec!["x".repeat(220); 15]);
    let mut absolute = vec!["messages".to_owned(), "0".into()];
    absolute.extend(relative.iter().cloned());
    let last = (0..256)
        .find(|bytes| {
            let mut path = absolute.clone();
            path.push("y".repeat(*bytes));
            serde_json::to_vec(&path).unwrap().len() == 3600
        })
        .unwrap()
        + extra;
    relative.push("y".repeat(last));
    let mut value = Value::Bool(true);
    for key in relative.into_iter().rev() {
        value = json!({key:value});
    }
    value
        .as_object_mut()
        .unwrap()
        .insert("provenance_source".into(), Value::Null);
    SeekableObservationSource::new(&json!({"messages":[value]})).unwrap()
}
#[test]
fn t08_t19_t20_message_shape_static_paths_keep_the_original_absolute_3600_byte_limit() {
    let valid = bounded_path_source(0);
    let shape = certify(&valid, 0);
    let actual = static_fields(&valid, &shape);
    assert!(
        actual
            .iter()
            .map(|field| rebase(field.clone(), &shape).encoded.len())
            .any(|bytes| bytes == 3600)
    );
    let invalid = bounded_path_source(1);
    let shape = certify(&invalid, 0);
    let mut expected_error = None;
    let result = invalid.visit_fields_selecting_previews(
        |_| FieldPreviewPolicy::OriginalSpanIfSmall,
        |_, _, _, _, _, _, _, _| Ok(()),
    );
    if let Err(error) = result {
        expected_error = Some(error);
    }
    assert_eq!(expected_error, Some("native layout path exceeds bound"));
    assert_eq!(
        invalid
            .message_shape_reader()
            .unwrap()
            .visit_message_shape_fields(
                &shape,
                |_| FieldPreviewPolicy::OriginalSpanIfSmall,
                |_, _, _, _, _, _, _, _| Ok(())
            )
            .unwrap_err(),
        "native layout path exceeds bound"
    );
}

#[test]
fn t08_t19_t20_message_shape_batch_reuses_one_native_reader_for_certification_and_all_field_visits()
{
    let source=SeekableObservationSource::new(&json!({"messages":(0..100).map(|item|message(&json!({"domain":"trace.original","item":item.to_string()}))).collect::<Vec<_>>()})).unwrap();
    let headers = (0..100)
        .map(|item| original_bytes(&source, &["messages".into(), item.to_string()]))
        .collect::<Vec<_>>();
    super::super::INDEX_OPENS.with(|work| work.set(0));
    super::super::FILE_WORK.with(|work| work.set((0, 0)));
    let reader = source.message_shape_reader().unwrap();
    let shapes = headers
        .iter()
        .enumerate()
        .map(|(item, header)| {
            reader
                .certify_message_shape(item as u64, header)
                .unwrap()
                .unwrap()
        })
        .collect::<Vec<_>>();
    let mut static_count = 0;
    for shape in &shapes {
        reader
            .visit_message_shape_fields(
                shape,
                |_| FieldPreviewPolicy::OriginalSpanIfSmall,
                |_, _, _, _, _, _, _, _| {
                    static_count += 1;
                    Ok(())
                },
            )
            .unwrap();
    }
    let mut fresh_count = 0;
    reader
        .visit_fields_with_message_shapes(
            &shapes,
            |_| FieldPreviewPolicy::OriginalSpanIfSmall,
            |_, _, _, _, _, _, _, _| {
                fresh_count += 1;
                Ok(())
            },
        )
        .unwrap();
    assert!(static_count > 1000 && fresh_count > 100);
    assert_eq!(
        super::super::INDEX_OPENS.with(std::cell::Cell::get),
        1,
        "all items share one original native SQLite reader"
    );
    assert_eq!(
        super::super::FILE_WORK.with(std::cell::Cell::get).1,
        1,
        "certification, previews and static/fresh traversals share one original byte file"
    );
}

struct ImportedArrayRequest<'a>(
    &'a std::sync::Arc<super::super::TrustedFragment>,
    &'a super::super::FragmentCapture,
);
impl serde::Serialize for ImportedArrayRequest<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let mut fields = serializer.serialize_map(Some(1))?;
        fields.serialize_entry("messages", &self.0.emitted(self.1))?;
        fields.end()
    }
}
impl schemars::JsonSchema for ImportedArrayRequest<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ImportedArrayRequestFixture".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <Value as schemars::JsonSchema>::json_schema(generator)
    }
}
#[test]
fn t08_t14_t19_message_shape_scoped_facts_include_virtual_roots_inside_an_imported_ancestor() {
    let original=SeekableObservationSource::new(&json!({"messages":[message(&json!({"original":"first"})),message(&json!({"original":"second"}))]})).unwrap();
    let fragment = original
        .capture_fragment(&["messages".into()], 65_536, 512)
        .unwrap()
        .unwrap();
    let capture = super::super::FragmentCapture::new(1);
    let reused = SeekableObservationSource::new_with_fragments(
        &ImportedArrayRequest(&fragment, &capture),
        &capture,
    )
    .unwrap();
    let reader = reused.message_shape_reader().unwrap();
    let original_fields = fields(&original, FieldPreviewPolicy::OriginalSpanIfSmall);
    for item in 0..2 {
        let expected = certify(&original, item);
        let actual = reader
            .certify_message_shape(
                item,
                &original_bytes(&original, &["messages".into(), item.to_string()]),
            )
            .unwrap()
            .unwrap();
        assert_eq!(actual.static_digest, expected.static_digest);
        let prefix = ["messages".to_owned(), item.to_string()];
        let original_count = original_fields
            .iter()
            .filter(|field| {
                field.path.starts_with(&prefix)
                    && field.path.len() > 2
                    && field.path[2] != "provenance_source"
            })
            .count();
        assert_eq!(
            original_count, 20,
            "exact fixture nodes from the full uncached traversal"
        );
        let actual_fields = static_fields(&reused, &actual);
        assert_eq!(actual_fields.len(), original_count);
        assert_eq!(actual_fields, static_fields(&original, &expected));
    }
}

include!("packed_tests.rs");
