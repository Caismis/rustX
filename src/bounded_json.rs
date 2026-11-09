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
