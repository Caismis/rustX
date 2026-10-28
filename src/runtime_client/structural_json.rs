//! Source-owned JSON encoding for typed native display domains.
//! Exact u64 fields use decimal text; opaque JSON values keep their own semantics.
use serde::Serialize;
use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use serde_json::Value;

mod plan;
#[cfg(test)]
mod plan_tests;
use plan::{Unsigned, View};

#[cfg(test)]
thread_local! {
    static LOOKUP_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    static PROPERTY_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    static LEGACY_PLAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static LEGACY_PROPERTY_LOOKUP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static STRING_KEY_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static GENERIC_KEY_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MAP_KEY_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TAG_VALUE_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
        #[cfg(test)]
        if LEGACY_PLAN.with(std::cell::Cell::get) {
            return self.value.serialize(Encoder {
                serializer,
                wire: false,
                schema: View::Interpreted {
                    schema: self.schema,
                    root: self.root,
                },
            });
        }
        let plan = encoding_plan(self.schema, self.root);
        self.value.serialize(Encoder {
            serializer,
            wire: false,
            schema: plan.select(self.schema).expect("compiled encoding root"),
        })
    }
}
// Recursive callbacks carry the already selected plan node. They never reacquire
// a schema cache or interpret the JSON schema again.
struct ValueAt<'a, T: ?Sized> {
    value: &'a T,
    schema: View<'a>,
    wire: bool,
}
impl<T: Serialize + ?Sized> Serialize for ValueAt<'_, T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(Encoder {
            serializer,
            schema: self.schema,
            wire: self.wire,
        })
    }
}
struct Encoder<'a, S> {
    serializer: S,
    schema: View<'a>,
    wire: bool,
}
/// Encode App Server's closed, Rust-derived envelopes directly. Native admission
/// remains owned by that protocol; its flattened enum tags precede their payload.
/// This mode preserves its untagged scalar and tuple rules independently of
/// source-domain encoding.
pub(crate) fn serialize_wire<T: Serialize + ?Sized, S: serde::Serializer>(
    value: &T,
    schema: &Value,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let plan = encoding_plan(schema, schema);
    value.serialize(Encoder {
        serializer,
        schema: plan.select(schema).expect("compiled envelope schema"),
        wire: true,
    })
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
        serialize_f64(f64),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8])
    );
    fn serialize_i8(self, value: i8) -> Result<S::Ok, S::Error> {
        if self.wire {
            self.serialize_i64(i64::from(value))
        } else {
            self.serializer.serialize_i8(value)
        }
    }
    fn serialize_i16(self, value: i16) -> Result<S::Ok, S::Error> {
        if self.wire {
            self.serialize_i64(i64::from(value))
        } else {
            self.serializer.serialize_i16(value)
        }
    }
    fn serialize_i32(self, value: i32) -> Result<S::Ok, S::Error> {
        if self.wire {
            self.serialize_i64(i64::from(value))
        } else {
            self.serializer.serialize_i32(value)
        }
    }
    fn serialize_u8(self, value: u8) -> Result<S::Ok, S::Error> {
        if self.wire {
            self.serialize_u64(u64::from(value))
        } else {
            self.serializer.serialize_u8(value)
        }
    }
    fn serialize_u16(self, value: u16) -> Result<S::Ok, S::Error> {
        if self.wire {
            self.serialize_u64(u64::from(value))
        } else {
            self.serializer.serialize_u16(value)
        }
    }
    fn serialize_u32(self, value: u32) -> Result<S::Ok, S::Error> {
        if self.wire {
            self.serialize_u64(u64::from(value))
        } else {
            self.serializer.serialize_u32(value)
        }
    }
    fn serialize_f32(self, value: f32) -> Result<S::Ok, S::Error> {
        if self.wire {
            self.serializer.serialize_f64(f64::from(value))
        } else {
            self.serializer.serialize_f32(value)
        }
    }
    fn serialize_i64(self, value: i64) -> Result<S::Ok, S::Error> {
        if self.wire {
            return self
                .schema
                .wire_number(Value::from(value))
                .map_err(serde::ser::Error::custom)?
                .serialize(self.serializer);
        }
        if self.schema.signed_safe() && value.unsigned_abs() > 9_007_199_254_740_991 {
            return Err(serde::ser::Error::custom(
                "native quantity exceeds safe JSON integer range",
            ));
        }
        self.serializer.serialize_i64(value)
    }
    fn serialize_u64(self, value: u64) -> Result<S::Ok, S::Error> {
        if self.wire {
            return self
                .schema
                .wire_number(Value::from(value))
                .map_err(serde::ser::Error::custom)?
                .serialize(self.serializer);
        }
        match self.schema.unsigned() {
            Unsigned::Decimal => self.serializer.serialize_str(&value.to_string()),
            Unsigned::Safe if value > 9_007_199_254_740_991 => Err(serde::ser::Error::custom(
                "native quantity exceeds safe JSON integer range",
            )),
            _ => self.serializer.serialize_u64(value),
        }
    }
    fn serialize_none(self) -> Result<S::Ok, S::Error> {
        self.serializer.serialize_none()
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<S::Ok, S::Error> {
        self.serializer.serialize_some(&ValueAt {
            value,
            schema: self.schema,
            wire: self.wire,
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
            &ValueAt {
                value,
                schema: self.schema,
                wire: self.wire,
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
            &ValueAt {
                value,
                schema: self.schema.child(Some(variant)),
                wire: self.wire,
            },
        )
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, S::Error> {
        Ok(Sequence::new(
            self.serializer.serialize_seq(len)?,
            self.schema,
            self.wire,
        ))
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, S::Error> {
        Ok(Sequence::new(
            self.serializer.serialize_tuple(len)?,
            self.schema,
            self.wire,
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
            self.wire,
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
            if self.wire {
                self.schema.child(Some(variant))
            } else {
                self.schema
            },
            self.wire,
        ))
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, S::Error> {
        Ok(Mapping::new(
            self.serializer.serialize_map(len)?,
            self.schema,
            self.wire,
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
            self.wire,
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
            self.schema.child(Some(variant)),
            self.wire,
        ))
    }
}
struct Sequence<'a, S> {
    inner: S,
    schema: View<'a>,
    index: usize,
    wire: bool,
}
impl<'a, S> Sequence<'a, S> {
    fn new(inner: S, schema: View<'a>, wire: bool) -> Self {
        Self {
            inner,
            schema,
            index: 0,
            wire,
        }
    }
}
macro_rules! sequence {
    ($trait:ident,$method:ident) => {
        impl<S: $trait> $trait for Sequence<'_, S> {
            type Ok = S::Ok;
            type Error = S::Error;
            fn $method<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
                let schema = if self.wire {
                    self.schema.wire_element()
                } else {
                    self.schema.element(self.index)
                };
                self.index += 1;
                self.inner.$method(&ValueAt {
                    value,
                    schema,
                    wire: self.wire,
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
macro_rules! string_key_reject {
    ($( $method:ident($($name:ident: $ty:ty),*) -> $result:ident; )*) => {$(
        fn $method(self, $($name: $ty),*) -> Result<Self::$result, Self::Error> {
            let _ = ($($name,)*);
            Err(serde::ser::Error::custom("non-string key probe"))
        }
    )*};
}
// Only built-in strings use this probe; every custom/numeric key retains the
// original finite serializer, including its callback count and error behavior.
struct StringKey;
impl serde::Serializer for StringKey {
    type Ok = Option<String>;
    type Error = serde::de::value::Error;
    type SerializeSeq = serde::ser::Impossible<Self::Ok, Self::Error>;
    type SerializeTuple = Self::SerializeSeq;
    type SerializeTupleStruct = Self::SerializeSeq;
    type SerializeTupleVariant = Self::SerializeSeq;
    type SerializeMap = Self::SerializeSeq;
    type SerializeStruct = Self::SerializeSeq;
    type SerializeStructVariant = Self::SerializeSeq;
    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        if value.len() > 256 {
            return Ok(None);
        }
        let mut remaining = 256_usize;
        for byte in value.bytes() {
            let bytes = match byte {
                b'"' | b'\\' | b'\x08' | b'\x0c' | b'\n' | b'\r' | b'\t' => 2,
                0..=0x1f => 6,
                _ => 1,
            };
            let Some(left) = remaining.checked_sub(bytes) else {
                return Ok(None);
            };
            remaining = left;
        }
        Ok(Some(value.to_owned()))
    }
    string_key_reject! {
        serialize_bool(value: bool) -> Ok;
        serialize_i8(value: i8) -> Ok;
        serialize_i16(value: i16) -> Ok;
        serialize_i32(value: i32) -> Ok;
        serialize_i64(value: i64) -> Ok;
        serialize_i128(value: i128) -> Ok;
        serialize_u8(value: u8) -> Ok;
        serialize_u16(value: u16) -> Ok;
        serialize_u32(value: u32) -> Ok;
        serialize_u64(value: u64) -> Ok;
        serialize_u128(value: u128) -> Ok;
        serialize_f32(value: f32) -> Ok;
        serialize_f64(value: f64) -> Ok;
        serialize_char(value: char) -> Ok;
        serialize_bytes(value: &[u8]) -> Ok;
        serialize_none() -> Ok;
        serialize_unit() -> Ok;
        serialize_unit_struct(name: &'static str) -> Ok;
        serialize_unit_variant(name: &'static str, index: u32, variant: &'static str) -> Ok;
        serialize_seq(len: Option<usize>) -> SerializeSeq;
        serialize_tuple(len: usize) -> SerializeTuple;
        serialize_tuple_struct(name: &'static str, len: usize) -> SerializeTupleStruct;
        serialize_tuple_variant(name: &'static str, index: u32, variant: &'static str, len: usize) -> SerializeTupleVariant;
        serialize_map(len: Option<usize>) -> SerializeMap;
        serialize_struct(name: &'static str, len: usize) -> SerializeStruct;
        serialize_struct_variant(name: &'static str, index: u32, variant: &'static str, len: usize) -> SerializeStructVariant;
    }
    fn serialize_some<T: Serialize + ?Sized>(self, _value: &T) -> Result<Self::Ok, Self::Error> {
        Err(serde::ser::Error::custom("non-string key probe"))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Err(serde::ser::Error::custom("non-string key probe"))
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Err(serde::ser::Error::custom("non-string key probe"))
    }
    fn collect_str<T: std::fmt::Display + ?Sized>(
        self,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Err(serde::ser::Error::custom("non-string key probe"))
    }
}
fn captured_key<T: Serialize + ?Sized>(key: &T) -> Option<String> {
    let builtin_string = matches!(
        std::any::type_name::<T>().trim_start_matches('&'),
        "str" | "alloc::string::String"
    );
    #[cfg(test)]
    let builtin_string = builtin_string && !LEGACY_PROPERTY_LOOKUP.with(std::cell::Cell::get);
    if builtin_string {
        #[cfg(test)]
        STRING_KEY_WORK.with(|work| work.set(work.get() + 1));
        if let Ok(value) = key.serialize(StringKey) {
            return value;
        }
    }
    #[cfg(test)]
    GENERIC_KEY_WORK.with(|work| work.set(work.get() + 1));
    super::checkpoint::bounded_bytes(key, 258)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|key| match key {
            Value::String(key) => Some(key),
            Value::Number(key) => Some(key.to_string()),
            _ => None,
        })
}

struct Mapping<'a, S> {
    inner: S,
    schema: View<'a>,
    key: Option<String>,
    ordered_next: usize,
    wire: bool,
}
impl<'a, S> Mapping<'a, S> {
    fn new(inner: S, schema: View<'a>, wire: bool) -> Self {
        Self {
            inner,
            schema,
            key: None,
            ordered_next: 0,
            wire,
        }
    }
    fn wire_tag<T: Serialize + ?Sized>(&mut self, key: &str, value: &T) {
        if !self.schema.has_wire_tag(key) {
            return;
        }
        if let Ok(value) = serde_json::to_value(value)
            && let Some(branch) = self.schema.wire_variant(key, &value)
        {
            self.select_schema(branch);
        }
    }
    fn select_schema(&mut self, schema: View<'a>) {
        self.schema = schema;
        self.ordered_next = 0;
    }
    fn field(&self, key: &str) -> View<'a> {
        #[cfg(test)]
        PROPERTY_WORK.with(|work| {
            let (ordered, fallback) = work.get();
            work.set((ordered, fallback + 1));
        });
        self.schema.child(Some(key))
    }
    fn struct_field(&mut self, key: &str) -> View<'a> {
        #[cfg(test)]
        if LEGACY_PROPERTY_LOOKUP.with(std::cell::Cell::get) {
            return self.field(key);
        }
        while let Some((name, schema)) = self.schema.property(self.ordered_next) {
            self.ordered_next += 1;
            if name == key {
                #[cfg(test)]
                PROPERTY_WORK.with(|work| {
                    let (ordered, fallback) = work.get();
                    work.set((ordered + 1, fallback));
                });
                return schema;
            }
        }
        self.field(key)
    }
}
impl<S: SerializeMap> SerializeMap for Mapping<'_, S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Self::Error> {
        let capture_key = !self.schema.is_null();
        #[cfg(test)]
        let capture_key = capture_key || LEGACY_PROPERTY_LOOKUP.with(std::cell::Cell::get);
        if !capture_key {
            self.key = None;
            return self.inner.serialize_key(key);
        }
        #[cfg(test)]
        MAP_KEY_WORK.with(|work| work.set(work.get() + 1));
        self.key = captured_key(key);
        self.inner.serialize_key(key)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        if self.wire
            && let Some(key) = self.key.take()
        {
            self.wire_tag(&key, value);
            self.key = Some(key);
        }
        let schema = self.field(self.key.as_deref().unwrap_or(""));
        self.inner.serialize_value(&ValueAt {
            value,
            schema,
            wire: self.wire,
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
                // Keep oneOf followed by anyOf, and the original first matching
                // branch in each group. Constants retain their serde callbacks.
                if self.wire {
                    self.wire_tag(key, value);
                }
                let before = self.schema;
                let builtin_tag = matches!(
                    std::any::type_name::<T>().trim_start_matches('&'),
                    "str" | "alloc::string::String"
                );
                #[cfg(test)]
                let builtin_tag = builtin_tag && !LEGACY_PLAN.with(std::cell::Cell::get);
                // Generated enum tags are immutable built-in strings. Custom
                // callbacks retain their original per-candidate invocation.
                let mut tag = None;
                for kind in 0..if self.wire { 0 } else { 2 } {
                    let mut index = 0;
                    while let Some(branch) = before.branch(kind, index) {
                        index += 1;
                        if let Some(expected) = branch.tag(key) {
                            let encode = || {
                                #[cfg(test)]
                                TAG_VALUE_WORK.with(|work| work.set(work.get() + 1));
                                serde_json::to_value(value).ok()
                            };
                            let matches = if builtin_tag {
                                tag.get_or_insert_with(encode).as_ref() == Some(expected)
                            } else {
                                encode().as_ref() == Some(expected)
                            };
                            if matches {
                                self.select_schema(branch);
                                break;
                            }
                        }
                    }
                }
                let schema = self.struct_field(key);
                self.inner.serialize_field(
                    key,
                    &ValueAt {
                        value,
                        schema,
                        wire: self.wire,
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
#[derive(Default)]
struct SchemaCache {
    roots: std::collections::BTreeMap<&'static str, std::sync::Arc<Value>>,
    plans: std::collections::HashMap<usize, std::sync::Arc<plan::Plan>>,
}
static SCHEMAS: std::sync::LazyLock<std::sync::Mutex<SchemaCache>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(SchemaCache::default()));

fn encoding_plan(schema: &Value, root: &Value) -> std::sync::Arc<plan::Plan> {
    let cached = SCHEMAS
        .lock()
        .expect("native schemas")
        .plans
        .get(&(std::ptr::from_ref(root) as usize))
        .cloned();
    if let Some(plan) = cached
        && plan.select(schema).is_some()
    {
        return plan;
    }
    // Arbitrary borrowed test/inspection schemas never enter the type cache.
    std::sync::Arc::new(plan::Plan::new(schema, root))
}
pub(crate) fn root<T: schemars::JsonSchema>() -> std::sync::Arc<Value> {
    let identity = std::any::type_name::<T>();
    if let Some(schema) = SCHEMAS
        .lock()
        .expect("native schemas")
        .roots
        .get(identity)
        .cloned()
    {
        return schema;
    }
    // Generate outside the cache boundary: custom schemas may request another
    // native type. The plan contains schema nodes only, never captured values.
    let schema = std::sync::Arc::new(
        serde_json::to_value(schemars::schema_for!(T)).expect("native structural schema"),
    );
    let plan = std::sync::Arc::new(plan::Plan::new(&schema, &schema));
    let mut cache = SCHEMAS.lock().expect("native schemas");
    if let Some(existing) = cache.roots.get(identity) {
        return existing.clone();
    }
    cache
        .plans
        .insert(std::sync::Arc::as_ptr(&schema) as usize, plan);
    cache.roots.insert(identity, schema.clone());
    schema
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

    type PropertyEncoding = (
        Result<Vec<u8>, String>,
        Vec<(Option<bool>, Option<Vec<u8>>)>,
    );
    pub(super) fn property_encoding<T: Serialize>(
        value: &T,
        schema: &Value,
        legacy: bool,
    ) -> PropertyEncoding {
        struct Reset(bool, bool);
        impl Drop for Reset {
            fn drop(&mut self) {
                LEGACY_PROPERTY_LOOKUP.with(|mode| mode.set(self.0));
                LEGACY_PLAN.with(|mode| mode.set(self.1));
            }
        }
        let _reset = Reset(
            LEGACY_PROPERTY_LOOKUP.with(|mode| mode.replace(legacy)),
            LEGACY_PLAN.with(|mode| mode.replace(legacy)),
        );
        PROPERTY_WORK.with(|work| work.set((0, 0)));
        MAP_KEY_WORK.with(|work| work.set(0));
        let encoded = Encoded {
            value,
            schema,
            root: schema,
        };
        let bytes = serde_json::to_vec(&encoded).map_err(|error| error.to_string());
        let admission = [0, 32, 256, 4096, 65_536, 786_432]
            .into_iter()
            .map(|limit| {
                (
                    super::super::preflight::bounded_fit(&encoded, limit),
                    super::super::checkpoint::bounded_bytes(&encoded, limit),
                )
            })
            .collect();
        (bytes, admission)
    }

    #[test]
    fn t11_t14_t20_ordered_schema_properties_preserve_complete_checkpoint_bytes_and_admission() {
        let mut checkpoint = super::super::projection::RuntimeClientProjection::new(
            crate::runtime::identity::ConversationId::new(
                "conv_77621431-ad5b-7155-9eca-9f21014c0bc9",
            ),
            Vec::new(),
            serde_json::from_value(serde_json::json!({"configured_tools":[],"revision":1}))
                .unwrap(),
            None,
            64,
        )
        .checkpoint_candidate()
        .unwrap();
        checkpoint.read_cut.through = u64::MAX;
        checkpoint.read_cut.revision = u64::MAX;
        checkpoint
            .read_cut
            .domain_revisions
            .insert("trace".into(), u64::MAX);
        let schema = root::<super::super::checkpoint::RuntimeClientCheckpoint>();
        let expected = property_encoding(&checkpoint, &schema, true);
        let (_, old_fallbacks) = PROPERTY_WORK.with(std::cell::Cell::get);
        let actual = property_encoding(&checkpoint, &schema, false);
        let (ordered, fallbacks) = PROPERTY_WORK.with(std::cell::Cell::get);
        assert_eq!(actual, expected);
        assert!(ordered > 100 && fallbacks < old_fallbacks);
        let value: Value = serde_json::from_slice(actual.0.as_ref().unwrap()).unwrap();
        assert_eq!(value["read_cut"]["through"], u64::MAX.to_string());
        assert_eq!(value["read_cut"]["revision"], u64::MAX.to_string());
    }

    #[test]
    fn t11_t20_ordered_schema_properties_keep_custom_order_missing_fields_and_tagged_branches() {
        struct Custom;
        impl Serialize for Custom {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut fields = serializer.serialize_struct("Custom", 4)?;
                fields.serialize_field("third", &u64::MAX)?;
                fields.serialize_field("first", &17_u64)?;
                fields.serialize_field("missing", &u64::MAX)?;
                fields.serialize_field("third", &19_u64)?;
                fields.end()
            }
        }
        #[derive(Serialize, schemars::JsonSchema)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Tagged {
            Record {
                sequence: u64,
                #[serde(skip_serializing_if = "Option::is_none")]
                skipped: Option<String>,
                opaque: Value,
            },
            Empty,
        }
        let mut schema = serde_json::json!({"properties": {
            "first": {"format":"uint64"}, "skipped": {"type":"string"},
            "third": {"format":"uint64"}
        },"additionalProperties":{"format":"uint64"}});
        let expected = property_encoding(&Custom, &schema, true);
        assert_eq!(property_encoding(&Custom, &schema, false), expected);
        let (ordered, fallback) = PROPERTY_WORK.with(std::cell::Cell::get);
        assert!(ordered > 0 && fallback > 0);
        schema["properties"]["third"]["maximum"] = serde_json::json!(9_007_199_254_740_991_u64);
        let expected = property_encoding(&Custom, &schema, true);
        assert!(
            expected.0.is_err(),
            "native coordinate validation remains active"
        );
        assert_eq!(property_encoding(&Custom, &schema, false), expected);
        let schema = root::<Tagged>();
        for source in [
            Tagged::Record {
                sequence: u64::MAX,
                skipped: None,
                opaque: serde_json::json!({"unmodified": u64::MAX,"escaped":"水\n\"\\"}),
            },
            Tagged::Empty,
        ] {
            let expected = property_encoding(&source, &schema, true);
            assert_eq!(property_encoding(&source, &schema, false), expected);
        }
    }

    struct KeyMap<'a, K: ?Sized>(&'a K);
    impl<K: Serialize + ?Sized> Serialize for KeyMap<'_, K> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut map = serializer.serialize_map(Some(1))?;
            map.serialize_entry(self.0, &u64::MAX)?;
            map.end()
        }
    }
    fn assert_key_map_equivalence<K: Serialize + ?Sized>(key: &K) {
        let schema = serde_json::json!({"additionalProperties":{"format":"uint64"}});
        let expected = property_encoding(&KeyMap(key), &schema, true);
        assert_eq!(property_encoding(&KeyMap(key), &schema, false), expected);
    }
    #[test]
    fn t11_t20_string_map_keys_match_legacy_escaped_admission_without_json_roundtrips() {
        let keys = [
            String::new(),
            "plain".into(),
            "水🙂\n\"\\\u{0001}".into(),
            "x".repeat(256),
            "x".repeat(257),
            "\n".repeat(128),
            format!("{}x", "\n".repeat(128)),
            format!("{}xxxx", "\u{0001}".repeat(42)),
            format!("{}xxxxx", "\u{0001}".repeat(42)),
        ];
        for key in &keys {
            let legacy = super::super::checkpoint::bounded_bytes(key, 258)
                .and_then(|bytes| serde_json::from_slice::<String>(&bytes).ok());
            assert_eq!(captured_key(key), legacy);
            assert_key_map_equivalence(key);
            assert_key_map_equivalence(key.as_str());
            assert_key_map_equivalence(&key);
            assert_key_map_equivalence(&&key);
        }
        for key in ["x".repeat(4 * 1024 * 1024), "\u{0001}".repeat(257)] {
            assert!(captured_key(&key).is_none());
            assert!(super::super::checkpoint::bounded_bytes(&key, 258).is_none());
        }
        STRING_KEY_WORK.with(|work| work.set(0));
        GENERIC_KEY_WORK.with(|work| work.set(0));
        assert_eq!(captured_key(&"exact"), Some("exact".into()));
        assert_eq!(
            captured_key(&&&String::from("nested")),
            Some("nested".into())
        );
        assert_eq!(STRING_KEY_WORK.with(std::cell::Cell::get), 2);
        assert_eq!(GENERIC_KEY_WORK.with(std::cell::Cell::get), 0);
        let schema = serde_json::json!({"properties":{"native":{"format":"uint64"}}});
        let source = std::collections::BTreeMap::from([("native", u64::MAX)]);
        assert_eq!(
            property_encoding(&source, &schema, false),
            property_encoding(&source, &schema, true)
        );
    }
    #[test]
    fn t11_t19_string_key_probe_never_replays_custom_numeric_or_invalid_key_callbacks() {
        struct DisplayKey;
        impl Serialize for DisplayKey {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(&"native")
            }
        }
        struct Custom<'a>(&'a std::cell::Cell<usize>);
        impl Serialize for Custom<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.0.set(self.0.get() + 1);
                serializer.serialize_newtype_struct("OriginalKey", "native")
            }
        }
        let calls = std::cell::Cell::new(0);
        let key = Custom(&calls);
        let schema = serde_json::json!({"properties":{"native":{"format":"uint64"}}});
        let expected = property_encoding(&KeyMap(&key), &schema, true);
        let legacy_calls = calls.replace(0);
        assert_eq!(property_encoding(&KeyMap(&key), &schema, false), expected);
        assert_eq!(calls.get(), legacy_calls);
        assert!(legacy_calls > 0);
        assert_key_map_equivalence(&u64::MAX);
        assert_key_map_equivalence(&i64::MIN);
        assert_key_map_equivalence(&1.25_f64);
        assert_key_map_equivalence(&f64::NAN);
        assert_key_map_equivalence(&true);
        assert_key_map_equivalence(&'水');
        assert_key_map_equivalence(&());
        assert_key_map_equivalence(&Some("native"));
        assert_key_map_equivalence(&vec!["invalid", "compound"]);
        assert_key_map_equivalence(&DisplayKey);
        STRING_KEY_WORK.with(|work| work.set(0));
        GENERIC_KEY_WORK.with(|work| work.set(0));
        let _ = captured_key(&key);
        assert_eq!(STRING_KEY_WORK.with(std::cell::Cell::get), 0);
        assert_eq!(GENERIC_KEY_WORK.with(std::cell::Cell::get), 1);
    }

    #[test]
    fn t11_t20_null_schema_map_keys_skip_only_unused_capture_and_keep_opaque_and_invalid_keys() {
        let mut source = serde_json::json!({
            "exact": u64::MAX, "signed": i64::MIN,
            "nested": [{"escaped":"水\n\"\\", "raw":"{\"n\":1.000}"}]
        });
        source.as_object_mut().unwrap().insert(
            "escaped\n\"\\水".repeat(64),
            serde_json::json!({"original":"long key remains exact"}),
        );
        let expected = property_encoding(&source, &Value::Null, true);
        assert!(MAP_KEY_WORK.with(std::cell::Cell::get) > 0);
        assert_eq!(property_encoding(&source, &Value::Null, false), expected);
        assert_eq!(MAP_KEY_WORK.with(std::cell::Cell::get), 0);
        assert_eq!(expected.0.unwrap(), serde_json::to_vec(&source).unwrap());
        for key in [vec![1_u64, 2], Vec::new()] {
            let source = std::collections::BTreeMap::from([(key, "original")]);
            let expected = property_encoding(&source, &Value::Null, true);
            assert!(expected.0.is_err(), "compound JSON keys remain invalid");
            assert_eq!(property_encoding(&source, &Value::Null, false), expected);
            assert_eq!(MAP_KEY_WORK.with(std::cell::Cell::get), 0);
        }
        let source = std::collections::BTreeMap::from([(u64::MAX, "exact numeric key")]);
        let schema = serde_json::json!({"additionalProperties":{"type":"string"}});
        let expected = property_encoding(&source, &schema, true);
        assert_eq!(property_encoding(&source, &schema, false), expected);
        assert!(MAP_KEY_WORK.with(std::cell::Cell::get) > 0);
    }
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
        let plan = encoding_plan(&schema, &schema);
        lookup_work();
        let actual = serde_json::to_vec(&ValueAt {
            value: &rows,
            schema: plan.select(&schema).unwrap(),
            wire: false,
        })
        .unwrap();
        // The plan resolves the reference chain once, independent of both row
        // count and field count; serializing values performs no JSON lookup.
        assert_eq!(lookup_work(), (0, 0));
        assert_eq!(actual, expected);
    }

    #[test]
    fn t11_t20_tuple_prefix_does_not_visit_an_unused_item_fallback() {
        let schema = serde_json::json!({
            "prefixItems": [null, null],
            "allOf": [{"items": {"format": "uint64"}}]
        });
        let source = ("first", "second");
        let plan = encoding_plan(&schema, &schema);
        lookup_work();
        let encoded = serde_json::to_vec(&ValueAt {
            value: &source,
            schema: plan.select(&schema).unwrap(),
            wire: false,
        })
        .unwrap();
        assert_eq!(lookup_work(), (0, 0));
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
