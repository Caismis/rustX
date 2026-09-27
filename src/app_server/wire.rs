//! App Server-only numeric codec. Native/local-stdio serde representations stay
//! unchanged. The same Rust-derived schemas drive the codec and code generation:
//! unbounded uint64 domains are exact text; explicitly bounded quantities and
//! platform-sized collection indices are safe JSON integers.
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SAFE: u64 = 9_007_199_254_740_991;

fn exact(schema: &Value) -> bool {
    schema["format"] == "uint64"
        && schema
            .get("maximum")
            .and_then(Value::as_f64)
            .is_none_or(|max| max > 9_007_199_254_740_991.0)
}

/// Canonical unsigned decimal syntax, including the u64 upper bound.
fn pattern() -> String {
    let max = u64::MAX.to_string();
    let mut alternatives = vec!["0".to_owned(), "[1-9][0-9]{0,18}".to_owned(), max.clone()];
    for (index, digit) in max.bytes().enumerate() {
        let low = if index == 0 { b'1' } else { b'0' };
        if digit > low {
            alternatives.push(format!(
                "{}[{}-{}][0-9]{{{}}}",
                &max[..index],
                char::from(low),
                char::from(digit - 1),
                max.len() - index - 1
            ));
        }
    }
    format!("^({})$(?![\\s\\S])", alternatives.join("|"))
}

/// Re-encodes schema `default` annotations into the domains they annotate.
///
/// Schemars copies a Rust `Default` verbatim, so an exact `u64` domain is
/// annotated with the JSON number `0` while its own type is canonical decimal
/// text. That default is then invalid against the very schema it documents, and
/// a client generator that infers a type from it produces an impossible one.
///
/// Encoding defaults through the same codec real values use keeps exactly one
/// rule for what an exact domain looks like. A default that does not match its
/// schema is left untouched rather than corrupted: `default` is an annotation,
/// and silently rewriting one we cannot interpret would be worse than leaving
/// the inconsistency visible.
fn encode_defaults(node: &mut Value, root: &Value) {
    match node {
        Value::Object(object) => {
            if let Some(mut default) = object.remove("default") {
                let subschema = Value::Object(object.clone());
                let _ = convert(&mut default, &subschema, root, true);
                object.insert("default".to_owned(), default);
            }
            for value in object.values_mut() {
                encode_defaults(value, root);
            }
        }
        Value::Array(items) => {
            for item in items {
                encode_defaults(item, root);
            }
        }
        _ => {}
    }
}

pub(super) fn schema(mut schema: Value) -> Value {
    fn transform(node: &mut Value, pattern: &str) {
        let is_exact = exact(node);
        let Some(object) = node.as_object_mut() else {
            if let Some(items) = node.as_array_mut() {
                for item in items {
                    transform(item, pattern);
                }
            }
            return;
        };
        if is_exact {
            let nullable = object.get("type").is_some_and(Value::is_array);
            object.insert(
                "type".into(),
                if nullable {
                    serde_json::json!(["string", "null"])
                } else {
                    Value::from("string")
                },
            );
            object.insert("pattern".into(), Value::from(pattern));
            object.remove("format");
            object.remove("minimum");
            object.remove("maximum");
        } else if let Some(format) = object.get("format").and_then(Value::as_str) {
            let bounds = match format {
                "uint" | "uint64" => Some((0_i64, SAFE)),
                "int64" | "int" => Some((-9_007_199_254_740_991, SAFE)),
                "uint32" => Some((0, u64::from(u32::MAX))),
                "uint16" => Some((0, u64::from(u16::MAX))),
                "uint8" => Some((0, u64::from(u8::MAX))),
                "int32" => Some((i64::from(i32::MIN), i32::MAX.cast_unsigned().into())),
                "int16" => Some((i64::from(i16::MIN), i16::MAX.cast_unsigned().into())),
                "int8" => Some((i64::from(i8::MIN), i8::MAX.cast_unsigned().into())),
                _ => None,
            };
            if let Some((min, max)) = bounds {
                object.entry("minimum").or_insert(Value::from(min));
                object.entry("maximum").or_insert(Value::from(max));
            }
        }
        for value in object.values_mut() {
            transform(value, pattern);
        }
    }
    // Defaults are encoded against the untransformed schema, where exact
    // domains are still identifiable by their `uint64` format.
    let root = schema.clone();
    encode_defaults(&mut schema, &root);
    transform(&mut schema, &pattern());
    schema
}

