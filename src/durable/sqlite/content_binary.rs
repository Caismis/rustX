//! Frozen canonical artifact evidence. Native settlement excludes native writers;
//! chunk digests additionally detect external changes without a whole-file read.
use super::{
    Connection, ConversationStoreError, OptionalExtension, SEGMENT, SEGMENT_BYTES, invalid, params,
    storage,
};
use crate::runtime::identity::ArtifactId;
use crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX;
use crate::tools::artifacts::ArtifactStore;
use sha2::{Digest, Sha256};
use std::io::{Read as _, Seek as _};

const SOURCES_SQL: &str = "CREATE TABLE IF NOT EXISTS canonical_binary_sources (
    source_id TEXT NOT NULL, block_index INTEGER NOT NULL,
    artifact_id TEXT NOT NULL CHECK(typeof(artifact_id)='text' AND octet_length(artifact_id)<=256),
    readable_end INTEGER NOT NULL CHECK(typeof(readable_end)='integer' AND readable_end>=0 AND readable_end<=9007199254740991),
    PRIMARY KEY(source_id,block_index)
) WITHOUT ROWID";
const SEGMENTS_SQL: &str = "CREATE TABLE IF NOT EXISTS canonical_binary_segments (
    source_id TEXT NOT NULL, block_index INTEGER NOT NULL,
    segment INTEGER NOT NULL CHECK(typeof(segment)='integer' AND segment>=0),
    digest BLOB NOT NULL CHECK(typeof(digest)='blob' AND length(digest)=32),
    PRIMARY KEY(source_id,block_index,segment),
    FOREIGN KEY(source_id,block_index) REFERENCES canonical_binary_sources(source_id,block_index)
) WITHOUT ROWID";

pub(in crate::durable::sqlite) fn create_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    for sql in [SOURCES_SQL, SEGMENTS_SQL] {
        connection
            .execute_batch(sql)
            .map_err(|error| storage(error.to_string()))?;
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn verify_schema(
    connection: &Connection,
) -> Result<(), ConversationStoreError> {
    for (table, expected) in [
        ("canonical_binary_sources", SOURCES_SQL),
        ("canonical_binary_segments", SEGMENTS_SQL),
    ] {
        let actual: Option<String> = connection.query_row(
            "SELECT CASE WHEN typeof(sql)='text' AND octet_length(sql)<=8192 THEN sql END FROM sqlite_schema WHERE type='table' AND name=?1",
            [table], |row| row.get(0),
        ).optional().map_err(|error| storage(error.to_string()))?.flatten();
        if actual.as_deref() != Some(expected.replace(" IF NOT EXISTS", "").as_str()) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing or invalid binary content table {table}"
            )));
        }
    }
    Ok(())
}

