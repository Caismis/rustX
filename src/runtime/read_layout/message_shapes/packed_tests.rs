#[test]
fn t08_t14_t19_packed_message_hole_facts_reconstruct_every_default_field_and_original_byte() {
    let source=SeekableObservationSource::new(&json!({"lead":"different offset", "messages":[
        message(&json!({"type":"value","domain":"trace.records.9.detail.request.messages","item":"0","path":["provenance"]})),
        message(&json!({"type":"value","domain":"trace.records.123456789.detail.request.messages","item":"1","path":["provenance"]}))
    ]})).unwrap();
    let shapes = (0..2)
        .map(|item| certify(&source, item))
        .collect::<Vec<_>>();
    let reader = source.message_shape_reader().unwrap();
    let mut actual = Vec::new();
    reader
        .visit_fields_with_packed_message_shapes(
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
    assert!(
        !actual
            .iter()
            .any(|field| field.path.len() > 2 && field.path[0] == "messages")
    );
    for shape in &shapes {
        actual.extend(
            static_fields(&source, shape)
                .into_iter()
                .map(|field| rebase(field, shape)),
        );
        reader
            .visit_message_hole_fields(
                shape,
                |path, encoded, parent, start, end, children, key, preview| {
                    let mut field =
                        evidence(path, encoded, parent, (start, end, children, key), preview);
                    let prefix = vec![
                        "messages".into(),
                        shape.item.to_string(),
                        "provenance_source".into(),
                    ];
                    field.path.splice(0..0, prefix.iter().cloned());
                    field.encoded = serde_json::to_string(&field.path).unwrap();
                    field.parent = if let Some((parent, ordinal)) = field.parent {
                        let mut path: Vec<String> = serde_json::from_str(&parent).unwrap();
                        path.splice(0..0, prefix);
                        Some((serde_json::to_string(&path).unwrap(), ordinal))
                    } else {
                        Some((
                            serde_json::to_string(&["messages", &shape.item.to_string()]).unwrap(),
                            shape.hole_ordinal,
                        ))
                    };
                    field.start += shape.hole.start;
                    field.end += shape.hole.start;
                    field.key = field
                        .key
                        .map(|(a, b)| (a + shape.hole.start, b + shape.hole.start));
                    if path.is_empty() {
                        let legacy = fields(&source, FieldPreviewPolicy::OriginalSpanIfSmall)
                            .into_iter()
                            .find(|old| old.path == field.path)
                            .unwrap();
                        // Scoped root has no parent-key span; preserve the independent original key facts.
                        field.key = legacy.key;
                    }
                    assert_eq!(
                        original_bytes(&source, &field.path).len() as u64,
                        field.end - field.start
                    );
                    actual.push(field);
                    Ok(())
                },
            )
            .unwrap();
    }
    actual.sort_by(|a, b| a.path.cmp(&b.path));
    let mut expected = fields(&source, FieldPreviewPolicy::OriginalSpanIfSmall);
    expected.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(actual, expected);
    let other = SeekableObservationSource::new(&json!({"messages":[]})).unwrap();
    assert!(
        other
            .message_shape_reader()
            .unwrap()
            .visit_message_hole_fields(&shapes[0], |_, _, _, _, _, _, _, _| Ok(()))
            .is_err()
    );
}