fn convert(value: &mut Value, schema: &Value, root: &Value, encode: bool) -> Result<(), String> {
    if value.is_null() {
        return Ok(());
    }
    if let Some(reference) = schema["$ref"].as_str() {
        let target = root
            .pointer(
                reference
                    .strip_prefix('#')
                    .ok_or("non-local wire schema reference")?,
            )
            .ok_or("missing wire schema reference")?;
        return convert(value, target, root, encode);
    }
    // Tagged alternatives are selected by their Rust-derived discriminants,
    // never by a second list of protocol methods or semantic field names.
    if let Some(properties) = schema["properties"].as_object()
        && properties.iter().any(|(key, property)| {
            property
                .get("const")
                .is_some_and(|expected| value.get(key) != Some(expected))
        })
    {
        return Ok(());
    }
    if exact(schema) {
        if encode {
            let number = value.as_u64().ok_or("exact domain requires native u64")?;
            *value = Value::from(number.to_string());
        } else {
            let text = value.as_str().ok_or("exact domain requires decimal text")?;
            let number = text
                .parse::<u64>()
                .map_err(|_| "invalid unsigned decimal domain")?;
            if number.to_string() != text {
                return Err("noncanonical unsigned decimal domain".into());
            }
            *value = Value::from(number);
        }
        return Ok(());
    }
    if value.is_number()
        && matches!(
            schema["format"].as_str(),
            Some("uint" | "uint64" | "int" | "int64")
        )
    {
        let number = value
            .as_i64()
            .ok_or("quantity must be a safe JSON integer")?;
        if number.unsigned_abs() > SAFE {
            return Err("quantity exceeds safe JSON integer range".into());
        }
    }
    if let (Some(object), Some(properties)) =
        (value.as_object_mut(), schema["properties"].as_object())
    {
        for (name, property) in properties {
            if let Some(field) = object.get_mut(name) {
                convert(field, property, root, encode)?;
            }
        }
    }
    if let Some(object) = value.as_object_mut()
        && schema["additionalProperties"].is_object()
    {
        for (name, field) in object {
            if schema["properties"].get(name).is_none() {
                convert(field, &schema["additionalProperties"], root, encode)?;
            }
        }
    }
    if let Some(items) = value.as_array_mut()
        && schema["items"].is_object()
    {
        for item in items {
            convert(item, &schema["items"], root, encode)?;
        }
    }
    for keyword in ["oneOf", "anyOf", "allOf"] {
        if let Some(branches) = schema[keyword].as_array() {
            for branch in branches {
                convert(value, branch, root, encode)?;
            }
        }
    }
    Ok(())
}

// Reject duplicate JSON fields before the numeric codec materializes an object.
// Otherwise a duplicate could disappear before the native DTO parser sees it.
struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = StrictValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON with unique object fields")
            }
            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| StrictValue(n.into()))
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(StrictValue(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(StrictValue(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, StrictValue(value))) =
                    map.next_entry::<String, StrictValue>()?
                {
                    if values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate JSON field"));
                    }
                }
                Ok(StrictValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

macro_rules! envelope {
    ($ty:ty) => {
        impl Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                struct Native<'a>(&'a $ty);
                impl Serialize for Native<'_> {
                    fn serialize<S: serde::Serializer>(
                        &self,
                        serializer: S,
                    ) -> Result<S::Ok, S::Error> {
                        <$ty>::serialize(self.0, serializer)
                    }
                }
                static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
                let schema = SCHEMA.get_or_init(|| {
                    serde_json::to_value(schemars::schema_for!($ty)).expect("Rust schema")
                });
                let mut value =
                    serde_json::to_value(Native(self)).map_err(serde::ser::Error::custom)?;
                convert(&mut value, schema, schema, true).map_err(serde::ser::Error::custom)?;
                value.serialize(serializer)
            }
        }
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
                let schema = SCHEMA.get_or_init(|| {
                    serde_json::to_value(schemars::schema_for!($ty)).expect("Rust schema")
                });
                let StrictValue(mut value) = StrictValue::deserialize(deserializer)?;
                convert(&mut value, schema, schema, false).map_err(serde::de::Error::custom)?;
                <$ty>::deserialize(value).map_err(serde::de::Error::custom)
            }
        }
    };
}
envelope!(super::protocol::Request);
envelope!(super::protocol::Response);
envelope!(super::protocol::Notification);

/// The complete public JSON Schema, with the same numeric rules as the codec.
pub(super) fn protocol_schema() -> Value {
    schema(
        serde_json::to_value(schemars::schema_for!(super::protocol::ProtocolMessage))
            .expect("Rust schema"),
    )
}

#[cfg(test)]
mod tests {

    #[test]
    fn referenced_and_nested_defaults_use_the_public_exact_domain() {
        let public = super::protocol_schema();
        assert_eq!(
            public["$defs"]["RuntimeClientResourcesView"]["properties"]["revision"]["default"],
            "0"
        );
        assert_eq!(
            public["$defs"]["RuntimeClientSnapshot"]["properties"]["resources"]["default"]["revision"],
            "0"
        );
    }
    use super::*;

