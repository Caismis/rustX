//! Derived byte layout construction uses one 64 KiB output buffer.
use super::{Connection, ConversationStoreError, params, storage};
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
    let mut writer = SegmentWriter {
        connection,
        key,
        revision,
        publication_metadata,
        buffer: Vec::with_capacity(65_536),
        written: 0,
        segment: 0,
    };
    crate::runtime::read_layout::canonical::to_writer(&mut writer, value)
        .map_err(|error| storage(error.to_string()))?;
    writer.flush().map_err(|error| storage(error.to_string()))?;
    let length = writer.written;
    if revision > CONTENT_SAFE_INTEGER_MAX {
        return Err(storage("content revision exhausted"));
    }
    connection
        .execute(
            "INSERT INTO content_frontiers VALUES(?1,?2,?3,?4,?5,?6)",
            params![key.0, key.1, key.2, key.3.key(), revision, length],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(length)
}
pub(super) fn put_native<T: Serialize + schemars::JsonSchema>(
    connection: &Connection,
    key: (&str, &str, u32, ContentRepresentation),
    revision: u64,
    value: &T,
) -> Result<u64, ConversationStoreError> {
    let mut writer = SegmentWriter {
        connection,
        key,
        revision,
        publication_metadata: false,
        buffer: Vec::with_capacity(65_536),
        written: 0,
        segment: 0,
    };
    crate::runtime_client::structural_json::to_writer(&mut writer, value)
        .map_err(|error| storage(error.to_string()))?;
    writer.flush().map_err(|error| storage(error.to_string()))?;
    let length = writer.written;
    if revision > CONTENT_SAFE_INTEGER_MAX {
        return Err(storage("content revision exhausted"));
    }
    connection
        .execute(
            "INSERT INTO content_frontiers VALUES(?1,?2,?3,?4,?5,?6)",
            params![key.0, key.1, key.2, key.3.key(), revision, length],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(length)
}
struct SegmentWriter<'a> {
    connection: &'a Connection,
    key: (&'a str, &'a str, u32, ContentRepresentation),
    revision: u64,
    publication_metadata: bool,
    buffer: Vec<u8>,
    written: u64,
    segment: u64,
}
impl std::io::Write for SegmentWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
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
            self.connection.execute(
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
