//! Compare every reachable plan edge, including every method and nested variant,
//! with the old schema interpreter. Fixtures alone select only a few branches.
use super::*;
use crate::observation::structural_json::plan::{Plan, Unsigned};
use std::collections::HashSet;

fn resolve<'a>(schema: &'a Value, root: &'a Value) -> &'a Value {
    schema["$ref"].as_str().map_or(schema, |reference| {
        resolve(
            root.pointer(reference.strip_prefix('#').unwrap()).unwrap(),
            root,
        )
    })
}

fn fixture(schema: &Value, root: &Value, depth: usize) -> Value {
    if depth == 0 {
        return Value::Null;
    }
    let schema = resolve(schema, root);
    if let Some(value) = schema.get("const").or_else(|| schema["enum"].get(0)) {
        return value.clone();
    }
    let mut value = if let Some(properties) = schema["properties"].as_object() {
        properties
            .iter()
            .map(|(key, schema)| (key.clone(), fixture(schema, root, depth - 1)))
            .collect()
    } else if schema["items"].is_object() {
        serde_json::json!([fixture(&schema["items"], root, depth - 1)])
    } else {
        match schema["type"].as_str() {
            Some("string") => Value::from("fixture"),
            Some("boolean") => Value::Bool(true),
            Some("integer") => Value::from(17),
            Some("number") => serde_json::json!(-0.0),
            _ => Value::Null,
        }
    };
    for keyword in ["oneOf", "anyOf", "allOf"] {
        if let Some(branch) = schema[keyword].get(0) {
            let branch = fixture(branch, root, depth - 1);
            if let (Some(object), Some(fields)) = (value.as_object_mut(), branch.as_object()) {
                object.extend(fields.clone());
            } else {
                value = branch;
            }
        }
    }
    value
}

fn compare(source: &Value, schema: &Value, root: &Value, view: View<'_>) {
    for encode in [true, false] {
        let mut expected = source.clone();
        let mut actual = source.clone();
        assert_eq!(
            view.convert_wire(&mut actual, encode)
                .map_err(str::to_owned),
            convert(&mut expected, schema, root, encode),
            "source={source} schema={schema} encode={encode}"
        );
        // Value equality does not distinguish signed zero or field order.
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
    }
}

fn probes() -> Vec<Value> {
    vec![
        Value::Null,
        Value::Bool(true),
        Value::from(0),
        serde_json::json!(-0.0),
        Value::from(9_007_199_254_740_991_u64),
        Value::from(9_007_199_254_740_992_u64),
        Value::from(u64::MAX),
        Value::from(i64::MIN),
        serde_json::json!(1.5),
        Value::from("0"),
        Value::from(u64::MAX.to_string()),
        Value::from("01"),
        Value::from("+1"),
        Value::from("18446744073709551616"),
        Value::from("1e3"),
        serde_json::json!([u64::MAX, -0.0]),
        serde_json::json!({"unknown":u64::MAX,"requestParams":{"n":u64::MAX}}),
    ]
}

fn audit(schema: &Value, root: &Value, view: View<'_>, seen: &mut HashSet<(usize, u32)>) {
    let schema = resolve(schema, root);
    let View::Compiled(_, node) = view else {
        unreachable!()
    };
    if !seen.insert((std::ptr::from_ref(schema) as usize, node)) {
        return;
    }
    let expected_unsigned = match schema["format"].as_str() {
        Some("uint64")
            if schema["maximum"]
                .as_f64()
                .is_none_or(|n| n > 9_007_199_254_740_991.0) =>
        {
            Unsigned::Decimal
        }
        Some("uint" | "uint64") => Unsigned::Safe,
        _ => Unsigned::Plain,
    };
    assert!(view.unsigned() == expected_unsigned);
    assert_eq!(
        view.signed_safe(),
        matches!(schema["format"].as_str(), Some("int" | "int64"))
    );
    let source = fixture(schema, root, 12);
    compare(&source, schema, root, view);
    for source in probes() {
        compare(&source, schema, root, view);
    }
    let properties = schema["properties"].as_object();
    for (index, (key, property)) in properties.into_iter().flatten().enumerate() {
        let (actual_key, child) = view.property(index).unwrap();
        assert_eq!(actual_key, key);
        let View::Compiled(_, expected_node) = child else {
            unreachable!()
        };
        let View::Compiled(_, named_node) = view.property_named(key).unwrap() else {
            unreachable!()
        };
        assert_eq!(named_node, expected_node, "sorted lookup for {key}");
        assert_eq!(view.tag(key), property.get("const"));
        audit(property, root, child, seen);
        // Multiple invalid fields exercise the original error/check ordering
        // and the state of fields converted before an error.
        for replacement in probes() {
            let mut source = source.clone();
            source
                .as_object_mut()
                .unwrap()
                .insert(key.clone(), replacement);
            compare(&source, schema, root, view);
        }
    }
    assert!(
        view.property(properties.map_or(0, serde_json::Map::len))
            .is_none()
    );
    if let Some(additional) = schema.get("additionalProperties").filter(|v| v.is_object()) {
        audit(additional, root, view.additional().unwrap(), seen);
    } else {
        assert!(view.additional().is_none());
    }
    for (kind, keyword) in ["oneOf", "anyOf", "allOf"].iter().enumerate() {
        let branches = schema[*keyword].as_array();
        for (index, branch) in branches.into_iter().flatten().enumerate() {
            audit(branch, root, view.branch(kind, index).unwrap(), seen);
            let branch = fixture(branch, root, 12);
            // Apply each alternative through the parent, not just in isolation.
            let mut envelope = source.clone();
            if let (Some(object), Some(fields)) = (envelope.as_object_mut(), branch.as_object()) {
                object.extend(fields.clone());
            } else {
                envelope = branch;
            }
            compare(&envelope, schema, root, view);
        }
        assert!(view.branch(kind, branches.map_or(0, Vec::len)).is_none());
    }
    let prefix = schema["prefixItems"].as_array();
    for (index, item) in prefix.into_iter().flatten().enumerate() {
        audit(item, root, view.array_item(index).unwrap(), seen);
    }
    let index = prefix.map_or(0, Vec::len);
    if let Some(items) = schema.get("items").filter(|v| v.is_object()) {
        audit(items, root, view.array_item(index).unwrap(), seen);
    } else {
        assert!(view.array_item(index).is_none());
    }
}

fn audit_envelope<T: Envelope>() {
    let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    let mut seen = HashSet::new();
    audit(&schema, &schema, T::codec().root(), &mut seen);
    eprintln!(
        "{}: {} schema/plan node pairs",
        std::any::type_name::<T>(),
        seen.len()
    );
}

#[test]
fn every_closed_envelope_method_and_recursive_schema_edge_matches_the_oracle() {
    audit_envelope::<super::super::protocol::Request>();
    audit_envelope::<super::super::protocol::Response>();
    audit_envelope::<super::super::protocol::Notification>();
    audit_envelope::<super::super::protocol::ServerRequest>();
    audit_envelope::<super::super::product_host::FileRead>();
    // Export-only definitions also remain in the differential vocabulary.
    let schema = serde_json::to_value(schemars::schema_for!(
        super::super::protocol::ProtocolMessage
    ))
    .unwrap();
    let plan = Plan::new(&schema, &schema);
    audit(&schema, &schema, plan.root(), &mut HashSet::new());
}