    #[test]
    fn every_exact_public_u64_leaf_is_lossless_and_every_quantity_is_safe() {
        fn audit(native: &Value, public: &Value, count: &mut usize) {
            if exact(native) {
                *count += 1;
                assert_eq!(
                    public["type"],
                    if native["type"].is_array() {
                        serde_json::json!(["string", "null"])
                    } else {
                        Value::from("string")
                    }
                );
                let validator = jsonschema::validator_for(public).unwrap();
                for number in [0, 9_007_199_254_740_993_u64, u64::MAX] {
                    let mut wire = serde_json::to_value(number).unwrap();
                    convert(&mut wire, native, native, true).unwrap();
                    assert_eq!(wire, number.to_string());
                    assert!(validator.is_valid(&wire));
                    convert(&mut wire, native, native, false).unwrap();
                    assert_eq!(serde_json::from_value::<u64>(wire).unwrap(), number);
                    assert!(!validator.is_valid(&Value::from(number)));
                }
                for invalid in [
                    "",
                    "00",
                    "01",
                    "+1",
                    "-1",
                    "1.0",
                    "1e3",
                    " 1",
                    "1\n",
                    "18446744073709551616",
                ] {
                    assert!(
                        !validator.is_valid(&Value::from(invalid)),
                        "accepted {invalid:?}"
                    );
                    assert!(convert(&mut Value::from(invalid), native, native, false).is_err());
                }
            } else if matches!(
                native["format"].as_str(),
                Some("uint" | "uint64" | "int" | "int64")
            ) {
                assert!(public["maximum"].as_f64().unwrap() <= 9_007_199_254_740_991.0);
                assert!(
                    convert(
                        &mut Value::from(9_007_199_254_740_993_u64),
                        native,
                        native,
                        true
                    )
                    .is_err()
                );
            }
            if let Some(object) = native.as_object() {
                for (key, value) in object {
                    audit(value, &public[key], count);
                }
            } else if let Some(array) = native.as_array() {
                for (index, value) in array.iter().enumerate() {
                    audit(value, &public[index], count);
                }
            }
        }
        let native = serde_json::to_value(schemars::schema_for!(
            super::super::protocol::ProtocolMessage
        ))
        .unwrap();
        let mut count = 0;
        audit(&native, &schema(native.clone()), &mut count);
        assert!(
            count >= 20,
            "the complete nested public surface was audited"
        );
        let types = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("protocol/app-server/v24.ts"),
        )
        .unwrap();
        for domain in [
            "RuntimeIncarnationId",
            "RuntimeClientCursor",
            "RuntimeClientTranscriptCursor",
            "SurfaceRevision",
            "RuntimeResourceRevision",
            "InboundSequence",
        ] {
            assert!(
                types.contains(&format!("export type {domain} = string;")),
                "{domain} must remain a distinct named string domain"
            );
        }
    }

    fn native_round_trip<T>(value: &T)
    where
        T: Serialize
            + serde::de::DeserializeOwned
            + schemars::JsonSchema
            + PartialEq
            + std::fmt::Debug,
    {
        let root = serde_json::to_value(schemars::schema_for!(T)).unwrap();
        let mut wire = serde_json::to_value(value).unwrap();
        convert(&mut wire, &root, &root, true).unwrap();
        assert!(
            jsonschema::validator_for(&schema(root.clone()))
                .unwrap()
                .is_valid(&wire)
        );
        assert!(wire.to_string().contains("\"9007199254740993\""));
        convert(&mut wire, &root, &root, false).unwrap();
        assert_eq!(&serde_json::from_value::<T>(wire).unwrap(), value);
    }

    #[test]
    fn nested_subagent_compaction_and_todo_domains_round_trip() {
        const EXACT: u64 = 9_007_199_254_740_993;
        native_round_trip(&crate::runtime::subagent::SubagentObservation {
            revision: EXACT,
            ..Default::default()
        });
        native_round_trip(
            &crate::runtime_client::snapshot::RuntimeClientCompactionView {
                generation: EXACT,
                summary_message_id: crate::runtime::identity::MessageId::new("summary"),
                surface_revision: crate::conversation::surface::SurfaceRevision::new(EXACT),
                tokens_before: crate::runtime::types::TokenMeasurement {
                    input_tokens: 100,
                    source: crate::runtime::types::TokenMeasurementSource::ProviderReported,
                },
                estimated_tokens_after: 50,
            },
        );
        native_round_trip(&crate::tools::todo::TodoSnapshot {
            next_id: EXACT + 1,
            tasks: vec![crate::tools::todo::TodoTask {
                id: EXACT,
                subject: "fixture".into(),
                description: None,
                active_form: None,
                status: crate::tools::todo::TodoStatus::Pending,
                blocked_by: vec![EXACT - 1],
                owner: None,
                metadata: None,
            }],
        });
    }
}
