//! Authenticated bounded pages inside one immutable observation fragment.
//! This index owns bytes/field facts only; native manifest nodes own selection.
use super::{Connection, ConversationStoreError, params, storage};
use base64::Engine as _;
use rusqlite::OptionalExtension as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub(super) const PAGE_BYTES: usize = 65_536;
const FANOUT: usize = 4;
const MAX_DEPTH: usize = 16;
pub(super) type Hash = [u8; 32];
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Reference {
    pub(super) hash: Hash,
    pub(super) last: String,
    pub(super) depth: u8,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Page {
    Leaf { entries: Vec<(String, String)> },
    Branch { children: Vec<Reference> },
}
fn encode(page: &Page) -> Result<Vec<u8>, ConversationStoreError> {
    crate::runtime_client::checkpoint::bounded_bytes(page, PAGE_BYTES)
        .ok_or_else(|| storage("observation fragment page exceeds bound"))
}
fn persist(connection: &Connection, page: &Page) -> Result<Reference, ConversationStoreError> {
    let (last, depth) = match page {
        Page::Leaf { entries } => (entries.last().map(|entry| &entry.0), 0),
        Page::Branch { children } => (
            children.last().map(|child| &child.last),
            children.first().map_or(0, |child| child.depth + 1),
        ),
    };
    let last = last
        .ok_or_else(|| storage("empty fragment index page"))?
        .clone();
    if last.len() > 4160 || usize::from(depth) >= MAX_DEPTH {
        return Err(storage("fragment index geometry exceeds bound"));
    }
    let bytes = encode(page)?;
    let hash: Hash = Sha256::digest(&bytes).into();
    connection
        .prepare_cached("INSERT OR IGNORE INTO request_source_pages(hash,bytes) VALUES(?1,?2)")
        .map_err(|error| storage(error.to_string()))?
        .execute(params![hash.as_slice(), bytes])
        .map_err(|error| storage(error.to_string()))?;
    Ok(Reference { hash, last, depth })
}
pub(super) fn store_blob(
    connection: &Connection,
    bytes: &[u8],
) -> Result<Hash, ConversationStoreError> {
    if bytes.len() > PAGE_BYTES {
        return Err(storage("fragment metadata exceeds bound"));
    }
    let digest: Hash = Sha256::digest(bytes).into();
    connection
        .prepare_cached("INSERT OR IGNORE INTO request_source_pages(hash,bytes) VALUES(?1,?2)")
        .map_err(|error| storage(error.to_string()))?
        .execute(params![digest.as_slice(), bytes])
        .map_err(|error| storage(error.to_string()))?;
    Ok(digest)
}
pub(super) fn load_blob(
    connection: &Connection,
    digest: Hash,
) -> Result<Vec<u8>, ConversationStoreError> {
    let bytes:Option<Vec<u8>>=connection.prepare_cached("SELECT CASE WHEN typeof(bytes)='blob' AND length(bytes)<=65536 THEN bytes END FROM request_source_pages WHERE hash=?1").map_err(|error|storage(error.to_string()))?.query_row([digest.as_slice()],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?.flatten();
    let bytes = bytes.ok_or_else(|| storage("original fragment metadata unavailable"))?;
    #[cfg(test)]
    super::record_index(bytes.len());
    if Hash::from(Sha256::digest(&bytes)) != digest {
        return Err(storage("original fragment metadata digest differs"));
    }
    Ok(bytes)
}
fn load(connection: &Connection, reference: &Reference) -> Result<Page, ConversationStoreError> {
    if usize::from(reference.depth) >= MAX_DEPTH || reference.last.len() > 4160 {
        return Err(storage("fragment page reference exceeds bound"));
    }
    let bytes: Option<Vec<u8>> = connection.prepare_cached(
        "SELECT CASE WHEN typeof(bytes)='blob' AND length(bytes)<=65536 THEN bytes END FROM request_source_pages WHERE hash=?1",
    ).map_err(|error| storage(error.to_string()))?.query_row(
        [reference.hash.as_slice()], |row| row.get(0),
    ).optional().map_err(|error| storage(error.to_string()))?.flatten();
    let bytes = bytes.ok_or_else(|| storage("original fragment page unavailable"))?;
    #[cfg(test)]
    super::record_index(bytes.len());
    if Hash::from(Sha256::digest(&bytes)) != reference.hash {
        return Err(storage("original fragment page digest differs"));
    }
    let page: Page =
        serde_json::from_slice(&bytes).map_err(|_| storage("fragment page JSON unavailable"))?;
    let valid = match &page {
        Page::Leaf { entries } => {
            reference.depth == 0
                && !entries.is_empty()
                && entries.len() <= FANOUT
                && entries
                    .iter()
                    .all(|(key, value)| key.len() <= 4160 && value.len() <= 54_616)
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
/// Builder consumes lexicographically ordered facts from the producer's disk
/// spool. It retains at most one bounded leaf and FANOUT references per level.
pub(super) struct Builder<'a> {
    connection: &'a Connection,
    entries: Vec<(String, String)>,
    levels: Vec<Vec<Reference>>,
    previous: Option<String>,
}
impl<'a> Builder<'a> {
    pub(super) fn new(connection: &'a Connection) -> Self {
        Self {
            connection,
            entries: Vec::new(),
            levels: Vec::new(),
            previous: None,
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
        if !self.entries.is_empty() {
            let size = self
                .entries
                .iter()
                .map(|(key, value)| key.len() * 2 + value.len() + 32)
                .sum::<usize>();
            if self.entries.len() == FANOUT
                || size + key.len() * 2 + (value.len().div_ceil(3) * 4) + 32 > PAGE_BYTES - 1024
            {
                self.flush()?;
            }
        }
        self.previous = Some(key.clone());
        self.entries
            .push((key, base64::engine::general_purpose::STANDARD.encode(value)));
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
            self.admit(0, reference)?;
        }
        Ok(())
    }
    fn admit(&mut self, level: usize, reference: Reference) -> Result<(), ConversationStoreError> {
        if level >= MAX_DEPTH {
            return Err(storage("fragment tree exceeds bounded depth"));
        }
        if self.levels.len() == level {
            self.levels.push(Vec::new());
        }
        self.levels[level].push(reference);
        if self.levels[level].len() == FANOUT {
            let parent = persist(
                self.connection,
                &Page::Branch {
                    children: std::mem::take(&mut self.levels[level]),
                },
            )?;
            self.admit(level + 1, parent)?;
        }
        Ok(())
    }
    pub(super) fn finish(mut self) -> Result<Option<Reference>, ConversationStoreError> {
        self.flush()?;
        let mut level = 0;
        while level < self.levels.len() {
            if !self.levels[level].is_empty() {
                if level + 1 == self.levels.len() && self.levels[level].len() == 1 {
                    return Ok(self.levels[level].pop());
                }
                let parent = persist(
                    self.connection,
                    &Page::Branch {
                        children: std::mem::take(&mut self.levels[level]),
                    },
                )?;
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
                return entries
                    .into_iter()
                    .find(|entry| entry.0 == key)
                    .map(|entry| {
                        base64::engine::general_purpose::STANDARD
                            .decode(entry.1)
                            .map_err(|_| storage("fragment fact encoding unavailable"))
                    })
                    .transpose();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_fragment_writes_preserve_rollback_tamper_and_schema_failure() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE request_source_pages(hash BLOB PRIMARY KEY,bytes BLOB NOT NULL)",
            )
            .unwrap();
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
            assert_eq!(store_blob(&transaction, b"original").unwrap(), digest);
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
