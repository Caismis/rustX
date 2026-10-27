//! Finite structural admission before JSON encoding touches source strings.
use serde::ser::{self, SerializeMap, SerializeSeq, SerializeStruct};
use serde::{Serialize, Serializer};

#[cfg(test)]
thread_local! {
    static STRUCTURAL_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn with_structural_admission_only<T>(work: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            STRUCTURAL_ONLY.with(|flag| flag.set(self.0));
        }
    }
    let _restore = Restore(STRUCTURAL_ONLY.with(|flag| flag.replace(true)));
    work()
}

pub(crate) fn fits(value: &(impl Serialize + ?Sized), limit: usize) -> bool {
    bounded_fit(value, limit).unwrap_or(true)
}
/// A successful conservative walk proves the JSON byte bound. Unusual map
/// keys still require the capped encoder to establish JSON representability.
pub(crate) fn bounded_fit(value: &(impl Serialize + ?Sized), limit: usize) -> Option<bool> {
    checked_fit(value, limit, is_opaque_json(value))
}
/// Conservative admission for native structural encoding without its schema walk.
/// Treat every integer as opaque: unsafe signed/constrained quantities must
/// still reach the structural codec, while full-width native u64 may be quoted.
pub(crate) fn bounded_safe_integer_fit(
    value: &(impl Serialize + ?Sized),
    limit: usize,
) -> Option<bool> {
    #[cfg(test)]
    if STRUCTURAL_ONLY.with(std::cell::Cell::get) {
        return None;
    }
    checked_fit(value, limit, true)
}
/// A reusable sufficient charge for the same safe-integer admission walk.
/// Unknown map keys and unsafe quantities cannot produce a certificate.
pub(crate) fn required_safe_integer_budget(
    value: &(impl Serialize + ?Sized),
    limit: usize,
) -> Option<usize> {
    #[cfg(test)]
    if STRUCTURAL_ONLY.with(std::cell::Cell::get) {
        return None;
    }
    let mut left = limit;
    let mut uncertain = false;
    value
        .serialize(Check {
            left: &mut left,
            opaque_json: true,
            uncertain: &mut uncertain,
            map_key: false,
        })
        .ok()?;
    (!uncertain).then_some(limit - left)
}
fn checked_fit(value: &(impl Serialize + ?Sized), limit: usize, opaque_json: bool) -> Option<bool> {
    let mut left = limit;
    let mut uncertain = false;
    let admitted = value
        .serialize(Check {
            left: &mut left,
            opaque_json,
            uncertain: &mut uncertain,
            map_key: false,
        })
        .is_ok();
    if !admitted {
        Some(false)
    } else if uncertain {
        None
    } else {
        Some(true)
    }
}
/// The exact conservative threshold of the existing admission walk. A prepared
/// immutable DTO also retains its actual capped JSON size and representability.
pub(crate) fn required_budget(value: &(impl Serialize + ?Sized), limit: usize) -> Option<usize> {
    let mut left = limit;
    let mut uncertain = false;
    value
        .serialize(Check {
            left: &mut left,
            opaque_json: is_opaque_json(value),
            uncertain: &mut uncertain,
            map_key: false,
        })
        .ok()?;
    Some(limit - left)
}
// Native typed u64 domains have their own schema-driven decimal codec. Only
// opaque JSON values are forbidden from becoming lossy JavaScript numbers.
fn is_opaque_json<T: Serialize + ?Sized>(_: &T) -> bool {
    let name = std::any::type_name::<T>().trim_start_matches('&');
    name == std::any::type_name::<serde_json::Value>()
        || name == std::any::type_name::<serde_json::Number>()
}
#[derive(Debug)]
struct Full;
impl std::fmt::Display for Full {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("finite display structural budget")
    }
}
impl std::error::Error for Full {}
impl ser::Error for Full {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Self
    }
}
// Raw length rejects a huge borrowed string in O(1). Every scanned string is
// at most the remaining encoded budget; JSON escapes are charged exactly.
fn string_content(left: &mut usize, value: &str) -> Result<(), Full> {
    // Charge the original UTF-8 bytes once. Only escaped bytes add work to
    // the budget; ordinary bytes need no repeated checked subtraction.
    *left = left.checked_sub(value.len()).ok_or(Full)?;
    for byte in value.bytes() {
        let extra = match byte {
            b'"' | b'\\' | b'\x08' | b'\x0c' | b'\n' | b'\r' | b'\t' => 1,
            0..=0x1f => 5,
            _ => continue,
        };
        *left = left.checked_sub(extra).ok_or(Full)?;
    }
    Ok(())
}
struct Check<'a> {
    left: &'a mut usize,
    opaque_json: bool,
    uncertain: &'a mut bool,
    map_key: bool,
}
impl Check<'_> {
    fn uncertain_key(&mut self) {
        if self.map_key {
            *self.uncertain = true;
        }
    }
    fn named(&mut self, name: &str, punctuation: usize) -> Result<(), Full> {
        self.debit(punctuation)?;
        string_content(self.left, name)
    }
    fn debit(&mut self, bytes: usize) -> Result<(), Full> {
        *self.left = self.left.checked_sub(bytes).ok_or(Full)?;
        Ok(())
    }
    fn value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Full> {
        self.debit(1)?;
        value.serialize(Check {
            left: self.left,
            opaque_json: self.opaque_json || is_opaque_json(value),
            uncertain: self.uncertain,
            map_key: false,
        })
    }
}
macro_rules! number {($($name:ident($ty:ty)),*) => {$(fn $name(mut self,_value:$ty)->Result<(),Full>{self.debit(24)})*};}
impl Serializer for Check<'_> {
    type Ok = ();
    type Error = Full;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;
    number!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_char(char)
    );
    fn serialize_f32(mut self, _value: f32) -> Result<(), Full> {
        self.uncertain_key();
        self.debit(24)
    }
    fn serialize_f64(mut self, _value: f64) -> Result<(), Full> {
        self.uncertain_key();
        self.debit(24)
    }
    fn serialize_i64(mut self, value: i64) -> Result<(), Full> {
        if self.opaque_json && !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&value) {
            return Err(Full);
        }
        self.debit(24)
    }
    fn serialize_u64(mut self, value: u64) -> Result<(), Full> {
        if self.opaque_json && value > 9_007_199_254_740_991 {
            return Err(Full);
        }
        self.debit(24)
    }
    fn collect_str<T: std::fmt::Display + ?Sized>(mut self, value: &T) -> Result<(), Full> {
        // serde's default allocates Display::to_string before checking a bound.
        // Debit each borrowed formatting fragment and stop at the same budget.
        struct Fragments<'a>(&'a mut usize);
        impl std::fmt::Write for Fragments<'_> {
            fn write_str(&mut self, text: &str) -> std::fmt::Result {
                string_content(self.0, text).map_err(|_| std::fmt::Error)
            }
        }
        self.debit(2)?;
        std::fmt::write(&mut Fragments(self.left), format_args!("{value}")).map_err(|_| Full)
    }
    fn serialize_str(mut self, value: &str) -> Result<(), Full> {
        self.debit(2)?;
        string_content(self.left, value)
    }
    fn serialize_bytes(mut self, value: &[u8]) -> Result<(), Full> {
        self.uncertain_key();
        self.debit(
            value
                .len()
                .checked_mul(4)
                .and_then(|bytes| bytes.checked_add(2))
                .ok_or(Full)?,
        )
    }
    fn serialize_none(mut self) -> Result<(), Full> {
        self.uncertain_key();
        self.debit(4)
    }
    fn serialize_some<T: Serialize + ?Sized>(mut self, value: &T) -> Result<(), Full> {
        self.uncertain_key();
        self.value(value)
    }
    fn serialize_unit(mut self) -> Result<(), Full> {
        self.uncertain_key();
        self.debit(4)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Full> {
        self.serialize_unit()
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<(), Full> {
        self.serialize_str(variant)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        mut self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), Full> {
        self.uncertain_key();
        self.value(value)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        mut self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Full> {
        self.uncertain_key();
        self.named(variant, 5)?;
        self.value(value)
    }
    fn serialize_seq(mut self, len: Option<usize>) -> Result<Self, Full> {
        self.uncertain_key();
        self.debit(len.unwrap_or(0).checked_add(2).ok_or(Full)?)?;
        Ok(self)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self, Full> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<Self, Full> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        mut self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self, Full> {
        self.uncertain_key();
        self.named(variant, 5)?;
        self.serialize_seq(Some(len))
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self, Full> {
        self.serialize_seq(len)
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<Self, Full> {
        self.serialize_seq(Some(len))
    }
    fn serialize_struct_variant(
        mut self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self, Full> {
        self.uncertain_key();
        self.named(variant, 5)?;
        self.serialize_seq(Some(len))
    }
}
impl SerializeSeq for Check<'_> {
    type Ok = ();
    type Error = Full;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Full> {
        self.value(value)
    }
    fn end(self) -> Result<(), Full> {
        Ok(())
    }
}
impl ser::SerializeTuple for Check<'_> {
    type Ok = ();
    type Error = Full;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Full> {
        self.value(value)
    }
    fn end(self) -> Result<(), Full> {
        Ok(())
    }
}
impl ser::SerializeTupleStruct for Check<'_> {
    type Ok = ();
    type Error = Full;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Full> {
        self.value(value)
    }
    fn end(self) -> Result<(), Full> {
        Ok(())
    }
}
impl ser::SerializeTupleVariant for Check<'_> {
    type Ok = ();
    type Error = Full;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Full> {
        self.value(value)
    }
    fn end(self) -> Result<(), Full> {
        Ok(())
    }
}
impl SerializeMap for Check<'_> {
    type Ok = ();
    type Error = Full;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Full> {
        self.debit(1)?;
        value.serialize(Check {
            left: self.left,
            opaque_json: self.opaque_json || is_opaque_json(value),
            uncertain: self.uncertain,
            map_key: true,
        })
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Full> {
        self.value(value)
    }
    fn end(self) -> Result<(), Full> {
        Ok(())
    }
}
impl SerializeStruct for Check<'_> {
    type Ok = ();
    type Error = Full;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Full> {
        self.named(key, 3)?;
        self.value(value)
    }
    fn end(self) -> Result<(), Full> {
        Ok(())
    }
}
impl ser::SerializeStructVariant for Check<'_> {
    type Ok = ();
    type Error = Full;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Full> {
        self.named(key, 3)?;
        self.value(value)
    }
    fn end(self) -> Result<(), Full> {
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn safe_integer_proof_preserves_structural_admission_at_every_boundary() {
        #[derive(Serialize, schemars::JsonSchema)]
        struct Native {
            count: u64,
            signed: i64,
            #[schemars(range(max = 9_007_199_254_740_991_u64))]
            bounded: u64,
            opaque: serde_json::Value,
            names: Vec<String>,
        }
        let schema = super::super::structural_json::root::<Native>();
        let mut proved = 0;
        for count in [0, 1, 9_007_199_254_740_991, 9_007_199_254_740_992, u64::MAX] {
            for signed in [i64::MIN, -9_007_199_254_740_991, 0, i64::MAX] {
                for bounded in [0, 9_007_199_254_740_991, u64::MAX] {
                    for opaque in [
                        serde_json::json!({"quoted\"\n": [1, "水", null]}),
                        serde_json::json!({"wide": u64::MAX}),
                    ] {
                        let value = Native {
                            count,
                            signed,
                            bounded,
                            opaque,
                            names: vec!["a\"\\\n\u{0}水".repeat(3)],
                        };
                        let encoded = super::super::structural_json::Encoded {
                            value: &value,
                            schema: &schema,
                            root: &schema,
                        };
                        for limit in 0..512 {
                            if bounded_safe_integer_fit(&value, limit) == Some(true) {
                                proved += 1;
                                assert_eq!(bounded_fit(&encoded, limit), Some(true));
                            }
                        }
                    }
                }
            }
        }
        assert!(
            proved > 0,
            "the conservative shortcut admits ordinary native DTOs"
        );
    }

    #[test]
    fn safe_integer_proof_defers_wide_native_values_and_ambiguous_keys() {
        #[derive(Serialize, schemars::JsonSchema)]
        struct Native {
            count: u64,
        }
        #[derive(Serialize, schemars::JsonSchema)]
        struct Constrained {
            #[schemars(range(max = 9_007_199_254_740_991_u64))]
            count: u64,
        }
        #[derive(Serialize, schemars::JsonSchema)]
        struct Signed {
            count: i64,
        }
        let wide = Native { count: u64::MAX };
        assert_eq!(bounded_fit(&wide, 4096), Some(true));
        assert_eq!(bounded_safe_integer_fit(&wide, 4096), Some(false));
        assert!(super::super::checkpoint::structural_value(&wide).is_some());
        assert!(
            super::super::checkpoint::structural_value(&Constrained { count: u64::MAX }).is_none()
        );
        for count in [i64::MIN, i64::MAX] {
            let value = Signed { count };
            assert_eq!(bounded_safe_integer_fit(&value, 4096), Some(false));
            assert!(super::super::checkpoint::structural_value(&value).is_none());
        }
        assert_eq!(
            bounded_safe_integer_fit(&serde_json::json!({"wide": u64::MAX}), 4096),
            Some(false)
        );
        let ambiguous = std::collections::BTreeMap::from([((1_u8, 2_u8), 3_u8)]);
        assert_eq!(bounded_safe_integer_fit(&ambiguous, 4096), None);
    }

    fn prove_bound(value: &impl Serialize) {
        let bytes = serde_json::to_vec(value).unwrap();
        let first = (0..bytes.len() * 32 + 128)
            .find(|limit| bounded_fit(value, *limit) == Some(true))
            .expect("finite JSON obtains a proved upper bound");
        assert!(
            first >= bytes.len(),
            "admitted {first}, encoded {}",
            bytes.len()
        );
        assert_eq!(bounded_fit(value, first - 1), Some(false));
        assert_eq!(
            super::super::checkpoint::bounded_bytes(value, first).unwrap(),
            bytes
        );
    }
    struct Bytes<'a>(&'a [u8]);
    impl Serialize for Bytes<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_bytes(self.0)
        }
    }
    #[test]
    fn t06_t19_json_upper_bound_covers_bytes_escaped_keys_and_enum_punctuation() {
        #[derive(Serialize)]
        enum Variants {
            #[serde(rename = "tuple\"\n")]
            Tuple(),
            #[serde(rename = "newtype\\\u{0}")]
            Newtype(u64),
            #[serde(rename = "struct\t")]
            Struct {
                #[serde(rename = "key\"\n\u{0}")]
                value: String,
            },
        }
        for length in 0..=32 {
            let bytes = vec![255; length];
            let value = Bytes(&bytes);
            assert_eq!(bounded_fit(&value, 4 * length + 2), Some(true));
            prove_bound(&value);
        }
        prove_bound(&Variants::Tuple());
        prove_bound(&Variants::Newtype(u64::MAX));
        prove_bound(&Variants::Struct {
            value: "水🙂\"\n".into(),
        });
        prove_bound(&std::collections::BTreeMap::from([("\"\n\u{0}", u64::MAX)]));
        prove_bound(&[f64::MIN, f64::MAX, f64::MIN_POSITIVE, f64::NAN]);
        prove_bound(&['\u{0}', '水', '🙂']);
    }
    #[test]
    fn t06_t19_json_upper_bound_handles_unknown_lengths_and_checked_overflow() {
        struct Unknown;
        impl Serialize for Unknown {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut rows = serializer.serialize_seq(None)?;
                for value in ["\"\n", "水🙂", "tail"] {
                    rows.serialize_element(value)?;
                }
                rows.end()
            }
        }
        struct Overflow;
        impl Serialize for Overflow {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_seq(Some(usize::MAX))?.end()
            }
        }
        prove_bound(&Unknown);
        assert_eq!(bounded_fit(&Overflow, usize::MAX), Some(false));
    }
    #[test]
    fn t06_t19_uncertain_json_keys_use_the_capped_encoder() {
        let compound = std::collections::BTreeMap::from([((1_u8, 2_u8), 3_u8)]);
        assert_eq!(bounded_fit(&compound, 4096), None);
        assert!(fits(&compound, 4096));
        assert!(super::super::checkpoint::bounded_bytes(&compound, 4096).is_none());
        let optional = std::collections::BTreeMap::from([(Some("key"), "value")]);
        assert_eq!(bounded_fit(&optional, 4096), None);
        assert_eq!(
            super::super::checkpoint::bounded_bytes(&optional, 4096).unwrap(),
            serde_json::to_vec(&optional).unwrap()
        );
    }
    #[test]
    fn t06_t19_schema_uint64_quotes_remain_inside_the_proved_byte_bound() {
        #[derive(Serialize, schemars::JsonSchema)]
        struct Native {
            revision: u64,
            domains: std::collections::BTreeMap<String, u64>,
        }
        let value = Native {
            revision: u64::MAX,
            domains: std::collections::BTreeMap::from([("quoted\"\n".into(), u64::MAX)]),
        };
        let schema = super::super::structural_json::root::<Native>();
        let encoded = super::super::structural_json::Encoded {
            value: &value,
            schema: &schema,
            root: &schema,
        };
        let bytes = serde_json::to_vec(&encoded).unwrap();
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .contains("\"18446744073709551615\"")
        );
        prove_bound(&encoded);
        assert_eq!(
            bounded_fit(&serde_json::json!({"wide": u64::MAX}), 4096),
            Some(false)
        );
    }
    #[test]
    fn t20_display_preflight_stops_formatting_before_an_unbounded_string_is_allocated() {
        struct Displayed<'a>(&'a Cell<usize>);
        impl std::fmt::Display for Displayed<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                for _ in 0..1_000_000 {
                    self.0.set(self.0.get() + 1);
                    f.write_str("x")?;
                }
                Ok(())
            }
        }
        impl Serialize for Displayed<'_> {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }
        let work = Cell::new(0);
        assert!(!fits(&Displayed(&work), 2048));
        assert_eq!(
            work.get(),
            2047,
            "formatting stops exactly at the encoded byte budget, before allocating its unbounded result"
        );
    }
    #[test]
    fn t01_t15_string_budget_matches_original_byte_walk_at_every_boundary() {
        fn original(mut left: usize, value: &str) -> Option<usize> {
            if value.len() > left {
                return None;
            }
            for byte in value.bytes() {
                let encoded = match byte {
                    b'"' | b'\\' | b'\x08' | b'\x0c' | b'\n' | b'\r' | b'\t' => 2,
                    0..=0x1f => 6,
                    _ => 1,
                };
                left = left.checked_sub(encoded)?;
            }
            Some(left)
        }
        let controls: String = (0..=127).map(char::from).collect();
        for value in [
            String::new(),
            controls,
            "normal source identity".into(),
            "水🙂é\\\"\n\u{0}".repeat(16),
        ] {
            let encoded = serde_json::to_vec(&value).unwrap();
            for budget in 0..=encoded.len() + 2 {
                let mut remaining = budget;
                let actual = string_content(&mut remaining, &value)
                    .ok()
                    .map(|()| remaining);
                assert_eq!(actual, original(budget, &value), "budget={budget}");
            }
        }
    }
    #[test]
    fn t01_t15_string_admission_counts_actual_json_escapes_and_utf8_bytes() {
        for value in [
            "ascii".repeat(2800),
            "水🙂".repeat(1000),
            "\\\"\n\t\u{0}\u{1f}".repeat(1000),
        ] {
            let bytes = serde_json::to_vec(&value).unwrap();
            assert!(fits(&value, bytes.len()));
            assert!(!fits(&value, bytes.len() - 1));
        }
        assert!(
            fits(&"x".repeat(14_000), 16_384),
            "a valid finite ASCII item is not rejected by a sixfold estimate"
        );
    }
    #[test]
    fn t08_t19_opaque_wide_integers_defer_without_rejecting_exact_typed_u64_domains() {
        #[derive(Serialize)]
        struct Fields<'a> {
            typed_revision: u64,
            opaque: &'a serde_json::Value,
        }
        #[derive(Serialize)]
        struct Evidence {
            revision: u64,
            domains: std::collections::BTreeMap<String, u64>,
        }
        let wide = serde_json::json!({"wide":9_007_199_254_740_993_u64});
        assert!(!fits(&wide, 4096));
        assert!(!fits(
            &Fields {
                typed_revision: u64::MAX,
                opaque: &wide
            },
            4096
        ));
        assert!(fits(
            &Evidence {
                revision: u64::MAX,
                domains: std::collections::BTreeMap::from([("native".into(), u64::MAX)])
            },
            4096
        ));
        assert!(fits(
            &serde_json::json!({"wide":9_007_199_254_740_991_u64}),
            4096
        ));
        assert_eq!(
            serde_json::to_string(&wide).unwrap(),
            "{\"wide\":9007199254740993}"
        );
    }
    struct Rows<'a>(&'a Cell<usize>);
    impl Serialize for Rows<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut rows = serializer.serialize_seq(None)?;
            for _ in 0..1_000_000 {
                self.0.set(self.0.get() + 1);
                rows.serialize_element(&0_u64)?;
            }
            rows.end()
        }
    }
    #[test]
    fn t15_t20_structural_admission_stops_source_walk_before_encoding() {
        let visited = Cell::new(0);
        assert!(!fits(&Rows(&visited), 1024));
        assert!(
            visited.get() <= 42,
            "bounded source walk: {}",
            visited.get()
        );
        let giant = "x".repeat(2 * 1024 * 1024);
        assert!(!fits(&giant, 1024));
        assert!(fits(&"a\nb", 64));
    }
}
