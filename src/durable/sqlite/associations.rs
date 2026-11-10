//! Immutable numeric Tool-association chain; request coordinates live in a
//! small binding. Seats become reusable only after the enclosing COMMIT.
use super::statements::FixedStatements;
use super::{Connection, ConversationStoreError, manifest, storage};
use crate::model::occurrence::{
    AssociationCoordinates, AssociationNode, AssociationRoot, FrozenToolAssociations,
    HistoricalToolAssociation, StoredAssociation, ToolAssociationHistory,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Weak, atomic::Ordering};

mod origins;
#[cfg(test)]
pub(super) use origins::WRITTEN as ORIGIN_WRITTEN;
const MAX_PRIVATE_BYTES: usize = 64 * 1024 * 1024;
const HEADER_BYTES: u64 = 128;

#[derive(Serialize, Deserialize)]
struct Fact<T> {
    #[serde(flatten)]
    association: T,
    origin_set: origins::Root,
}
fn byte_bound() -> ConversationStoreError {
    crate::context::resources::bound("historical Tool associations exceed materialization bound")
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Binding {
    pub root: Option<AssociationRoot>,
    pub inserted_at: Option<u32>,
    pub relabel: bool,
    pub stored_boundary: Option<u32>,
}
impl Binding {
    pub(super) fn coordinates(
        &self,
        snapshot: &crate::model::RequestSnapshot,
    ) -> AssociationCoordinates {
        AssociationCoordinates {
            inserted_at: self.inserted_at,
            wire: self.relabel.then(|| crate::model::occurrence::WirePolicy {
                protocol: snapshot.invocation.protocol,
                origin: snapshot.invocation.replay_origin(),
                stored_boundary: self.stored_boundary,
            }),
        }
    }
}
pub(super) struct Selection {
    pub binding: Binding,
    history: ToolAssociationHistory,
    seats: Vec<(Weak<AssociationNode>, AssociationRoot)>,
    epoch: u64,
}
fn invalid(message: &str) -> ConversationStoreError {
    ConversationStoreError::InvalidReference(message.into())
}
fn digest(parent: Option<AssociationRoot>, json: &str, private_bytes: u64) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update(b"rustx-tool-association-v2\0");
    if let Some(parent) = parent {
        sha.update(parent.digest);
        sha.update(parent.count.to_le_bytes());
        sha.update(parent.private_bytes.to_le_bytes());
    }
    sha.update(private_bytes.to_le_bytes());
    sha.update(json.as_bytes());
    sha.finalize().into()
}
fn history_and_coordinates(
    values: &FrozenToolAssociations,
    request: &crate::runtime::RequestId,
) -> Result<(ToolAssociationHistory, AssociationCoordinates), ConversationStoreError> {
    Ok(if let Some(binding) = &values.binding {
        if binding.request != *request {
            return Err(invalid("foreign Tool association binding"));
        }
        (binding.history.clone(), binding.coordinates.clone())
    } else {
        let mut history = ToolAssociationHistory::default();
        for value in values {
            if value.call.message.request_id != *request || value.result.request_id != *request {
                return Err(invalid("foreign actual-request Tool association"));
            }
            history.push_back(HistoricalToolAssociation {
                call_message: value.call.message.item_index,
                call_block: value.call.block.0,
                result_message: value.result.item_index,
                source: value.source.clone(),
                wire_id: value.wire_id.clone(),
                native_origins: Arc::default(),
            });
        }
        (history, AssociationCoordinates::default())
    })
}
pub(super) fn persist(
    connection: &Connection,
    values: &FrozenToolAssociations,
    request: &crate::runtime::RequestId,
    owner: &manifest::Owner,
) -> Result<Selection, ConversationStoreError> {
    let (history, coordinates) = history_and_coordinates(values, request)?;
    let epoch = owner.epoch.load(Ordering::Acquire);
    let mut pending = Vec::new();
    let mut next = history.head.clone();
    let mut root = None;
    while let Some(node) = next.take() {
        #[cfg(test)]
        VISITED.with(|work| work.set(work.get() + 1));
        if let Some(saved) = *node
            .stored
            .lock()
            .map_err(|_| storage("Tool association seat poisoned"))?
            && saved.owner == owner.id
            && saved.epoch == epoch
            && epoch != u64::MAX
        {
            root = Some(saved.root);
            break;
        }
        next.clone_from(&node.parent);
        pending.push(node);
    }
    let mut seats = Vec::new();
    let mut origin_sets = std::collections::HashMap::new();
    for node in pending.into_iter().rev() {
        let origin_set = match origin_sets.entry(node.value.call_message) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                let (root, origins) = entry.get();
                if !Arc::ptr_eq(origins, &node.value.native_origins)
                    && origins != &node.value.native_origins
                {
                    return Err(invalid("conflicting Tool origin sets for one assistant"));
                }
                *root
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry
                    .insert((
                        origins::persist(connection, &node.value)?,
                        Arc::clone(&node.value.native_origins),
                    ))
                    .0
            }
        };
        let json = serde_json::to_string(&Fact {
            association: &node.value,
            origin_set,
        })
        .map_err(|e| storage(e.to_string()))?;
        let private_bytes = root
            .map_or(0, |r: AssociationRoot| r.private_bytes)
            .checked_add(json.len() as u64 + HEADER_BYTES)
            .and_then(|n| {
                n.checked_add(if node.first_origin_set {
                    origin_set.bytes
                } else {
                    0
                })
            })
            .filter(|n| *n <= MAX_PRIVATE_BYTES as u64)
            .ok_or_else(|| invalid("Tool association private byte budget"))?;
        let hash = digest(root, &json, private_bytes);
        let existing:Option<(u64,bool)>=connection.query_row_cached("SELECT id,parent_id IS ?2 AND parent_digest IS ?3 AND parent_private_bytes IS ?7 AND count=?4 AND private_bytes=?6 AND typeof(association_json)='text' AND CAST(association_json AS BLOB)=?5 FROM tool_association_nodes WHERE digest=?1",params![hash.as_slice(),root.map(|r|r.id),root.as_ref().map(|r|r.digest.as_slice()),node.count,json.as_bytes(),private_bytes,root.map(|r|r.private_bytes)],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|e|super::sqlite_error(&e,e.to_string()))?;
        let id = if let Some((id, matches)) = existing {
            if !matches {
                return Err(invalid("Tool association stored bytes differ"));
            }
            id
        } else {
            connection.execute_cached("INSERT INTO tool_association_nodes(parent_id,parent_digest,digest,count,association_json,private_bytes,parent_private_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![root.map(|r|r.id),root.as_ref().map(|r|r.digest.as_slice()),hash.as_slice(),node.count,json,private_bytes,root.map(|r|r.private_bytes)]).map_err(|e|super::sqlite_error(&e,e.to_string()))?;
            #[cfg(test)]
            WRITTEN.with(|work| {
                let (rows, bytes) = work.get();
                work.set((rows + 1, bytes + json.len()));
            });
            super::nonnegative(connection.last_insert_rowid(), "Tool association ID")?
        };
        let current = AssociationRoot {
            id,
            digest: hash,
            count: node.count,
            private_bytes,
        };
        seats.push((Arc::downgrade(&node), current));
        root = Some(current);
    }
    Ok(Selection {
        binding: Binding {
            root,
            inserted_at: coordinates.inserted_at,
            relabel: coordinates.wire.is_some(),
            stored_boundary: coordinates.wire.and_then(|wire| wire.stored_boundary),
        },
        history,
        seats,
        epoch,
    })
}
impl Selection {
    pub(super) fn authenticate(
        &self,
        connection: &Connection,
        owner: &manifest::Owner,
    ) -> Result<(), ConversationStoreError> {
        if owner.epoch.load(Ordering::Acquire) != self.epoch {
            let stored = load(connection, &self.binding, MAX_PRIVATE_BYTES)?;
            if stored != self.history {
                return Err(invalid("changed Tool association history"));
            }
        }
        Ok(())
    }
    pub(super) fn commit(self, owner: &manifest::Owner) {
        if self.epoch != owner.epoch.load(Ordering::Acquire) {
            return;
        }
        for (weak, root) in self.seats {
            if let Some(node) = weak.upgrade() {
                *node.stored.lock().expect("committed Tool association seat") =
                    Some(StoredAssociation {
                        owner: owner.id,
                        epoch: self.epoch,
                        root,
                    });
            }
        }
    }
}
/// Scalar consumers authenticate the pinned root header without materializing
/// the relationships, just as native selected-input metadata authenticates its
/// root. Full reconstruction authenticates every payload in the chain.
pub(super) fn validate_binding(
    connection: &Connection,
    binding: &Binding,
) -> Result<(), ConversationStoreError> {
    if (!binding.relabel && (binding.inserted_at.is_some() || binding.stored_boundary.is_some()))
        || binding.inserted_at.is_some_and(|at| at > 8192)
        || binding.stored_boundary.is_some_and(|at| at >= 8192)
    {
        return Err(invalid("Tool association binding shape"));
    }
    if let Some(root) = binding.root {
        if root.id == 0
            || root.count == 0
            || root.count > 8192
            || root.private_bytes > MAX_PRIVATE_BYTES as u64
        {
            return Err(invalid("Tool association root bounds"));
        }
        let matches:bool=connection.query_row_cached("SELECT EXISTS(SELECT 1 FROM tool_association_nodes WHERE id=?1 AND digest=?2 AND count=?3 AND private_bytes=?4)",params![root.id,root.digest.as_slice(),root.count,root.private_bytes],|row|row.get(0)).map_err(|e|super::sqlite_error(&e,e.to_string()))?;
        if !matches {
            return Err(invalid("Tool association root unavailable or changed"));
        }
    }
    Ok(())
}
type StoredRow = (
    Option<u64>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<u64>,
    u32,
    u64,
    Option<String>,
);
pub(super) fn load(
    connection: &Connection,
    binding: &Binding,
    max_bytes: usize,
) -> Result<ToolAssociationHistory, ConversationStoreError> {
    let mut next = binding.root;
    let mut values = Vec::new();
    let allowance = max_bytes.min(MAX_PRIVATE_BYTES) as u64;
    if binding
        .root
        .is_some_and(|root| root.private_bytes > allowance)
    {
        return Err(byte_bound());
    }
    let mut remaining = allowance;
    let mut origins = origins::Reader::default();
    while let Some(root) = next {
        if root.count == 0 || root.count > 8192 {
            return Err(invalid("Tool association count bound"));
        }
        if remaining < HEADER_BYTES {
            return Err(byte_bound());
        }
        let (parent,parent_digest,stored_digest,parent_bytes,count,private_bytes,json):StoredRow=connection.query_row_cached(
            "SELECT parent_id,CASE WHEN typeof(parent_digest)='blob' AND length(parent_digest)=32 THEN parent_digest END,CASE WHEN typeof(digest)='blob' AND length(digest)=32 THEN digest END,parent_private_bytes,count,private_bytes,CASE WHEN (parent_digest IS NULL OR (typeof(parent_digest)='blob' AND length(parent_digest)=32)) AND typeof(association_json)='text' AND octet_length(association_json)<=?2 THEN association_json END FROM tool_association_nodes WHERE id=?1",
            params![root.id,remaining-HEADER_BYTES],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
        ).optional().map_err(|e|super::sqlite_error(&e,e.to_string()))?.ok_or_else(||invalid("Tool association node unavailable"))?;
        let json = json.ok_or_else(byte_bound)?;
        remaining -= json.len() as u64 + HEADER_BYTES;
        #[cfg(test)]
        READ_BYTES.with(|work| work.set(work.get() + json.len() as u64 + HEADER_BYTES));
        let parent = match (parent, parent_digest, parent_bytes) {
            (None, None, None) if count == 1 => None,
            (Some(id), Some(hash), Some(bytes))
                if id < root.id && count > 1 && bytes < private_bytes =>
            {
                Some(AssociationRoot {
                    id,
                    digest: hash
                        .try_into()
                        .map_err(|_| invalid("Tool association parent digest shape"))?,
                    count: count - 1,
                    private_bytes: bytes,
                })
            }
            _ => return Err(invalid("Tool association chain shape")),
        };
        if count != root.count
            || private_bytes != root.private_bytes
            || stored_digest.as_deref() != Some(root.digest.as_slice())
            || digest(parent, &json, private_bytes) != root.digest
        {
            return Err(invalid("Tool association integrity mismatch"));
        }
        let Fact {
            association: mut value,
            origin_set,
        } = super::decode::<Fact<HistoricalToolAssociation>>(&json, "Tool association")?;
        if value.call_message >= value.result_message || value.result_message >= 8192 {
            return Err(invalid("Tool association occurrence bounds"));
        }
        if binding.relabel && value.wire_id.as_ref().is_none_or(|id| id.0.is_empty()) {
            return Err(invalid("invalid historical wire correlation ID"));
        }
        value.native_origins = origins.load(connection, origin_set, &value, &mut remaining)?;
        values.push(value);
        next = parent;
    }
    if binding.root.map_or(0, |root| root.private_bytes) != allowance - remaining {
        return Err(invalid("Tool association byte accounting mismatch"));
    }
    Ok(values.into_iter().rev().collect())
}
#[cfg(test)]
thread_local! {pub(super) static WRITTEN:std::cell::Cell<(usize,usize)>=const{std::cell::Cell::new((0,0))};}

