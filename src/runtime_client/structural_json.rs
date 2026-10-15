//! Source-owned JSON encoding for typed native display domains.
//! Exact u64 fields use decimal text; opaque JSON values keep their own semantics.
use serde::Serialize;
use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use serde_json::Value;

#[cfg(test)]
thread_local! {
    static LOOKUP_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}
#[cfg(test)]
fn lookup_work() -> (usize, usize) {
    LOOKUP_WORK.with(|work| work.replace((0, 0)))
}
fn resolve<'a>(schema: &'a Value, root: &'a Value) -> &'a Value {
    if schema.is_null() {
        return schema;
    }
    #[cfg(test)]
    LOOKUP_WORK.with(|work| {
        let (resolve, child) = work.get();
        work.set((resolve + 1, child));
    });
    if let Some(reference) = schema["$ref"]
        .as_str()
        .and_then(|reference| reference.strip_prefix('#'))
    {
        // Generated native schemas predominantly reference one unescaped
        // definition. Avoid parsing and unescaping that JSON pointer for every
        // field; nested and escaped pointers retain the general resolver.
        let definition = reference
            .strip_prefix("/$defs/")
            .filter(|key| !key.as_bytes().contains(&b'/') && !key.as_bytes().contains(&b'~'));
        let target = match (definition, root.get("$defs").and_then(Value::as_object)) {
            (Some(key), Some(definitions)) => definitions.get(key),
            _ => root.pointer(reference),
        };
        return target.map_or(schema, |schema| resolve(schema, root));
    }
    schema
}
fn child<'a>(schema: &'a Value, root: &'a Value, key: Option<&str>) -> &'a Value {
    if schema.is_null() {
        return schema;
    }
    #[cfg(test)]
    LOOKUP_WORK.with(|work| {
        let (resolve, child) = work.get();
        work.set((resolve, child + 1));
    });
    let schema = resolve(schema, root);
    let selected = match key {
        Some(key) => schema["properties"].get(key).or_else(|| {
            schema
                .get("additionalProperties")
                .filter(|schema| schema.is_object())
        }),
        None => schema.get("items"),
    };
    if let Some(selected) = selected {
        return selected;
    }
    branch_child(schema, root, key)
}
fn branch_child<'a>(schema: &'a Value, root: &'a Value, key: Option<&str>) -> &'a Value {
    if schema.is_null() {
        return schema;
    }
    for keyword in ["oneOf", "anyOf", "allOf"] {
        if let Some(branches) = schema[keyword].as_array() {
            for branch in branches {
                let selected = child(branch, root, key);
                if !selected.is_null() {
                    return selected;
                }
            }
        }
    }
    &Value::Null
}
fn element<'a>(schema: &'a Value, root: &'a Value, index: usize) -> &'a Value {
    let schema = resolve(schema, root);
    schema["prefixItems"]
        .get(index)
        .unwrap_or_else(|| child(schema, root, None))
}
#[cfg(test)]
fn selected_schema<'a>(root: &'a Value, path: &[String]) -> &'a Value {
    let mut schema = root;
    for key in path {
        let resolved = resolve(schema, root);
        schema = if resolved.get("items").is_some() || resolved.get("prefixItems").is_some() {
            key.parse::<usize>()
                .map_or(&Value::Null, |index| element(schema, root, index))
        } else {
            child(schema, root, Some(key))
        };
    }
    schema
}

