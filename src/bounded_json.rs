//! Shared allocation-capped JSON encoding for bounded native values.

use serde::Serialize;

struct Buffer {
    bytes: Vec<u8>,
    limit: usize,
}
impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("bounded JSON encoding"));
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
/// Exact JSON encoding with a hard byte and buffer-capacity limit.
pub(crate) fn encode_capped_bytes<T: Serialize + ?Sized>(
    value: &T,
    limit: usize,
) -> serde_json::Result<Vec<u8>> {
    let mut buffer = Buffer {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut buffer, value)?;
    Ok(buffer.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn t20_checkpoint_encoder_capacity_obeys_its_physical_byte_budget() {
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
    fn small_limits_bound_allocation_and_reject_the_next_byte() {
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
}
