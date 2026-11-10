//! Authenticated bounded pages inside one immutable observation fragment.
//! This index owns bytes/field facts only; native manifest nodes own selection.
use super::{Connection, ConversationStoreError, params, storage};
use crate::durable::sqlite::statements::FixedStatements;
use rusqlite::OptionalExtension as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub(super) const PAGE_BYTES: usize = 65_536;
const FANOUT: usize = 64;
#[path = "page_codec.rs"]
mod codec;
const MAX_DEPTH: usize = 16;
pub(super) type Hash = [u8; 32];
/// A numeric location plus independent immutable-byte authentication.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BlobRef {
    pub(super) id: u64,
    pub(super) hash: Hash,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Reference {
    pub(super) id: u64,
    pub(super) hash: Hash,
    pub(super) last: String,
    pub(super) depth: u8,
}
#[cfg_attr(test, derive(Serialize, Deserialize))]
#[cfg_attr(test, serde(tag = "type", rename_all = "snake_case"))]
enum Page {
    Leaf { entries: Vec<(String, Vec<u8>)> },
    Branch { children: Vec<Reference> },
}
// A bounded connection-local memo is an optimization, never byte authority.
// SQLite rolls it back with page writes/savepoints; hits authenticate current bytes.
pub(super) fn install_cache(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection.execute_batch("CREATE TEMP TABLE IF NOT EXISTS request_source_page_memo(stamp INTEGER PRIMARY KEY,hash BLOB NOT NULL UNIQUE,page_id INTEGER NOT NULL)")
        .map_err(|error|storage(error.to_string()))
}
fn remember(connection: &Connection, reference: BlobRef) -> Result<(), ConversationStoreError> {
    connection
        .execute_cached(
            "DELETE FROM request_source_page_memo WHERE hash=?1",
            [reference.hash.as_slice()],
        )
        .map_err(|error| storage(error.to_string()))?;
    connection
        .execute_cached(
            "INSERT INTO request_source_page_memo(hash,page_id)VALUES(?1,?2)",
            params![reference.hash.as_slice(), reference.id],
        )
        .map_err(|error| storage(error.to_string()))?;
    connection.execute_cached("DELETE FROM request_source_page_memo WHERE stamp=(SELECT min(stamp) FROM request_source_page_memo) AND (SELECT count(*) FROM request_source_page_memo)>256",[]).map_err(|error|storage(error.to_string()))?;
    Ok(())
}
fn encode(page: &Page) -> Result<Vec<u8>, ConversationStoreError> {
    codec::encode(page)
}
fn persist(connection: &Connection, page: &Page) -> Result<Reference, ConversationStoreError> {
    let (last, depth) = match page {
        Page::Leaf { entries } => (entries.last().map(|entry| &entry.0), 0),
        Page::Branch { children } => (
            children.last().map(|child| &child.last),
            children
                .first()
                .map_or(0, |child| child.depth.saturating_add(1)),
        ),
    };
    let last = last
        .ok_or_else(|| storage("empty fragment index page"))?
        .clone();
    if last.len() > 4160 || usize::from(depth) >= MAX_DEPTH {
        return Err(storage("fragment index geometry exceeds bound"));
    }
    let bytes = encode(page)?;
    let blob = store_blob(connection, &bytes)?;
    Ok(Reference {
        id: blob.id,
        hash: blob.hash,
        last,
        depth,
    })
}
pub(super) fn store_blob(
    connection: &Connection,
    bytes: &[u8],
) -> Result<BlobRef, ConversationStoreError> {
    if bytes.len() > PAGE_BYTES {
        return Err(storage("fragment metadata exceeds bound"));
    }
    let hash: Hash = Sha256::digest(bytes).into();
    let cached: Option<u64> = connection
        .query_row_cached(
            "SELECT page_id FROM request_source_page_memo WHERE hash=?1",
            [hash.as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    if let Some(id) = cached {
        let reference = BlobRef { id, hash };
        load_blob_reusing(connection, reference, Some(bytes))?;
        remember(connection, reference)?;
        return Ok(reference);
    }
    connection
        .prepare_cached("INSERT INTO request_source_pages(hash,bytes) VALUES(?1,?2)")
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?
        .execute(params![hash.as_slice(), bytes])
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    let reference = BlobRef {
        id: super::super::super::nonnegative(connection.last_insert_rowid(), "Source page id")?,
        hash,
    };
    remember(connection, reference)?;
    Ok(reference)
}
#[cfg(test)]
pub(super) fn load_blob(
    connection: &Connection,
    digest: BlobRef,
) -> Result<Vec<u8>, ConversationStoreError> {
    load_blob_reusing(connection, digest, None).map(|(bytes, _)| bytes)
}
/// A freshly encoded template already has its digest. Reuse that computation
/// only after reading and comparing the bytes at this numeric location,
/// including any changes made by an insert trigger.
pub(super) fn load_blob_reusing(
    connection: &Connection,
    digest: BlobRef,
    encoded: Option<&[u8]>,
) -> Result<(Vec<u8>, bool), ConversationStoreError> {
    let bytes:Option<Vec<u8>>=connection.prepare_cached("SELECT CASE WHEN typeof(bytes)='blob' AND length(bytes)<=65536 THEN bytes END FROM request_source_pages WHERE id=?1 AND hash=?2").map_err(|error|crate::durable::sqlite::sqlite_error(&error,error.to_string()))?.query_row(params![digest.id, digest.hash.as_slice()],|row|row.get(0)).optional().map_err(|error|crate::durable::sqlite::sqlite_error(&error,error.to_string()))?.flatten();
    let bytes = bytes.ok_or_else(|| storage("original fragment metadata unavailable"))?;
    #[cfg(test)]
    super::record_index(bytes.len());
    let reused = encoded == Some(bytes.as_slice());
    if !reused && Hash::from(Sha256::digest(&bytes)) != digest.hash {
        return Err(storage("original fragment metadata digest differs"));
    }
    Ok((bytes, reused))
}

fn load(connection: &Connection, reference: &Reference) -> Result<Page, ConversationStoreError> {
    if usize::from(reference.depth) >= MAX_DEPTH || reference.last.len() > 4160 {
        return Err(storage("fragment page reference exceeds bound"));
    }
    let bytes: Option<Vec<u8>> = connection.prepare_cached(
        "SELECT CASE WHEN typeof(bytes)='blob' AND length(bytes)<=65536 THEN bytes END FROM request_source_pages WHERE id=?1 AND hash=?2",
    ).map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?.query_row(
        params![reference.id, reference.hash.as_slice()], |row| row.get(0),
    ).optional().map_err(|error| crate::durable::sqlite::sqlite_error(&error,error.to_string()))?.flatten();
    let bytes = bytes.ok_or_else(|| storage("original fragment page unavailable"))?;
    #[cfg(test)]
    super::record_index(bytes.len());
    if Hash::from(Sha256::digest(&bytes)) != reference.hash {
        return Err(storage("original fragment page digest differs"));
    }
    let page = codec::decode(&bytes)?;
    let valid = match &page {
        Page::Leaf { entries } => {
            reference.depth == 0
                && !entries.is_empty()
                && entries.len() <= FANOUT
                && entries
                    .iter()
                    .all(|(key, value)| key.len() <= 4160 && value.len() <= 40_960)
                && entries.windows(2).all(|pair| pair[0].0 < pair[1].0)
                && entries
                    .last()
                    .is_some_and(|entry| entry.0 == reference.last)
        }
        Page::Branch { children } => {
            reference.depth > 0
                && !children.is_empty()
                && children.len() <= FANOUT
                && children.iter().all(|child| {
                    child.depth.checked_add(1) == Some(reference.depth) && child.last.len() <= 4160
                })
                && children.windows(2).all(|pair| pair[0].last < pair[1].last)
                && children
                    .last()
                    .is_some_and(|child| child.last == reference.last)
        }
    };
    if !valid {
        return Err(storage("fragment page evidence differs"));
    }
    Ok(page)
}
/// Builder consumes ordered facts from the bounded memory/disk spool.
/// It retains one byte/count-bounded leaf and bounded references per level.
pub(super) struct Builder<'a> {
    connection: &'a Connection,
    entries: Vec<(String, Vec<u8>)>,
    levels: Vec<Level>,
    previous: Option<String>,
    entry_bytes: usize,
}
struct Level {
    children: Vec<Reference>,
    size: usize,
}
impl<'a> Builder<'a> {
    pub(super) fn new(connection: &'a Connection) -> Self {
        Self {
            connection,
            entries: Vec::new(),
            levels: Vec::new(),
            previous: None,
            entry_bytes: codec::HEADER,
        }
    }
    pub(super) fn push(
        &mut self,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), ConversationStoreError> {
        if key.len() > 4160
            || value.len() > 40_960
            || self
                .previous
                .as_ref()
                .is_some_and(|previous| previous >= &key)
        {
            return Err(storage("fragment fact exceeds bound or repeats"));
        }
        let size = codec::leaf_size(&key, &value);
        if !self.entries.is_empty()
            && (self.entries.len() == FANOUT || self.entry_bytes.saturating_add(size) > PAGE_BYTES)
        {
            self.flush()?;
        }
        self.previous = Some(key.clone());
        self.entry_bytes = self.entry_bytes.saturating_add(size);
        self.entries.push((key, value));
        Ok(())
    }
    fn flush(&mut self) -> Result<(), ConversationStoreError> {
        if !self.entries.is_empty() {
            let reference = persist(
                self.connection,
                &Page::Leaf {
                    entries: std::mem::take(&mut self.entries),
                },
            )?;
            self.entry_bytes = codec::HEADER;
            self.admit(0, reference)?;
        }
        Ok(())
    }
    fn admit(&mut self, level: usize, reference: Reference) -> Result<(), ConversationStoreError> {
        if level >= MAX_DEPTH {
            return Err(storage("fragment tree exceeds bounded depth"));
        }
        if self.levels.len() == level {
            self.levels.push(Level {
                children: Vec::new(),
                size: codec::HEADER,
            });
        }
        let size = codec::branch_size(&reference);
        if !self.levels[level].children.is_empty()
            && (self.levels[level].children.len() == FANOUT
                || self.levels[level].size.saturating_add(size) > PAGE_BYTES)
        {
            let parent = persist(
                self.connection,
                &Page::Branch {
                    children: std::mem::take(&mut self.levels[level].children),
                },
            )?;
            self.levels[level].size = codec::HEADER;
            self.admit(level + 1, parent)?;
        }
        self.levels[level].size = self.levels[level].size.saturating_add(size);
        self.levels[level].children.push(reference);
        Ok(())
    }
    pub(super) fn finish(mut self) -> Result<Option<Reference>, ConversationStoreError> {
        self.flush()?;
        let mut level = 0;
        while level < self.levels.len() {
            if !self.levels[level].children.is_empty() {
                if level + 1 == self.levels.len() && self.levels[level].children.len() == 1 {
                    return Ok(self.levels[level].children.pop());
                }
                let parent = persist(
                    self.connection,
                    &Page::Branch {
                        children: std::mem::take(&mut self.levels[level].children),
                    },
                )?;
                self.levels[level].size = codec::HEADER;
                self.admit(level + 1, parent)?;
            }
            level += 1;
        }
        Ok(None)
    }
}
pub(super) fn get(
    connection: &Connection,
    root: &Reference,
    key: &str,
) -> Result<Option<Vec<u8>>, ConversationStoreError> {
    let mut reference = root.clone();
    for _ in 0..MAX_DEPTH {
        match load(connection, &reference)? {
            Page::Leaf { entries } => {
                return Ok(entries
                    .into_iter()
                    .find(|entry| entry.0 == key)
                    .map(|entry| entry.1));
            }
            Page::Branch { children } => {
                let Some(child) = children
                    .into_iter()
                    .find(|child| key <= child.last.as_str())
                else {
                    return Ok(None);
                };
                reference = child;
            }
        }
    }
    Err(storage("fragment lookup exceeds bounded depth"))
}

/// One operation's at most nine adjacent byte chunks share each authenticated
/// ancestor and leaf. Nothing is retained across calls or storage snapshots.
pub(super) fn get_many(
    connection: &Connection,
    root: &Reference,
    keys: &[String],
) -> Result<Vec<Option<Vec<u8>>>, ConversationStoreError> {
    if keys.is_empty()
        || keys.len() > 9
        || keys.iter().any(|key| key.len() > 4160)
        || !keys.windows(2).all(|pair| pair[0] < pair[1])
    {
        return Err(storage("fragment batch keys exceed bound or repeat"));
    }
    let mut output = vec![None; keys.len()];
    select_many(connection, root, keys, &mut output)?;
    Ok(output)
}

fn select_many(
    connection: &Connection,
    reference: &Reference,
    keys: &[String],
    output: &mut [Option<Vec<u8>>],
) -> Result<(), ConversationStoreError> {
    match load(connection, reference)? {
        Page::Leaf { entries } => {
            for (key, value) in entries {
                if let Ok(at) = keys.binary_search(&key) {
                    output[at] = Some(value);
                }
            }
        }
        Page::Branch { children } => {
            let mut start = 0;
            for child in children {
                let end = start + keys[start..].partition_point(|key| key <= &child.last);
                if end > start {
                    select_many(
                        connection,
                        &child,
                        &keys[start..end],
                        &mut output[start..end],
                    )?;
                }
                start = end;
                if start == keys.len() {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batched_chunk_reads_share_pages_and_revalidate_each_operation() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE request_source_pages(id INTEGER PRIMARY KEY,hash BLOB NOT NULL,bytes BLOB NOT NULL)",
            )
            .unwrap();
        install_cache(&connection).unwrap();
        let mut builder = Builder::new(&connection);
        for i in 0_u64..200 {
            builder
                .push(format!("{i:020}"), i.to_le_bytes().to_vec())
                .unwrap();
        }
        let root = builder.finish().unwrap().unwrap();
        let keys = [1, 2, 63, 64, 65, 127, 128, 199, 999].map(|i| format!("{i:020}"));
        super::super::take_work();
        let expected = keys
            .iter()
            .map(|key| get(&connection, &root, key).unwrap())
            .collect::<Vec<_>>();
        let individual = super::super::take_work().index_pages;
        assert_eq!(get_many(&connection, &root, &keys).unwrap(), expected);
        let batch = super::super::take_work().index_pages;
        assert_eq!(batch, 5);
        assert!(batch < individual);
        assert!(get_many(&connection, &root, &vec![String::new(); 10]).is_err());
        assert!(get_many(&connection, &root, &["b".into(), "a".into()]).is_err());
        connection.execute_batch("SAVEPOINT damage_batch").unwrap();
        connection
            .execute(
                "UPDATE request_source_pages SET bytes=x'00' WHERE id=?1 AND hash=?2",
                params![root.id, root.hash.as_slice()],
            )
            .unwrap();
        assert!(get_many(&connection, &root, &keys).is_err());
        connection
            .execute_batch("ROLLBACK TO damage_batch; RELEASE damage_batch")
            .unwrap();
        assert_eq!(get_many(&connection, &root, &keys).unwrap(), expected);
    }

    #[test]
    fn cached_fragment_writes_preserve_rollback_tamper_and_schema_failure() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE request_source_pages(id INTEGER PRIMARY KEY,hash BLOB NOT NULL,bytes BLOB NOT NULL)",
            )
            .unwrap();
        install_cache(&connection).unwrap();
        let digest = {
            let transaction = connection.transaction().unwrap();
            let digest = store_blob(&transaction, b"original").unwrap();
            assert_eq!(load_blob(&transaction, digest).unwrap(), b"original");
            transaction.rollback().unwrap();
            digest
        };
        assert!(
            load_blob(&connection, digest).is_err(),
            "a cached write cannot survive rollback"
        );
        {
            let transaction = connection.transaction().unwrap();
            assert_eq!(
                store_blob(&transaction, b"original").unwrap().hash,
                digest.hash
            );
            transaction.commit().unwrap();
        }
        assert_eq!(load_blob(&connection, digest).unwrap(), b"original");
        connection
            .execute(
                "UPDATE request_source_pages SET bytes=?1",
                [b"changed".as_slice()],
            )
            .unwrap();
        assert!(
            load_blob(&connection, digest).is_err(),
            "cached statements cannot bless changed Original bytes"
        );
        connection
            .execute_batch("DROP TABLE request_source_pages")
            .unwrap();
        assert!(
            store_blob(&connection, b"another").is_err(),
            "a warm statement must observe schema invalidation"
        );
    }
}

#[cfg(test)]
#[path = "page_codec_tests.rs"]
mod codec_tests;

// Test inspection/mutation uses typed pages, then the production binary codec.
// These helpers never act as a persisted JSON-page reader.
#[cfg(test)]
pub(super) fn inspect_page(bytes: &[u8]) -> serde_json::Value {
    serde_json::to_value(codec::decode(bytes).unwrap()).unwrap()
}
#[cfg(test)]
pub(super) fn encode_test_page(page: serde_json::Value) -> Vec<u8> {
    codec::encode(&serde_json::from_value(page).unwrap()).unwrap()
}

#[cfg(test)]
mod memo_tests {
    use super::*;

    #[test]
    fn bounded_memo_keeps_hot_pages_and_authenticates_each_hit_and_rollback() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE request_source_pages(id INTEGER PRIMARY KEY,hash BLOB NOT NULL,bytes BLOB NOT NULL)").unwrap();
        install_cache(&connection).unwrap();
        let hot = store_blob(&connection, b"hot immutable geometry").unwrap();
        for history in 0_u64..4096 {
            store_blob(&connection, &history.to_le_bytes()).unwrap();
            assert_eq!(
                store_blob(&connection, b"hot immutable geometry").unwrap(),
                hot
            );
            let count: u64 = connection
                .query_row("SELECT count(*) FROM request_source_page_memo", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert!(count <= 256, "H={history}");
        }
        {
            let transaction = connection.transaction().unwrap();
            store_blob(&transaction, b"rolled back geometry").unwrap();
        }
        let cached: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM request_source_page_memo WHERE hash=?1)",
                [Hash::from(Sha256::digest(b"rolled back geometry")).as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!cached);
        let before = connection.total_changes();
        connection
            .execute(
                "UPDATE request_source_pages SET bytes=x'00' WHERE id=?1",
                [hot.id],
            )
            .unwrap();
        assert!(store_blob(&connection, b"hot immutable geometry").is_err());
        assert_eq!(
            connection.total_changes(),
            before + 1,
            "a corrupt hit never appends replacement evidence"
        );
    }
}
