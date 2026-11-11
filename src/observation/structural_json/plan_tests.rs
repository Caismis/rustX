use super::tests::property_encoding;
use super::*;

#[test]
fn compiled_wire_tag_names_cover_nested_branches_without_direct_properties() {
    let schema = serde_json::json!({
        "properties": {"root_only": {"const": "root"}},
        "oneOf": [
            {"properties": {"kind": {"const": "one"}, "ordinary": {"type": "string"}}},
            {"anyOf": [
                {"properties": {"type": {"const": null}}},
                {"allOf": [{"properties": {"state": {"const": false}}}]}
            ]}
        ]
    });
    let plan = plan::Plan::new(&schema, &schema);
    let root = plan.root();
    for key in ["kind", "type", "state"] {
        assert!(root.has_wire_tag(key), "{key}");
    }
    for key in ["root_only", "ordinary", "absent"] {
        assert!(!root.has_wire_tag(key), "{key}");
    }
    let selected = |view: Option<plan::View<'_>>| match view {
        Some(plan::View::Compiled(_, index)) => Some(index),
        _ => None,
    };
    for key in ["kind", "type", "state", "absent"] {
        for value in ["one", "other", "null", "false"] {
            assert_eq!(
                selected(root.wire_variant_str(key, value)),
                selected(root.wire_variant(key, &serde_json::json!(value))),
                "{key}: {value}"
            );
        }
    }
}

#[derive(Debug, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Recursive {
    Branch {
        sequence: u64,
        children: Vec<Recursive>,
    },
    Leaf {
        quantity: i64,
        opaque: Value,
    },
}

#[test]
fn compiled_schema_plan_matches_recursive_interpreter_bytes_and_admission() {
    let schema = schema_root::<Recursive>();
    for quantity in [0, -9_007_199_254_740_991, i64::MIN] {
        let value = Recursive::Branch {
            sequence: u64::MAX,
            children: vec![Recursive::Branch {
                sequence: 9_007_199_254_740_993,
                children: vec![Recursive::Leaf {
                    quantity,
                    opaque: serde_json::json!({"wide":u64::MAX,"negative":-0.0,"text":"水\n\"\\"}),
                }],
            }],
        };
        let expected = property_encoding(&value, &schema, true);
        assert_eq!(property_encoding(&value, &schema, false), expected);
        assert_eq!(expected.0.is_err(), quantity == i64::MIN);
    }
}

#[test]
fn compiled_schema_plan_preserves_null_reference_selection_and_branch_order() {
    #[derive(Serialize)]
    struct Row {
        sequence: u64,
    }
    let value = Row { sequence: u64::MAX };
    for first in [serde_json::json!({"$ref":"#/$defs/opaque"}), Value::Null] {
        let schema = serde_json::json!({
            "$defs":{"opaque":null},
            "allOf":[{"properties":{"sequence":first}},
                     {"properties":{"sequence":{"format":"uint64"}}}]
        });
        let expected = property_encoding(&value, &schema, true);
        assert_eq!(property_encoding(&value, &schema, false), expected);
        let encoded: Value = serde_json::from_slice(expected.0.as_ref().unwrap()).unwrap();
        assert_eq!(encoded["sequence"].is_string(), first.is_null());
    }
}