pub(crate) struct Encoded<'a, T: ?Sized> {
    pub value: &'a T,
    pub schema: &'a Value,
    pub root: &'a Value,
}
impl<T: Serialize + ?Sized> Serialize for Encoded<'_, T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Keep the concrete Encoded callback type, including for a Null schema:
        // preflight distinguishes opaque JSON by the callback's concrete type.
        self.value.serialize(Encoder {
            serializer,
            schema: self.schema,
            root: self.root,
        })
    }
}
struct Encoder<'a, S> {
    serializer: S,
    schema: &'a Value,
    root: &'a Value,
}
macro_rules! scalar {
    ($($method:ident($ty:ty)),*) => {$ (fn $method(self, value: $ty) -> Result<Self::Ok, Self::Error> { self.serializer.$method(value) })*};
}
impl<'a, S: serde::Serializer> serde::Serializer for Encoder<'a, S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = Sequence<'a, S::SerializeSeq>;
    type SerializeTuple = Sequence<'a, S::SerializeTuple>;
    type SerializeTupleStruct = Sequence<'a, S::SerializeTupleStruct>;
    type SerializeTupleVariant = Sequence<'a, S::SerializeTupleVariant>;
    type SerializeMap = Mapping<'a, S::SerializeMap>;
    type SerializeStruct = Mapping<'a, S::SerializeStruct>;
    type SerializeStructVariant = Mapping<'a, S::SerializeStructVariant>;
    scalar!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8])
    );
    fn serialize_i64(self, value: i64) -> Result<S::Ok, S::Error> {
        if self.schema.is_null() {
            return self.serializer.serialize_i64(value);
        }
        if matches!(
            resolve(self.schema, self.root)["format"].as_str(),
            Some("int" | "int64")
        ) && value.unsigned_abs() > 9_007_199_254_740_991
        {
            return Err(serde::ser::Error::custom(
                "native quantity exceeds safe JSON integer range",
            ));
        }
        self.serializer.serialize_i64(value)
    }
    fn serialize_u64(self, value: u64) -> Result<S::Ok, S::Error> {
        if self.schema.is_null() {
            return self.serializer.serialize_u64(value);
        }
        let schema = resolve(self.schema, self.root);
        if schema["format"] == "uint64"
            && schema["maximum"]
                .as_f64()
                .is_none_or(|maximum| maximum > 9_007_199_254_740_991.0)
        {
            self.serializer.serialize_str(&value.to_string())
        } else {
            if matches!(schema["format"].as_str(), Some("uint" | "uint64"))
                && value > 9_007_199_254_740_991
            {
                return Err(serde::ser::Error::custom(
                    "native quantity exceeds safe JSON integer range",
                ));
            }
            self.serializer.serialize_u64(value)
        }
    }
    fn serialize_none(self) -> Result<S::Ok, S::Error> {
        self.serializer.serialize_none()
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<S::Ok, S::Error> {
        self.serializer.serialize_some(&Encoded {
            value,
            schema: self.schema,
            root: self.root,
        })
    }
    fn serialize_unit(self) -> Result<S::Ok, S::Error> {
        self.serializer.serialize_unit()
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<S::Ok, S::Error> {
        self.serializer.serialize_unit_struct(name)
    }
    fn serialize_unit_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
    ) -> Result<S::Ok, S::Error> {
        self.serializer.serialize_unit_variant(name, index, variant)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<S::Ok, S::Error> {
        self.serializer.serialize_newtype_struct(
            name,
            &Encoded {
                value,
                schema: self.schema,
                root: self.root,
            },
        )
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<S::Ok, S::Error> {
        self.serializer.serialize_newtype_variant(
            name,
            index,
            variant,
            &Encoded {
                value,
                schema: child(self.schema, self.root, Some(variant)),
                root: self.root,
            },
        )
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, S::Error> {
        Ok(Sequence::new(
            self.serializer.serialize_seq(len)?,
            self.schema,
            self.root,
        ))
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, S::Error> {
        Ok(Sequence::new(
            self.serializer.serialize_tuple(len)?,
            self.schema,
            self.root,
        ))
    }
    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, S::Error> {
        Ok(Sequence::new(
            self.serializer.serialize_tuple_struct(name, len)?,
            self.schema,
            self.root,
        ))
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, S::Error> {
        Ok(Sequence::new(
            self.serializer
                .serialize_tuple_variant(name, index, variant, len)?,
            self.schema,
            self.root,
        ))
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, S::Error> {
        Ok(Mapping::new(
            self.serializer.serialize_map(len)?,
            self.schema,
            self.root,
        ))
    }
    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, S::Error> {
        Ok(Mapping::new(
            self.serializer.serialize_struct(name, len)?,
            self.schema,
            self.root,
        ))
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, S::Error> {
        Ok(Mapping::new(
            self.serializer
                .serialize_struct_variant(name, index, variant, len)?,
            child(self.schema, self.root, Some(variant)),
            self.root,
        ))
    }
}
struct Sequence<'a, S> {
    inner: S,
    schema: &'a Value,
    items: Option<&'a Value>,
    prefix: Option<&'a [Value]>,
    root: &'a Value,
    index: usize,
}
impl<'a, S> Sequence<'a, S> {
    fn new(inner: S, schema: &'a Value, root: &'a Value) -> Self {
        let schema = resolve(schema, root);
        if schema.is_null() {
            return Self {
                inner,
                schema,
                items: Some(schema),
                prefix: None,
                root,
                index: 0,
            };
        }
        Self {
            inner,
            schema,
            items: schema.get("items"),
            prefix: schema
                .get("prefixItems")
                .and_then(Value::as_array)
                .map(Vec::as_slice),
            root,
            index: 0,
        }
    }
}
macro_rules! sequence {
    ($trait:ident,$method:ident) => {
        impl<S: $trait> $trait for Sequence<'_, S> {
            type Ok = S::Ok;
            type Error = S::Error;
            fn $method<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
                let schema = self
                    .prefix
                    .and_then(|items| items.get(self.index))
                    .or(self.items)
                    .unwrap_or_else(|| branch_child(self.schema, self.root, None));
                self.index += 1;
                self.inner.$method(&Encoded {
                    value,
                    schema,
                    root: self.root,
                })
            }
            fn end(self) -> Result<Self::Ok, Self::Error> {
                self.inner.end()
            }
        }
    };
}
sequence!(SerializeSeq, serialize_element);
sequence!(SerializeTuple, serialize_element);
sequence!(SerializeTupleStruct, serialize_field);
sequence!(SerializeTupleVariant, serialize_field);
struct Mapping<'a, S> {
    inner: S,
    schema: &'a Value,
    root: &'a Value,
    key: Option<String>,
    properties: Option<&'a serde_json::Map<String, Value>>,
    additional: Option<&'a Value>,
    variants: [Option<&'a [Value]>; 2],
}
impl<'a, S> Mapping<'a, S> {
    fn new(inner: S, schema: &'a Value, root: &'a Value) -> Self {
        let mut mapping = Self {
            inner,
            schema,
            root,
            key: None,
            properties: None,
            additional: None,
            variants: [None, None],
        };
        mapping.select_schema(schema);
        mapping
    }
    fn select_schema(&mut self, schema: &'a Value) {
        self.schema = resolve(schema, self.root);
        if self.schema.is_null() {
            self.properties = None;
            self.additional = None;
            self.variants = [None, None];
            return;
        }
        self.properties = self.schema.get("properties").and_then(Value::as_object);
        self.additional = self
            .schema
            .get("additionalProperties")
            .filter(|value| value.is_object());
        self.variants = [
            self.schema
                .get("oneOf")
                .and_then(Value::as_array)
                .map(Vec::as_slice),
            self.schema
                .get("anyOf")
                .and_then(Value::as_array)
                .map(Vec::as_slice),
        ];
    }
    fn field(&self, key: &str) -> &'a Value {
        if self.schema.is_null() {
            return self.schema;
        }
        self.properties
            .and_then(|fields| fields.get(key))
            .or(self.additional)
            .unwrap_or_else(|| branch_child(self.schema, self.root, Some(key)))
    }
}
impl<S: SerializeMap> SerializeMap for Mapping<'_, S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Self::Error> {
        self.key = super::checkpoint::bounded_bytes(key, 258)
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|key| match key {
                Value::String(key) => Some(key),
                Value::Number(key) => Some(key.to_string()),
                _ => None,
            });
        self.inner.serialize_key(key)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        let schema = self.field(self.key.as_deref().unwrap_or(""));
        self.inner.serialize_value(&Encoded {
            value,
            schema,
            root: self.root,
        })
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.inner.end()
    }
}
macro_rules! mapping {
    ($trait:ident) => {
        impl<S: $trait> $trait for Mapping<'_, S> {
            type Ok = S::Ok;
            type Error = S::Error;
            fn serialize_field<T: Serialize + ?Sized>(
                &mut self,
                key: &'static str,
                value: &T,
            ) -> Result<(), Self::Error> {
                for branches in self.variants.into_iter().flatten() {
                    for branch in branches {
                        if let Some(expected) =
                            resolve(branch, self.root)["properties"][key].get("const")
                        {
                            if serde_json::to_value(value).ok().as_ref() == Some(expected) {
                                self.select_schema(branch);
                                break;
                            }
                        }
                    }
                }
                let schema = self.field(key);
                self.inner.serialize_field(
                    key,
                    &Encoded {
                        value,
                        schema,
                        root: self.root,
                    },
                )
            }
            fn end(self) -> Result<Self::Ok, Self::Error> {
                self.inner.end()
            }
        }
    };
}
mapping!(SerializeStruct);
mapping!(SerializeStructVariant);
/// Immutable schemas depend only on their compile-time DTO type. Sharing them
/// avoids repeating the schema graph walk for every indexed Source field.
/// The vocabulary is finite monomorphized DTOs, never client-controlled keys.
pub(crate) fn root<T: schemars::JsonSchema>() -> std::sync::Arc<Value> {
    static SCHEMAS: std::sync::LazyLock<
        std::sync::Mutex<std::collections::BTreeMap<&'static str, std::sync::Arc<Value>>>,
    > = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::BTreeMap::new()));
    let identity = std::any::type_name::<T>();
    if let Some(schema) = SCHEMAS
        .lock()
        .expect("native schemas")
        .get(identity)
        .cloned()
    {
        return schema;
    }
    // Generate outside the cache boundary: a custom schema may request another
    // native type, and independent first encodes must not hold an owner lock.
    let schema = std::sync::Arc::new(
        serde_json::to_value(schemars::schema_for!(T)).expect("native structural schema"),
    );
    SCHEMAS
        .lock()
        .expect("native schemas")
        .entry(identity)
        .or_insert(schema)
        .clone()
}
pub(crate) fn to_writer<T: Serialize + schemars::JsonSchema, W: std::io::Write>(
    writer: W,
    value: &T,
) -> serde_json::Result<()> {
    let root = root::<T>();
    serde_json::to_writer(
        writer,
        &Encoded {
            value,
            schema: &root,
            root: &root,
        },
    )
}

