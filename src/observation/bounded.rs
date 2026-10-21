//! Bounded encoding of finite native display values. Allocation depends only on
//! the selected display budget, never on source size.
use serde::{Serialize, de::DeserializeOwned};

pub const CHECKPOINT_TARGET_BYTES: usize = 786_432;
pub const CHECKPOINT_PAGE_ITEMS: usize = 32;
pub const CHECKPOINT_ITEM_BYTES: usize = 16_384;
struct Buffer {
    bytes: Vec<u8>,
    limit: usize,
}
impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("bounded display item"));
        }
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() {
            let capacity = self
                .bytes
                .capacity()
                .max(8)
                .saturating_mul(2)
                .max(required)
                .min(self.limit);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
/// A capped serializer bounds both encoded length and buffer capacity.
/// The allocation depends only on the selected display budget, not source size.
pub(crate) fn bounded_bytes<T: Serialize + ?Sized>(value: &T, limit: usize) -> Option<Vec<u8>> {
    if !super::preflight::fits(value, limit) {
        return None;
    }
    let mut buffer = Buffer {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut buffer, value).ok()?;
    Some(buffer.bytes)
}
/// Decodes one source-typed native domain value, preserving exact native u64 fields.
/// Opaque JSON fields retain their original JSON semantics.
/// # Errors
/// Returns the native DTO's validation or shape error.
pub fn decode_domain_value<T: serde::de::DeserializeOwned + schemars::JsonSchema>(
    value: serde_json::Value,
) -> serde_json::Result<T> {
    super::structural_json::decode(value)
}
pub(crate) fn structural_value<T: serde::Serialize + schemars::JsonSchema>(
    value: &T,
) -> Option<serde_json::Value> {
    let schema = super::structural_json::root::<T>();
    let encoded = super::structural_json::Encoded {
        value,
        schema: &schema,
        root: &schema,
    };
    bounded_item(&encoded, CHECKPOINT_ITEM_BYTES).map(|(_, value)| value)
}
pub(crate) fn bounded_item_bytes(value: &impl Serialize, limit: usize) -> Option<Vec<u8>> {
    bounded_item(value, limit).map(|(bytes, _)| bytes)
}
// Admission already parses the capped bytes to check every container's item
// count. Callers needing JSON retain that same value instead of parsing twice.
pub(crate) fn bounded_item(
    value: &impl Serialize,
    limit: usize,
) -> Option<(Vec<u8>, serde_json::Value)> {
    fn bounded(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Array(items) => {
                items.len() <= CHECKPOINT_PAGE_ITEMS && items.iter().all(bounded)
            }
            serde_json::Value::Object(items) => {
                items.len() <= CHECKPOINT_PAGE_ITEMS && items.values().all(bounded)
            }
            _ => true,
        }
    }
    let bytes = bounded_bytes(value, limit)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    bounded(&value).then_some((bytes, value))
}
/// One immutable DTO's existing admission and normalization, prepared once by
/// its bounded native header owner. Both thresholds preserve later admission.
pub(crate) fn prepare_capture_item<T: Serialize + DeserializeOwned>(
    item: &T,
) -> Option<(T, usize, usize)> {
    let required = super::preflight::required_budget(item, CHECKPOINT_ITEM_BYTES)?;
    let (bytes, value) = bounded_item(item, CHECKPOINT_ITEM_BYTES)?;
    let encoded = bytes.len();
    Some((
        serde_json::from_value(value).ok()?,
        encoded,
        required.max(encoded),
    ))
}
pub(crate) fn bounded_clone<T: Serialize + DeserializeOwned>(value: &T, limit: usize) -> Option<T> {
    serde_json::from_slice(&bounded_item_bytes(value, limit)?).ok()
}
/// Opaque JSON is never rewritten with the typed protocol's integer codec.
/// A browser-unsafe integer is deferred to its exact authoritative UTF-8 bytes.
pub(crate) fn bounded_json_value(
    value: &serde_json::Value,
    limit: usize,
) -> Option<serde_json::Value> {
    fn safe(value: &serde_json::Value, remaining: &mut usize) -> bool {
        if *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        match value {
            serde_json::Value::Number(number) => {
                number
                    .as_u64()
                    .is_none_or(|value| value <= 9_007_199_254_740_991)
                    && number
                        .as_i64()
                        .is_none_or(|value| value.unsigned_abs() <= 9_007_199_254_740_991)
            }
            serde_json::Value::Array(values) => {
                values.len() <= CHECKPOINT_PAGE_ITEMS
                    && values.iter().all(|value| safe(value, remaining))
            }
            serde_json::Value::Object(values) => {
                values.len() <= CHECKPOINT_PAGE_ITEMS
                    && values.values().all(|value| safe(value, remaining))
            }
            _ => true,
        }
    }
    safe(value, &mut 512)
        .then(|| bounded_clone(value, limit))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t20_checkpoint_encoder_capacity_obeys_its_physical_byte_budget() {
        use std::io::Write;

        let value = serde_json::json!({
            "body": "x".repeat(600_000),
            "items": (0..30_000).collect::<Vec<u32>>(),
            "escaped": "\n\"\\\t水",
        });
        let mut buffer = Buffer {
            bytes: Vec::new(),
            limit: CHECKPOINT_TARGET_BYTES,
        };
        serde_json::to_writer(&mut buffer, &value).unwrap();
        assert_eq!(buffer.bytes, serde_json::to_vec(&value).unwrap());
        assert!(buffer.bytes.len() < buffer.limit);
        assert!(buffer.bytes.capacity() <= buffer.limit);
        let remaining = buffer.limit - buffer.bytes.len();
        buffer.write_all(&vec![b' '; remaining]).unwrap();
        let capacity = buffer.bytes.capacity();
        assert_eq!(buffer.bytes.len(), buffer.limit);
        assert!(capacity <= buffer.limit);
        assert!(buffer.write_all(b" ").is_err());
        assert_eq!(buffer.bytes.len(), buffer.limit);
        assert_eq!(buffer.bytes.capacity(), capacity);
    }

    #[test]
    fn t20_checkpoint_encoder_preserves_exact_bytes_at_small_budgets() {
        use std::io::Write;

        let value = "\n\"\\\t水";
        let expected = serde_json::to_vec(value).unwrap();
        let bytes = bounded_bytes(value, expected.len()).unwrap();
        assert_eq!(bytes, expected);
        assert!(bytes.capacity() <= expected.len());
        assert!(bounded_bytes(value, expected.len() - 1).is_none());
        for limit in [0, 1, 7, 17] {
            let mut buffer = Buffer {
                bytes: Vec::new(),
                limit,
            };
            for _ in 0..limit {
                buffer.write_all(b"x").unwrap();
                assert!(buffer.bytes.capacity() <= limit);
            }
            let capacity = buffer.bytes.capacity();
            assert!(buffer.write_all(b"x").is_err());
            assert_eq!(buffer.bytes.len(), limit);
            assert_eq!(buffer.bytes.capacity(), capacity);
        }
    }

    #[test]
    fn structural_item_reuses_admission_without_changing_bytes_counts_or_integer_domains() {
        #[derive(Serialize, schemars::JsonSchema)]
        struct Header {
            sequence: u64,
            opaque: serde_json::Value,
            items: Vec<u64>,
        }
        let mut header = Header {
            sequence: u64::MAX,
            opaque: serde_json::json!({"signed": -17, "fraction": 1.5, "text": "水\n\\\""}),
            items: (0..CHECKPOINT_PAGE_ITEMS as u64).collect(),
        };
        let projected = structural_value(&header).unwrap();
        assert_eq!(projected["sequence"], u64::MAX.to_string());
        assert_eq!(projected["opaque"], header.opaque);
        assert_eq!(
            projected["items"].as_array().unwrap().len(),
            CHECKPOINT_PAGE_ITEMS
        );
        assert_eq!(projected["items"][31], "31");
        header.items.push(32);
        assert!(structural_value(&header).is_none());

        let exact = "x".repeat(CHECKPOINT_ITEM_BYTES - 2);
        assert_eq!(structural_value(&exact), Some(serde_json::json!(exact)));
        assert!(structural_value(&(exact + "x")).is_none());
        let opaque = serde_json::json!({"integer": u64::MAX});
        assert!(bounded_json_value(&opaque, CHECKPOINT_ITEM_BYTES).is_none());
        assert_eq!(
            bounded_json_value(&header.opaque, CHECKPOINT_ITEM_BYTES),
            Some(header.opaque)
        );
        let map: std::collections::BTreeMap<_, _> = (0..=CHECKPOINT_PAGE_ITEMS)
            .map(|index| (index.to_string(), "original"))
            .collect();
        assert!(structural_value(&map).is_none());
    }
}
