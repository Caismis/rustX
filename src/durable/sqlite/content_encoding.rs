//! Derived byte layouts retain the immutable first chunk beside its frontier.
//! Encoding uses at most two 64 KiB buffers, independent of payload size.
use super::{Connection, ConversationStoreError, params, storage};
use crate::durable::sqlite::statements::FixedStatements;
use crate::runtime_client::content::{CONTENT_SAFE_INTEGER_MAX, ContentRepresentation};
use serde::Serialize;
use std::io::Write as _;

pub(super) fn put_json<T: Serialize + ?Sized>(
    connection: &Connection,
    key: (&str, &str, u32, ContentRepresentation),
    revision: u64,
    value: &T,
    publication_metadata: bool,
) -> Result<u64, ConversationStoreError> {
    let mut writer = SegmentWriter::new(connection, key, revision, publication_metadata);
    crate::runtime::read_layout::canonical::to_writer(&mut writer, value)
        .map_err(|error| storage(error.to_string()))?;
    writer.finish()
}

pub(super) fn put_native<T: Serialize + schemars::JsonSchema>(
    connection: &Connection,
    key: (&str, &str, u32, ContentRepresentation),
    revision: u64,
    value: &T,
) -> Result<u64, ConversationStoreError> {
    let mut writer = SegmentWriter::new(connection, key, revision, false);
    crate::runtime_client::structural_json::to_writer(&mut writer, value)
        .map_err(|error| storage(error.to_string()))?;
    writer.finish()
}

/// Live prefixes retain each revision separately. Raw Tool arguments also keep
/// their independent bytes: a missing frontier must still reveal retained raw
/// evidence instead of silently selecting reserialized canonical arguments.
pub(super) fn inline_first(kind: &str, representation: ContentRepresentation) -> bool {
    !matches!(kind, "live" | "live_block_metadata")
        && matches!(
            representation,
            ContentRepresentation::Utf8Text | ContentRepresentation::CanonicalJsonV1
        )
}

struct SegmentWriter<'a> {
    connection: &'a Connection,
    key: (&'a str, &'a str, u32, ContentRepresentation),
    revision: u64,
    publication_metadata: bool,
    buffer: Vec<u8>,
    first: Option<Vec<u8>>,
    written: u64,
    segment: u64,
}
impl<'a> SegmentWriter<'a> {
    fn new(
        connection: &'a Connection,
        key: (&'a str, &'a str, u32, ContentRepresentation),
        revision: u64,
        publication_metadata: bool,
    ) -> Self {
        Self {
            connection,
            key,
            revision,
            publication_metadata,
            buffer: Vec::with_capacity(65_536),
            first: None,
            written: 0,
            segment: 0,
        }
    }
    fn finish(mut self) -> Result<u64, ConversationStoreError> {
        self.flush().map_err(|error| storage(error.to_string()))?;
        if self.revision > CONTENT_SAFE_INTEGER_MAX {
            return Err(storage("content revision exhausted"));
        }
        // Empty immutable text still has an explicit, readable zero-byte body.
        let first = if inline_first(self.key.0, self.key.3) {
            Some(self.first.as_deref().unwrap_or_default())
        } else {
            None
        };
        self.connection.execute_cached(
            "INSERT INTO content_frontiers(source_kind,source_id,block_index,representation,revision,readable_end,bytes) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![self.key.0, self.key.1, self.key.2, self.key.3.key(), self.revision, self.written, first],
        ).map_err(|error| storage(error.to_string()))?;
        Ok(self.written)
    }
}
impl std::io::Write for SegmentWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if !bytes.is_empty() && self.buffer.capacity() == 0 {
            // The first chunk keeps its allocation. Allocate the overflow
            // buffer only when needed, with no amortized growth past 64 KiB.
            self.buffer.reserve_exact(65_536);
        }
        let count = bytes.len().min(65_536 - self.buffer.len());
        self.written = self
            .written
            .checked_add(count as u64)
            .filter(|end| *end <= CONTENT_SAFE_INTEGER_MAX)
            .ok_or_else(|| std::io::Error::other("content length exhausted"))?;
        self.buffer.extend_from_slice(&bytes[..count]);
        if self.buffer.len() == 65_536 {
            self.flush()?;
        }
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        if self.segment == 0 && inline_first(self.key.0, self.key.3) {
            self.first = Some(std::mem::take(&mut self.buffer));
            self.segment = 1;
            return Ok(());
        }
        if self.publication_metadata {
            self.connection.execute(
                "INSERT INTO publication_metadata_segments VALUES(?1,?2,?3,?4,?5)",
                params![
                    self.key.1,
                    self.key.2,
                    self.revision,
                    self.segment,
                    &self.buffer
                ],
            )
        } else {
            self.connection.execute_cached(
                "INSERT INTO content_segments VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    self.key.0,
                    self.key.1,
                    self.key.2,
                    self.key.3.key(),
                    self.segment,
                    &self.buffer
                ],
            )
        }
        .map_err(std::io::Error::other)?;
        self.buffer.clear();
        self.segment += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_encoder_lazily_allocates_a_bounded_second_buffer() {
        let mut connection = Connection::open_in_memory().unwrap();
        super::super::create_schema(&mut connection).unwrap();
        let mut small = SegmentWriter::new(
            &connection,
            ("canonical", "small", 0, ContentRepresentation::Utf8Text),
            1,
            false,
        );
        small.write_all(b"one").unwrap();
        small.flush().unwrap();
        assert_eq!(small.first.as_ref().unwrap().capacity(), 65_536);
        assert_eq!(
            small.buffer.capacity(),
            0,
            "no overflow allocation for small content"
        );
        small.finish().unwrap();

        let mut writer = SegmentWriter::new(
            &connection,
            (
                "canonical",
                "fragmented",
                0,
                ContentRepresentation::Utf8Text,
            ),
            1,
            false,
        );
        writer.write_all(&vec![b'a'; 65_536]).unwrap();
        assert_eq!(writer.buffer.capacity(), 0);
        for _ in 0..12_000 {
            // Eleven-byte writes would grow a zero-capacity Vec past 64 KiB.
            writer.write_all(b"01234567890").unwrap();
            assert!(writer.buffer.capacity() <= 65_536);
            assert!(writer.first.as_ref().unwrap().capacity() <= 65_536);
        }
        assert_eq!(writer.finish().unwrap(), 65_536 + 12_000 * 11);
    }
}