#[cfg(test)]
thread_local! {pub(super) static VISITED:std::cell::Cell<usize>=const{std::cell::Cell::new(0)};}

#[cfg(test)]
thread_local! {pub(super) static READ_BYTES:std::cell::Cell<u64>=const{std::cell::Cell::new(0)};}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_association_bytes_cannot_overflow_shifted_coordinates() {
        use crate::durable::{CanonicalStore, ConversationStore};
        let store = super::super::test_support::store();
        store.initialize(&[]).unwrap();
        // A digest proves stored bytes, not that decoded coordinates are safe.
        let json = serde_json::json!({
            "call_message": u32::MAX, "call_block": 0, "result_message": 1,
            "source": {"assistant": {
                "conversation_id": store.conversation_id(), "message_id": "call",
                "content_digest": "00"
            }, "block": 0},
            "wire_id": "wire", "origin_set": {"id": 1, "digest": vec![0; 32], "bytes": 40}
        })
        .to_string();
        let private_bytes = json.len() as u64 + HEADER_BYTES;
        let hash = digest(None, &json, private_bytes);
        let connection = store.lock().unwrap();
        connection.execute("INSERT INTO tool_association_nodes(id,digest,count,association_json,private_bytes) VALUES(1,?1,1,?2,?3)", params![hash.as_slice(), json,private_bytes]).unwrap();
        let binding = Binding {
            root: Some(AssociationRoot {
                id: 1,
                digest: hash,
                count: 1,
                private_bytes,
            }),
            inserted_at: Some(0),
            relabel: true,
            stored_boundary: None,
        };
        assert!(
            matches!(load(&connection, &binding, 4096), Err(ConversationStoreError::InvalidReference(message)) if message == "Tool association occurrence bounds")
        );
    }
}