pub(super) fn index(
    connection: &Connection,
    id: &str,
    block: u32,
    artifact: &ArtifactId,
) -> Result<(), ConversationStoreError> {
    let Some(root) = root(connection) else {
        return Ok(());
    };
    let Ok(mut reader) = ArtifactStore::open_archive_reader(root, artifact) else {
        return Ok(());
    };
    if reader.len > CONTENT_SAFE_INTEGER_MAX {
        return Err(invalid());
    }
    connection.execute("INSERT INTO canonical_binary_sources(source_id,block_index,artifact_id,readable_end) VALUES(?1,?2,?3,?4)", params![id,block,artifact.as_str(),reader.len]).map_err(|error|storage(error.to_string()))?;
    // One initial streaming pass, bounded scratch. Publication of the frontier
    // follows all digests inside the caller's optional-layout savepoint.
    let mut buffer = vec![0; SEGMENT_BYTES];
    let mut offset = 0;
    let mut verification = reader.verification();
    let mut statement = connection.prepare_cached("INSERT INTO canonical_binary_segments(source_id,block_index,segment,digest) VALUES(?1,?2,?3,?4)").map_err(|error|storage(error.to_string()))?;
    while offset < reader.len {
        let length = usize::try_from((reader.len - offset).min(SEGMENT)).map_err(|_| invalid())?;
        reader
            .read_exact(&mut buffer[..length])
            .map_err(|_| invalid())?;
        let digest = reader
            .verify_chunk(&mut verification, offset, &buffer[..length])
            .map_err(|_| invalid())?;
        statement
            .execute(params![id, block, offset / SEGMENT, digest.as_slice()])
            .map_err(|error| storage(error.to_string()))?;
        offset += u64::try_from(length).map_err(|_| invalid())?;
    }
    reader
        .finish_verification(verification)
        .map_err(|_| invalid())?;
    connection
        .execute(
            "INSERT INTO content_frontiers(source_kind,source_id,block_index,representation,revision,readable_end) VALUES('canonical',?1,?2,'binary_bytes',1,?3)",
            params![id, block, reader.len],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}

fn root(connection: &Connection) -> Option<&std::path::Path> {
    connection
        .path()
        .filter(|path| !path.is_empty())
        .and_then(|path| std::path::Path::new(path).parent())
}

pub(super) fn range(
    connection: &Connection,
    key: &str,
    block: u32,
    offset: u64,
    end: u64,
    frontier: u64,
) -> Result<Vec<u8>, ConversationStoreError> {
    #[cfg(test)]
    HASHED_BYTES.with(|bytes| bytes.set(0));
    if offset > end || end > frontier || end - offset > SEGMENT {
        return Err(invalid());
    }
    let (artifact,length):(Option<String>,u64)=connection.query_row(
        "SELECT CASE WHEN typeof(artifact_id)='text' AND octet_length(artifact_id)<=256 THEN artifact_id END,readable_end FROM canonical_binary_sources WHERE source_id=?1 AND block_index=?2",
        params![key,block],|row|Ok((row.get(0)?,row.get(1)?)),
    ).optional().map_err(|error|storage(error.to_string()))?.ok_or_else(invalid)?;
    if length != frontier {
        return Err(invalid());
    }
    let artifact = ArtifactId::new(artifact.ok_or_else(invalid)?);
    let mut reader =
        ArtifactStore::open_archive_reader(root(connection).ok_or_else(invalid)?, &artifact)
            .map_err(|_| invalid())?;
    if reader.len != frontier {
        return Err(invalid());
    }
    if offset == end {
        return Ok(Vec::new());
    }
    let mut result = Vec::with_capacity(usize::try_from(end - offset).map_err(|_| invalid())?);
    let mut buffer = vec![0; SEGMENT_BYTES];
    let mut start = offset / SEGMENT * SEGMENT;
    while start < end {
        let length = usize::try_from((frontier - start).min(SEGMENT)).map_err(|_| invalid())?;
        let expected: Option<Vec<u8>> = connection.query_row(
            "SELECT CASE WHEN typeof(digest)='blob' AND length(digest)=32 THEN digest END FROM canonical_binary_segments WHERE source_id=?1 AND block_index=?2 AND segment=?3",
            params![key,block,start/SEGMENT],|row|row.get(0),
        ).optional().map_err(|error|storage(error.to_string()))?.flatten();
        let expected = expected.ok_or_else(invalid)?;
        reader
            .seek(std::io::SeekFrom::Start(start))
            .map_err(|_| invalid())?;
        #[cfg(test)]
        super::binary_tests::before_chunk_read();
        read_chunk(&mut reader, &mut buffer[..length])?;
        let actual = Sha256::digest(&buffer[..length]);
        #[cfg(test)]
        HASHED_BYTES.with(|bytes| bytes.set(bytes.get() + length));
        if actual.as_slice() != expected {
            return Err(invalid());
        }
        #[cfg(test)]
        super::binary_tests::after_chunk_verified();
        let from = usize::try_from(offset.saturating_sub(start)).map_err(|_| invalid())?;
        let to = usize::try_from((end - start).min(u64::try_from(length).map_err(|_| invalid())?))
            .map_err(|_| invalid())?;
        result.extend_from_slice(&buffer[from..to]);
        start += u64::try_from(length).map_err(|_| invalid())?;
    }
    Ok(result)
}

#[cfg(test)]
thread_local! { static HASHED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(super) fn hashed_bytes() -> usize {
    HASHED_BYTES.with(std::cell::Cell::get)
}

// Account actual bytes even when a shortened/corrupted file stops mid-chunk.
fn read_chunk(
    reader: &mut impl std::io::Read,
    bytes: &mut [u8],
) -> Result<(), ConversationStoreError> {
    #[cfg(test)]
    super::LAST_READ_WORK.with(|work| {
        let (segments, bytes) = work.get();
        work.set((segments + 1, bytes));
    });
    let mut filled = 0;
    while filled < bytes.len() {
        let count = reader.read(&mut bytes[filled..]).map_err(|_| invalid())?;
        if count == 0 {
            return Err(invalid());
        }
        filled += count;
        #[cfg(test)]
        super::LAST_READ_WORK.with(|work| {
            let (segments, bytes) = work.get();
            work.set((segments, bytes + count));
        });
    }
    Ok(())
}
