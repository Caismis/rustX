//! One authenticated origin-set fact per assistant, shared by its associations.
use super::super::statements::FixedStatements;
#[cfg(test)]
use super::READ_BYTES;
use super::{
    Connection, ConversationStoreError, HistoricalToolAssociation, MAX_PRIVATE_BYTES, byte_bound,
    invalid, storage,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Arc,
};

// Charge the fixed digest/scalar cells as well as the variable JSON payload.
const HEADER_BYTES: u64 = 40;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Root {
    pub id: u64,
    pub digest: [u8; 32],
    pub bytes: u64,
}
#[derive(Serialize, Deserialize)]
struct Fact {
    call_message: u32,
    origins: Arc<Vec<[u8; 32]>>,
}
fn hash(json: &str) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update(b"rustx-tool-origins-v1\0");
    sha.update(json.as_bytes());
    sha.finalize().into()
}
pub(super) fn persist(
    connection: &Connection,
    value: &HistoricalToolAssociation,
) -> Result<Root, ConversationStoreError> {
    let fact = Fact {
        call_message: value.call_message,
        origins: Arc::clone(&value.native_origins),
    };
    let json = serde_json::to_string(&fact).map_err(|e| storage(e.to_string()))?;
    let bytes = json.len() as u64 + HEADER_BYTES;
    if bytes > MAX_PRIVATE_BYTES as u64 {
        return Err(invalid("Tool origin-set byte budget"));
    }
    let digest = hash(&json);
    let existing: Option<(u64, bool)> = connection.query_row_cached(
        "SELECT id,typeof(origins_json)='text' AND CAST(origins_json AS BLOB)=?2 FROM tool_association_origin_sets WHERE digest=?1",
        params![digest.as_slice(),json.as_bytes()], |row| Ok((row.get(0)?,row.get(1)?)),
    ).optional().map_err(|e|super::super::sqlite_error(&e,e.to_string()))?;
    let id = if let Some((id, matches)) = existing {
        if !matches {
            return Err(invalid("Tool origin-set stored bytes differ"));
        }
        id
    } else {
        connection
            .execute_cached(
                "INSERT INTO tool_association_origin_sets(digest,origins_json) VALUES(?1,?2)",
                params![digest.as_slice(), json],
            )
            .map_err(|e| super::super::sqlite_error(&e, e.to_string()))?;
        #[cfg(test)]
        WRITTEN.with(|work| {
            let (rows, bytes) = work.get();
            work.set((rows + 1, bytes + json.len()));
        });
        super::super::nonnegative(connection.last_insert_rowid(), "Tool origin-set ID")?
    };
    Ok(Root { id, digest, bytes })
}
#[derive(Default)]
pub(super) struct Reader {
    facts: HashMap<u64, (Root, Fact)>,
    assistants: HashMap<u32, Root>,
}
impl Reader {
    pub(super) fn load(
        &mut self,
        connection: &Connection,
        root: Root,
        value: &HistoricalToolAssociation,
        remaining: &mut u64,
    ) -> Result<Arc<Vec<[u8; 32]>>, ConversationStoreError> {
        if let Some(expected) = self.assistants.insert(value.call_message, root)
            && expected != root
        {
            return Err(invalid("conflicting Tool origin sets for one assistant"));
        }
        if let Entry::Vacant(entry) = self.facts.entry(root.id) {
            if root.bytes < HEADER_BYTES || root.bytes > *remaining {
                return Err(byte_bound());
            }
            let row: Option<(Option<Vec<u8>>, Option<String>)> = connection.query_row_cached(
                "SELECT CASE WHEN typeof(digest)='blob' AND length(digest)=32 THEN digest END,CASE WHEN typeof(origins_json)='text' AND octet_length(origins_json)<=?2 THEN origins_json END FROM tool_association_origin_sets WHERE id=?1",
                params![root.id,root.bytes-HEADER_BYTES],|row|Ok((row.get(0)?,row.get(1)?)),
            ).optional().map_err(|e|super::super::sqlite_error(&e,e.to_string()))?;
            let (digest, json) = row.ok_or_else(|| invalid("Tool origin-set unavailable"))?;
            let json = json.ok_or_else(byte_bound)?;
            *remaining -= json.len() as u64 + HEADER_BYTES;
            #[cfg(test)]
            READ_BYTES.with(|work| work.set(work.get() + json.len() as u64 + HEADER_BYTES));
            if json.len() as u64 + HEADER_BYTES != root.bytes
                || digest.as_deref() != Some(root.digest.as_slice())
                || hash(&json) != root.digest
            {
                return Err(invalid("Tool origin-set integrity mismatch"));
            }
            let fact = super::super::decode::<Fact>(&json, "Tool origin-set")?;
            entry.insert((root, fact));
        }
        let (stored, fact) = &self.facts[&root.id];
        if *stored != root || fact.call_message != value.call_message {
            return Err(invalid("Tool origin-set ownership mismatch"));
        }
        Ok(Arc::clone(&fact.origins))
    }
}
#[cfg(test)]
thread_local! {pub(in crate::durable::sqlite) static WRITTEN:std::cell::Cell<(usize,usize)>=const{std::cell::Cell::new((0,0))};}