#[test]
fn compiled_schema_plan_reuses_type_plan_for_exact_length_and_output() {
    let first = codec::<Recursive>();
    let tagged = Recursive::Leaf {
        quantity: 0,
        opaque: Value::Null,
    };
    TAG_VALUE_WORK.with(|work| work.set(0));
    serde_json::to_vec(&encoded(&tagged)).unwrap();
    assert_eq!(
        TAG_VALUE_WORK.with(std::cell::Cell::get),
        1,
        "one built-in discriminant materialization, independent of variant position"
    );
    for sequence in [0, 9, 10, u64::MAX] {
        let source = Recursive::Branch {
            sequence,
            children: Vec::new(),
        };
        let next = codec::<Recursive>();
        assert!(std::sync::Arc::ptr_eq(&first, &next));
        lookup_work();
        let encoded = encoded(&source);
        let bytes = serde_json::to_vec(&encoded).unwrap();
        assert_eq!(lookup_work(), (0, 0));
        // The exact byte threshold is independent of the conservative admission
        // charge. Compare every threshold around the actual serializer output.
        for limit in bytes.len().saturating_sub(1)..=bytes.len() + 1 {
            let mut output = Capped {
                bytes: Vec::new(),
                limit,
            };
            let result = serde_json::to_writer(&mut output, &encoded);
            assert_eq!(result.is_ok(), limit >= bytes.len());
            if result.is_ok() {
                assert_eq!(output.bytes, bytes);
            }
        }
        assert_eq!(lookup_work(), (0, 0));
    }
}

fn cached_owner<T>() -> std::sync::Arc<SchemaOwner> {
    SCHEMAS
        .lock()
        .unwrap()
        .roots
        .entry(std::any::type_name::<T>())
        .or_insert_with(|| std::sync::Arc::new(SchemaOwner::default()))
        .clone()
}

