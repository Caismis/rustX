use super::tests::property_encoding;
use super::*;

#[derive(Serialize, schemars::JsonSchema)]
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
    let schema = root::<Recursive>();
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
    let schema = root::<Recursive>();
    let first = encoding_plan(&schema, &schema);
    let tagged = Recursive::Leaf {
        quantity: 0,
        opaque: Value::Null,
    };
    TAG_VALUE_WORK.with(|work| work.set(0));
    serde_json::to_vec(&Encoded {
        value: &tagged,
        schema: &schema,
        root: &schema,
    })
    .unwrap();
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
        let next = encoding_plan(&schema, &schema);
        assert!(std::sync::Arc::ptr_eq(&first, &next));
        lookup_work();
        let encoded = Encoded {
            value: &source,
            schema: &schema,
            root: &schema,
        };
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
