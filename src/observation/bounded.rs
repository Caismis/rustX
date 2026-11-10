//! Bounded encoding of finite native display values. Allocation depends only on
//! the selected display budget, never on source size.
use crate::bounded_json::encode_capped_bytes;
#[cfg(test)]
use crate::observation::capture::{record_capture_work, record_item_work};
use serde::{Serialize, de::DeserializeOwned};

pub const CHECKPOINT_TARGET_BYTES: usize = 786_432;
pub const CHECKPOINT_PAGE_ITEMS: usize = 32;
pub const CHECKPOINT_ITEM_BYTES: usize = 16_384;
struct Length {
    bytes: usize,
    limit: usize,
}

/// The same native admission and exact JSON length as `bounded_bytes`, without
/// retaining a byte buffer when the caller only needs its length.
pub(crate) fn bounded_len<T: Serialize + ?Sized>(value: &T, limit: usize) -> Option<usize> {
    if !crate::observation::preflight::fits(value, limit) {
        return None;
    }
    encoded_len_admitted(value, limit)
}

/// Exact bounded encoding length after the caller has proved admission.
pub(crate) fn encoded_len_admitted<T: Serialize + ?Sized>(
    value: &T,
    limit: usize,
) -> Option<usize> {
    let mut length = Length { bytes: 0, limit };
    serde_json::to_writer(&mut length, value).ok()?;
    Some(length.bytes)
}

/// The exact structural wire length when the caller retains the typed value,
/// not its encoded bytes. A positive raw admission avoids repeating the schema
/// walk; wide native quantities and uncertain keys keep structural admission.
/// The final schema-aware serializer still validates every encoded value.
#[cfg(test)]
pub(crate) fn bounded_structural_len<T: Serialize + schemars::JsonSchema>(
    value: &T,
    limit: usize,
) -> Option<usize> {
    bounded_structural_len_with_budget(value, limit, None)
}

/// A capped serializer bounds both encoded length and buffer capacity.
/// The allocation depends only on the selected display budget, not source size.
pub(crate) fn bounded_bytes<T: Serialize + ?Sized>(value: &T, limit: usize) -> Option<Vec<u8>> {
    if !crate::observation::preflight::fits(value, limit) {
        return None;
    }
    encode_capped_bytes(value, limit).ok()
}
/// Decodes one source-typed native domain value, preserving exact native u64 fields.
/// Opaque JSON fields retain their original JSON semantics.
/// # Errors
/// Returns the native DTO's validation or shape error.
pub fn decode_domain_value<T: serde::de::DeserializeOwned + schemars::JsonSchema>(
    value: serde_json::Value,
) -> serde_json::Result<T> {
    crate::observation::structural_json::decode(value)
}
pub(crate) fn structural_value<T: serde::Serialize + schemars::JsonSchema>(
    value: &T,
) -> Option<serde_json::Value> {
    let encoded = crate::observation::structural_json::encoded(value);
    bounded_item(&encoded, CHECKPOINT_ITEM_BYTES).map(|(_, value)| value)
}
pub(crate) fn bounded_item_bytes(value: &impl Serialize, limit: usize) -> Option<Vec<u8>> {
    bounded_item(value, limit).map(|(bytes, _)| bytes)
}
/// An assembled admission certificate avoids another traversal, but never
/// replaces exact schema-aware length or native quantity validation.
pub(crate) fn bounded_structural_len_with_budget<T: Serialize + schemars::JsonSchema>(
    value: &T,
    limit: usize,
    prepared_budget: Option<usize>,
) -> Option<usize> {
    let encoded = crate::observation::structural_json::encoded(value);
    if prepared_budget.is_none_or(|budget| budget > limit) {
        #[cfg(test)]
        record_capture_work(5);
        if crate::observation::preflight::bounded_safe_integer_fit(value, limit) != Some(true)
            && !crate::observation::preflight::fits(&encoded, limit)
        {
            return None;
        }
    }
    let mut length = Length { bytes: 0, limit };
    serde_json::to_writer(&mut length, &encoded).ok()?;
    Some(length.bytes)
}

// The caller just admitted this same immutable value. Only that repeated walk
// is omitted: the capped writer, JSON parse and every container limit remain.
pub(crate) fn bounded_item_admitted(
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
    #[cfg(test)]
    record_item_work(0);
    let bytes = encode_capped_bytes(value, limit).ok()?;
    #[cfg(test)]
    record_item_work(1);
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    bounded(&value).then_some((bytes, value))
}

// Admission already parses the capped bytes to check every container's item
// count. Callers needing JSON retain that same value instead of parsing twice.
pub(crate) fn bounded_item(
    value: &impl Serialize,
    limit: usize,
) -> Option<(Vec<u8>, serde_json::Value)> {
    if !crate::observation::preflight::fits(value, limit) {
        return None;
    }
    bounded_item_admitted(value, limit)
}
/// One immutable DTO's existing admission and normalization, prepared once by
/// its bounded native header owner. Both thresholds preserve later admission.
pub(crate) fn prepare_capture_item<T: Serialize + DeserializeOwned>(
    item: &T,
) -> Option<(T, usize, usize)> {
    let required = crate::observation::preflight::required_budget(item, CHECKPOINT_ITEM_BYTES)?;
    let (bytes, value) = bounded_item_admitted(item, CHECKPOINT_ITEM_BYTES)?;
    let encoded = bytes.len();
    Some((
        serde_json::from_value(value).ok()?,
        encoded,
        required.max(encoded),
    ))
}
pub(crate) fn bounded_clone<T: Serialize + DeserializeOwned>(value: &T, limit: usize) -> Option<T> {
    let bytes = bounded_item_bytes(value, limit)?;
    #[cfg(test)]
    record_item_work(2);
    serde_json::from_slice(&bytes).ok()
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

impl std::io::Write for Length {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes) {
            return Err(std::io::Error::other("bounded display item"));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t20_checkpoint_encoder_preserves_exact_bytes_at_small_budgets() {
        let value = "\n\"\\\t水";
        let expected = serde_json::to_vec(value).unwrap();
        let bytes = bounded_bytes(value, expected.len()).unwrap();
        assert_eq!(bytes, expected);
        assert!(bytes.capacity() <= expected.len());
        assert!(bounded_bytes(value, expected.len() - 1).is_none());
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