pub(crate) fn decode<T: serde::de::DeserializeOwned + schemars::JsonSchema>(
    mut value: Value,
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
    let root = root::<T>();
    visit(&mut value, &root, &root);
    serde_json::from_value(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn t11_t20_null_schema_preserves_original_bytes_and_preflight_without_lookups() {
        #[derive(Serialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Opaque {
            Record {
                values: Vec<Value>,
                tuple: (u64, String),
            },
            Empty,
        }
        let unsafe_opaque = serde_json::json!({"nested": [{"exact": u64::MAX}]});
        assert_eq!(
            super::super::preflight::bounded_fit(&unsafe_opaque, 4096),
            Some(false)
        );
        assert_eq!(
            super::super::preflight::bounded_fit(
                &Encoded {
                    value: &unsafe_opaque,
                    schema: &Value::Null,
                    root: &Value::Null,
                },
                4096
            ),
            Some(true)
        );
        for source in [
            Opaque::Record {
                values: (0..64)
                    .map(|index| {
                        serde_json::json!({
                            "index": index, "wide": u64::MAX, "signed": i64::MIN,
                            "nested": [null, {"escaped": "water 水\n\"\\", "array": [1, 2, 3]}]
                        })
                    })
                    .collect(),
                tuple: (u64::MAX, "original".into()),
            },
            Opaque::Empty,
        ] {
            let expected = serde_json::to_vec(&source).unwrap();
            lookup_work();
            let actual = serde_json::to_vec(&Encoded {
                value: &source,
                schema: &Value::Null,
                root: &Value::Null,
            })
            .unwrap();
            assert_eq!(lookup_work(), (0, 0));
            assert_eq!(actual, expected);
            // Encoded callbacks intentionally retain the existing structural
            // preflight behavior, even with opaque values above JS-safe range.
            lookup_work();
            assert_eq!(
                super::super::preflight::bounded_fit(
                    &Encoded {
                        value: &source,
                        schema: &Value::Null,
                        root: &Value::Null,
                    },
                    128 * 1024
                ),
                Some(true)
            );
            assert_eq!(lookup_work(), (0, 0));
        }
    }

    #[test]
    fn t11_t20_container_schema_lookup_is_independent_of_field_count() {
        #[derive(Clone, Copy, Serialize)]
        struct Row<'a> {
            a: &'a str,
            b: &'a str,
            c: &'a str,
            d: &'a str,
            e: &'a str,
            f: &'a str,
            g: &'a str,
            h: &'a str,
        }
        let rows = vec![
            Row {
                a: "a",
                b: "b",
                c: "c",
                d: "d",
                e: "e",
                f: "f",
                g: "g",
                h: "water 水\n\"\\"
            };
            32
        ];
        let properties: serde_json::Map<String, Value> = ["a", "b", "c", "d", "e", "f", "g", "h"]
            .into_iter()
            .map(|key| (key.into(), serde_json::json!({"type": "string"})))
            .collect();
        let schema = serde_json::json!({
            "type": "array", "items": {"$ref": "#/$defs/row_alias"},
            "$defs": {"row_alias": {"$ref": "#/$defs/row~1fields"},
                "row/fields": {"type": "object", "properties": properties}}
        });
        let expected = serde_json::to_vec(&rows).unwrap();
        lookup_work();
        let actual = serde_json::to_vec(&Encoded {
            value: &rows,
            schema: &schema,
            root: &schema,
        })
        .unwrap();
        // One array resolution and three real reference-chain nodes per row;
        // the eight individual fields never re-resolve the parent or array.
        assert_eq!(lookup_work(), (1 + 3 * rows.len(), 0));
        assert_eq!(actual, expected);
    }

    #[test]
    fn t11_t20_tuple_prefix_does_not_visit_an_unused_item_fallback() {
        let schema = serde_json::json!({
            "prefixItems": [null, null],
            "allOf": [{"items": {"format": "uint64"}}]
        });
        let source = ("first", "second");
        lookup_work();
        let encoded = serde_json::to_vec(&Encoded {
            value: &source,
            schema: &schema,
            root: &schema,
        })
        .unwrap();
        assert_eq!(lookup_work(), (1, 0));
        assert_eq!(encoded, serde_json::to_vec(&source).unwrap());
        let source = ("first", "second", u64::MAX);
        let encoded = serde_json::to_value(Encoded {
            value: &source,
            schema: &schema,
            root: &schema,
        })
        .unwrap();
        assert_eq!(
            encoded,
            serde_json::json!(["first", "second", u64::MAX.to_string()])
        );
    }

    #[test]
    fn t11_t20_borrowed_container_schema_keeps_union_tuple_and_numeric_codecs() {
        #[derive(Serialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Tagged {
            Count { sequence: u64, opaque: Value },
            Text { label: String },
        }
        for union in ["oneOf", "anyOf"] {
            let mut schema = serde_json::json!({"$defs": {
                "Count": {"properties": {"kind": {"const": "count"},
                    "sequence": {"format": "uint64"}, "opaque": null}},
                "Text": {"properties": {"kind": {"const": "text"}, "label": {"type": "string"}}}
            }});
            schema[union] =
                serde_json::json!([{"$ref": "#/$defs/Text"}, {"$ref": "#/$defs/Count"}]);
            let source = Tagged::Count {
                sequence: u64::MAX,
                opaque: serde_json::json!({"exact": u64::MAX}),
            };
            let encoded = serde_json::to_value(Encoded {
                value: &source,
                schema: &schema,
                root: &schema,
            })
            .unwrap();
            assert_eq!(
                encoded,
                serde_json::json!({"kind": "count", "sequence": u64::MAX.to_string(), "opaque": {"exact": u64::MAX}})
            );
            let source = Tagged::Text {
                label: "water 水\n\"".into(),
            };
            assert_eq!(
                serde_json::to_value(Encoded {
                    value: &source,
                    schema: &schema,
                    root: &schema
                })
                .unwrap(),
                serde_json::to_value(&source).unwrap()
            );
        }
        let source = (u64::MAX, vec![u64::MAX, 17]);
        let schema = serde_json::json!({"prefixItems": [
            {"format": "uint64"}, {"allOf": [{"items": {"format": "uint64"}}]}
        ]});
        let encoded = serde_json::to_value(Encoded {
            value: &source,
            schema: &schema,
            root: &schema,
        })
        .unwrap();
        assert_eq!(
            encoded,
            serde_json::json!([u64::MAX.to_string(), [u64::MAX.to_string(), "17"]])
        );
        let source = std::collections::BTreeMap::from([("original", u64::MAX)]);
        let schema = serde_json::json!({"additionalProperties": {"format": "uint64"}});
        assert_eq!(
            serde_json::to_value(Encoded {
                value: &source,
                schema: &schema,
                root: &schema
            })
            .unwrap(),
            serde_json::json!({"original": u64::MAX.to_string()})
        );
    }

    #[test]
    fn t11_t20_definition_lookup_preserves_general_json_pointer_semantics() {
        fn general<'a>(schema: &'a Value, root: &'a Value) -> &'a Value {
            schema["$ref"]
                .as_str()
                .and_then(|reference| reference.strip_prefix('#'))
                .and_then(|reference| root.pointer(reference))
                .map_or(schema, |target| general(target, root))
        }
        let root = serde_json::json!({
            "$defs": {
                "plain": {"format": "uint64"},
                "alias": {"$ref": "#/$defs/plain"},
                "a/b": {"format": "escaped slash"},
                "a~b": {"format": "escaped tilde"},
                "日本語😀": {"format": "Unicode definition"},
                "nested": {"value": {"format": "nested"}}
            },
            "other": {"value": {"format": "outside definitions"}}
        });
        for reference in [
            "#/$defs/plain",
            "#/$defs/alias",
            "#/$defs/a~1b",
            "#/$defs/a~0b",
            "#/$defs/日本語😀",
            "#/$defs/nested/value",
            "#/$defs/missing",
            "#/other/value",
        ] {
            let schema = serde_json::json!({"$ref": reference});
            assert_eq!(resolve(&schema, &root), general(&schema, &root));
        }
        // Non-object definitions must retain general array-pointer behavior.
        let root = serde_json::json!({"$defs": [{"format": "array"}]});
        let schema = serde_json::json!({"$ref": "#/$defs/0"});
        assert_eq!(resolve(&schema, &root), general(&schema, &root));
    }
    #[derive(Serialize, schemars::JsonSchema)]
    struct SignedDomain {
        native: i64,
        opaque: Value,
    }
    #[test]
    fn t20_native_structural_schema_is_shared_by_type_without_source_data() {
        let first = root::<SignedDomain>();
        let second = root::<SignedDomain>();
        assert!(std::sync::Arc::ptr_eq(&first, &second));
        assert!(first.get("$defs").is_some() || first.get("properties").is_some());
    }
    #[test]
    fn t11_signed_native_quantities_match_wire_safe_integer_contract() {
        let mut bytes = Vec::new();
        to_writer(
            &mut bytes,
            &SignedDomain {
                native: -9_007_199_254_740_991,
                opaque: serde_json::json!({"integer": i64::MIN}),
            },
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["native"], -9_007_199_254_740_991_i64);
        assert_eq!(value["opaque"]["integer"], i64::MIN);
        for invalid in [i64::MIN, i64::MAX, -9_007_199_254_740_992] {
            assert!(
                to_writer(
                    std::io::sink(),
                    &SignedDomain {
                        native: invalid,
                        opaque: Value::Null,
                    },
                )
                .is_err()
            );
        }
    }
    #[derive(Serialize, schemars::JsonSchema)]
    struct NativeDomain {
        revision: u64,
        rows: Vec<NativeRow>,
        request_params: Value,
        indexed: (String, NativeRow),
    }
    #[derive(Serialize, schemars::JsonSchema)]
    struct NativeRow {
        bytes: u64,
        #[schemars(range(max = 65_536_u64))]
        quantity: u64,
    }
    #[test]
    fn t11_t15_native_structural_domains_preserve_exact_u64_and_opaque_json() {
        let source = NativeDomain {
            revision: u64::MAX,
            rows: vec![NativeRow {
                bytes: 9_007_199_254_740_993,
                quantity: 65_536,
            }],
            request_params: serde_json::json!({"opaque": u64::MAX}),
            indexed: (
                "row".into(),
                NativeRow {
                    bytes: u64::MAX,
                    quantity: 1,
                },
            ),
        };
        let mut bytes = Vec::new();
        to_writer(&mut bytes, &source).unwrap();
        let encoded: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(encoded["revision"], u64::MAX.to_string());
        assert_eq!(encoded["rows"][0]["bytes"], "9007199254740993");
        assert_eq!(encoded["rows"][0]["quantity"], 65_536);
        assert_eq!(encoded["request_params"]["opaque"], u64::MAX);
        assert_eq!(encoded["indexed"][1]["bytes"], u64::MAX.to_string());
        let root = root::<NativeDomain>();
        let path = vec!["rows".into(), "0".into(), "bytes".into()];
        let mut selected = Vec::new();
        serde_json::to_writer(
            &mut selected,
            &Encoded {
                value: &source.rows[0].bytes,
                schema: selected_schema(&root, &path),
                root: &root,
            },
        )
        .unwrap();
        assert_eq!(selected, b"\"9007199254740993\"");
    }
}
