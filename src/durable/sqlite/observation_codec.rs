//! Closed private observation records. Public JSON admission remains independent
//! of the storage encoding; there is no decoder for an earlier schema.
use super::{ConversationStoreError, storage};
use serde::{Deserialize, Serialize, de};

// MessagePack prefixes and f64 (9 bytes for a JSON `1.0`) can exceed the
// equivalent JSON bytes. Three times the JSON bound covers these closed,
// JSON-compatible records even on read/attach paths that only checked actual
// JSON length. Normal writers still apply the stricter original preflight.
pub(super) const MAX_BYTES: usize = crate::durable::observation::HEADER_ENCODED_MAX * 3;

struct Buffer(Vec<u8>);
impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn encode(
    public: &impl Serialize,
    record: &impl Serialize,
    limit: usize,
) -> Result<Vec<u8>, ConversationStoreError> {
    match crate::runtime_client::preflight::bounded_fit(public, limit) {
        Some(false) => return Err(storage("native observation header exceeds public bound")),
        // The existing conservative certificate proves JSON representability
        // and its byte bound. Do not traverse the same header again as JSON.
        Some(true) => {}
        None => {
            public_length(public, limit)?;
        }
    }
    encode_admitted(record)
}

pub(super) fn encode_admitted(record: &impl Serialize) -> Result<Vec<u8>, ConversationStoreError> {
    let mut buffer = Buffer(Vec::new());
    // Only the explicit outer tuple is positional. Nested public types include
    // internal tags and omitted optional fields, so their structs remain maps.
    // Human-readable serde branches match the original JSON representation.
    record
        .serialize(
            &mut rmp_serde::Serializer::new(&mut buffer)
                .with_struct_map()
                .with_human_readable(),
        )
        .map_err(|_| storage("native observation compact encoding failed"))?;
    Ok(buffer.0)
}

pub(super) fn decode<T: de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ConversationStoreError> {
    if bytes.len() > MAX_BYTES || !matches!(bytes.first(), Some(0x90..=0x9f | 0xdc | 0xdd)) {
        return Err(storage("native observation compact encoding invalid"));
    }
    // Check every actual element without retaining collections. Besides keeping
    // hostile length hints away from typed allocations, this rejects MessagePack
    // values JSON cannot represent (notably NaN, which Value would turn to null).
    // A valid container has at most one element per byte of this bounded input.
    let mut scan = rmp_serde::Deserializer::from_read_ref(bytes);
    scan.set_max_depth(128);
    JsonShape::deserialize(&mut scan)
        .map_err(|_| storage("native observation compact structure invalid"))?;
    let mut decoder =
        rmp_serde::Deserializer::new(std::io::Cursor::new(bytes)).with_human_readable();
    decoder.set_max_depth(128);
    let value = T::deserialize(&mut decoder)
        .map_err(|_| storage("native observation compact record corrupt"))?;
    if decoder.get_ref().position() != bytes.len() as u64 {
        return Err(storage(
            "native observation compact record has trailing bytes",
        ));
    }
    Ok(value)
}

struct JsonShape;
impl<'de> Deserialize<'de> for JsonShape {
    fn deserialize<D: de::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = JsonShape;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a finite JSON-compatible value")
            }
            fn visit_unit<E: de::Error>(self) -> Result<JsonShape, E> {
                Ok(JsonShape)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<JsonShape, E> {
                Ok(JsonShape)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<JsonShape, E> {
                Ok(JsonShape)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<JsonShape, E> {
                Ok(JsonShape)
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<JsonShape, E> {
                if value.is_finite() {
                    Ok(JsonShape)
                } else {
                    Err(E::custom("nonfinite JSON number"))
                }
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<JsonShape, E> {
                Ok(JsonShape)
            }
            fn visit_seq<A: de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<JsonShape, A::Error> {
                while sequence.next_element::<JsonShape>()?.is_some() {}
                Ok(JsonShape)
            }
            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<JsonShape, A::Error> {
                while map.next_key::<JsonKey>()?.is_some() {
                    map.next_value::<JsonShape>()?;
                }
                Ok(JsonShape)
            }
        }
        decoder.deserialize_any(Visitor)
    }
}

struct JsonKey;
impl<'de> Deserialize<'de> for JsonKey {
    fn deserialize<D: de::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = JsonKey;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON string key")
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<JsonKey, E> {
                Ok(JsonKey)
            }
        }
        decoder.deserialize_any(Visitor)
    }
}

/// The old SQL cells bounded actual JSON bytes, not the private byte count.
/// Preserve that check after decoding without allocating another JSON buffer.
pub(super) fn public_length(
    value: &impl Serialize,
    maximum: usize,
) -> Result<usize, ConversationStoreError> {
    struct Length {
        bytes: usize,
        maximum: usize,
    }
    impl std::io::Write for Length {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.maximum.saturating_sub(self.bytes) {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut length = Length { bytes: 0, maximum };
    serde_json::to_writer(&mut length, value)
        .map_err(|_| storage("native observation header exceeds JSON bound"))?;
    Ok(length.bytes)
}

#[cfg(test)]
mod tests;