#[test]
fn structural_decode_and_encoding_share_one_schema_free_plan() {
    #[derive(Debug, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema)]
    struct DecodeOnly {
        sequence: u64,
        opaque: Value,
    }
    let schema = schema_root::<DecodeOnly>();
    let owner = cached_owner::<DecodeOnly>();
    assert!(owner.plan.get().is_none());
    assert_eq!(
        owner.plan_builds.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    let source = serde_json::json!({
        "sequence":u64::MAX.to_string(),
        "opaque":{"wide":u64::MAX,"negative":-0.0}
    });
    for _ in 0..3 {
        let decoded: DecodeOnly = decode(source.clone()).unwrap();
        assert_eq!(decoded.sequence, u64::MAX);
        assert_eq!(decoded.opaque, source["opaque"]);
    }
    assert!(owner.plan.get().is_some());
    assert_eq!(
        owner.plan_builds.load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    let decoded: DecodeOnly = decode(source.clone()).unwrap();
    let expected = property_encoding(&decoded, &schema, true).0.unwrap();
    let mut bytes = Vec::new();
    to_writer(&mut bytes, &decoded).unwrap();
    assert_eq!(bytes, expected);
    assert_eq!(
        owner.plan_builds.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), source);
}

#[test]
fn concurrent_first_encoders_compile_one_shared_type_plan() {
    #[derive(Serialize, schemars::JsonSchema)]
    struct ConcurrentFirstEncode {
        sequence: u64,
        opaque: Value,
    }
    let schema = schema_root::<ConcurrentFirstEncode>();
    let owner = cached_owner::<ConcurrentFirstEncode>();
    assert!(owner.plan.get().is_none());
    let value = ConcurrentFirstEncode {
        sequence: u64::MAX,
        opaque: serde_json::json!({"wide":u64::MAX,"negative":-0.0}),
    };
    let expected = property_encoding(&value, &schema, true).0.unwrap();
    let barrier = std::sync::Barrier::new(8);
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    let mut bytes = Vec::new();
                    to_writer(&mut bytes, &value).unwrap();
                    (codec::<ConcurrentFirstEncode>(), bytes)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        owner.plan_builds.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    let first = &results[0].0;
    for (plan, bytes) in &results {
        assert!(std::sync::Arc::ptr_eq(first, plan));
        assert_eq!(bytes, &expected);
    }
}

struct Capped {
    bytes: Vec<u8>,
    limit: usize,
}
impl std::io::Write for Capped {
    fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len() + value.len() > self.limit {
            return Err(std::io::Error::other("exact limit"));
        }
        self.bytes.extend_from_slice(value);
        Ok(value.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn compact_plans_share_identical_subgraphs_and_outlive_schema_json() {
    #[derive(Serialize)]
    struct Pair {
        left: u64,
        right: u64,
    }
    let schema = serde_json::json!({"properties": {
        "left": {"format":"uint64"}, "right": {"format":"uint64"}
    }});
    let plan = plan::Plan::new(&schema, &schema);
    drop(schema);
    let View::Compiled(_, left) = plan.root().property_named("left").unwrap() else {
        unreachable!()
    };
    let View::Compiled(_, right) = plan.root().property_named("right").unwrap() else {
        unreachable!()
    };
    assert_eq!(left, right, "identical vocabulary is compiled once");
    let bytes = serde_json::to_vec(&ValueAt {
        value: &Pair {
            left: u64::MAX,
            right: u64::MAX,
        },
        schema: plan.root(),
        wire: false,
    })
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        serde_json::json!({"left":u64::MAX.to_string(),"right":u64::MAX.to_string()})
    );
}

// The pre-M2 decoder stays independent of the compact plan for this oracle.
fn interpreted_decode<T: serde::de::DeserializeOwned>(
    mut value: Value,
    root: &Value,
) -> serde_json::Result<T> {
    fn visit(value: &mut Value, schema: &Value, root: &Value) {
        let mut schema = resolve(schema, root);
        for keyword in ["oneOf", "anyOf"] {
            if let Some(branches) = schema[keyword].as_array() {
                for branch in branches {
                    let candidate = resolve(branch, root);
                    if let Some(properties) = candidate["properties"].as_object() {
                        let tagged: Vec<_> = properties
                            .iter()
                            .filter_map(|(key, property)| {
                                property.get("const").map(|expected| (key, expected))
                            })
                            .collect();
                        if !tagged.is_empty()
                            && tagged
                                .iter()
                                .all(|(key, expected)| value.get(*key) == Some(*expected))
                        {
                            schema = candidate;
                            break;
                        }
                    }
                }
            }
        }
        if schema["format"] == "uint64"
            && schema["maximum"]
                .as_f64()
                .is_none_or(|maximum| maximum > 9_007_199_254_740_991.0)
        {
            if let Some(number) = value.as_str().and_then(|value| {
                value
                    .parse::<u64>()
                    .ok()
                    .filter(|number| number.to_string() == value)
            }) {
                *value = Value::from(number);
            }
            return;
        }
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    visit(value, child(schema, root, Some(key)), root);
                }
            }
            Value::Array(items) => {
                for (index, value) in items.iter_mut().enumerate() {
                    visit(value, element(schema, root, index), root);
                }
            }
            _ => {}
        }
    }
    visit(&mut value, root, root);
    serde_json::from_value(value)
}

#[test]
fn structural_decoder_matches_old_interpreter_for_recursive_tags_and_numeric_errors() {
    let root = schema_root::<Recursive>();
    for sequence in [
        Value::from("0"),
        Value::from(u64::MAX.to_string()),
        Value::from(u64::MAX),
        Value::from("01"),
        Value::from("+1"),
        Value::from("18446744073709551616"),
        Value::Null,
        serde_json::json!(-0.0),
    ] {
        for quantity in [
            Value::from(17),
            Value::from(i64::MIN),
            Value::from("01"),
            Value::Null,
        ] {
            let source = serde_json::json!({
                "kind":"branch", "sequence":sequence,
                "children":[
                    {"kind":"leaf","quantity":quantity,"opaque":{"wide":u64::MAX,"zero":-0.0}},
                    {"kind":"branch","sequence":sequence,"children":[]}
                ]
            });
            let bytes = |result: serde_json::Result<Recursive>| {
                result
                    .map(|value| serde_json::to_vec(&value).unwrap())
                    .map_err(|error| error.to_string())
            };
            assert_eq!(
                bytes(decode(source.clone())),
                bytes(interpreted_decode(source, &root))
            );
        }
    }
}
