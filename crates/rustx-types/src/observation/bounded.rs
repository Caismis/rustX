//! Bounded serialization helpers shared by observation encoders.
use serde::Serialize;

/// Encode `value` as JSON when it fits `limit`, else `None`.
pub fn bounded_bytes<T: Serialize + ?Sized>(value: &T, limit: usize) -> Option<Vec<u8>> {
    if !crate::observation::preflight::fits(value, limit) {
        return None;
    }
    rustx_bounded_json::encode_capped_bytes(value, limit).ok()
}
