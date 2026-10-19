//! Native selected-input topology. Optional Source metadata never enters these digests.
use super::{
    Arc, AtomicU64, Connection, ConversationStoreError, Digest, Mutex, OptionalExtension, Ordering,
    Sha256, decode, nonnegative, params, seq_to_i64, storage,
};

#[cfg(test)]
mod tests;
use crate::model::projected::FrozenModelInput;
use crate::model::selected_input::{InputNode, InputNodeKind, StoredInputNode};

#[cfg(test)]
thread_local! {static NODE_READ_WORK:std::cell::Cell<(usize,usize)>=const{std::cell::Cell::new((0,0))};}
#[cfg(test)]
pub(super) fn node_read_work() -> (usize, usize) {
    NODE_READ_WORK.with(std::cell::Cell::get)
}
#[cfg(test)]
pub(super) fn take_node_read_work() -> (usize, usize) {
    NODE_READ_WORK.with(|work| work.replace((0, 0)))
}
pub(super) const MAX_HEIGHT: u8 = 32;
pub(super) const MAX_ITEMS: u64 = 8192;
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct NodeRef {
    pub id: u64,
    pub digest: [u8; 32],
    pub count: u64,
    pub height: u8,
}
#[derive(Debug, Clone)]
pub(super) enum NodeKind {
    Leaf,
    Branch { left: NodeRef, right: NodeRef },
}
#[derive(Debug, Clone)]
pub(super) struct Node {
    pub kind: NodeKind,
}
#[derive(Debug, Clone)]
pub(super) struct Selection {
    pub root: Option<NodeRef>,
    seats: Vec<(std::sync::Weak<InputNode>, NodeRef)>,
    epoch: u64,
}

const NODE_SCHEMA: &str = "CREATE TABLE native_input_nodes (
        id INTEGER PRIMARY KEY,digest BLOB NOT NULL CHECK(typeof(digest)='blob' AND length(digest)=32),
        count INTEGER NOT NULL CHECK(typeof(count)='integer' AND count BETWEEN 1 AND 8192),
        height INTEGER NOT NULL CHECK(typeof(height)='integer' AND height BETWEEN 1 AND 32),
        item_digest BLOB CHECK(item_digest IS NULL OR (typeof(item_digest)='blob' AND length(item_digest)=32)),
        left_id INTEGER REFERENCES native_input_nodes(id),right_id INTEGER REFERENCES native_input_nodes(id),
        left_digest BLOB,right_digest BLOB,
        item_json TEXT,
        CHECK((typeof(item_json)='text' AND item_digest IS NOT NULL AND count=1 AND height=1 AND left_id IS NULL AND right_id IS NULL AND left_digest IS NULL AND right_digest IS NULL) OR (item_json IS NULL AND item_digest IS NULL AND typeof(left_id)='integer' AND left_id>0 AND typeof(right_id)='integer' AND right_id>0 AND typeof(left_digest)='blob' AND length(left_digest)=32 AND typeof(right_digest)='blob' AND length(right_digest)=32))
    )";
pub(super) fn create_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection
        .execute_batch(NODE_SCHEMA)
        .map_err(|error| storage(format!("native selected input schema: {error}")))
}
pub(super) fn verify_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    let sql:Option<Option<String>>=connection.query_row("SELECT CASE WHEN typeof(sql)='text' AND length(CAST(sql AS BLOB))<=8192 THEN sql END FROM sqlite_schema WHERE type='table' AND name='native_input_nodes'",[],|r|r.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    if sql.flatten().as_deref() != Some(NODE_SCHEMA) {
        return Err(ConversationStoreError::IncompatibleSchema(
            "native selected input schema differs".into(),
        ));
    }
    Ok(())
}
fn invalid(message: &str) -> ConversationStoreError {
    ConversationStoreError::InvalidReference(message.into())
}
fn bytes32(bytes: Vec<u8>) -> Result<[u8; 32], ConversationStoreError> {
    bytes
        .try_into()
        .map_err(|_| invalid("native selected input digest shape"))
}
fn digest_leaf(item: &[u8; 32]) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update(b"rustx-native-input-leaf-v1\0");
    sha.update(item);
    sha.finalize().into()
}
fn digest_branch(left: NodeRef, right: NodeRef) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update(b"rustx-native-input-branch-v1\0");
    for child in [left, right] {
        sha.update(child.digest);
        sha.update(child.count.to_le_bytes());
        sha.update([child.height]);
    }
    sha.finalize().into()
}
fn header(connection: &Connection, id: u64) -> Result<NodeRef, ConversationStoreError> {
    #[cfg(test)]
    NODE_READ_WORK.with(|work| {
        let (headers, shapes) = work.get();
        work.set((headers + 1, shapes));
        crate::agent::execution::attribution::record_native(
            crate::agent::execution::attribution::NativeWork {
                index_queries: 1,
                ..Default::default()
            },
        );
    });
    let mut statement = connection
        .prepare_cached("SELECT CASE WHEN typeof(digest)='blob' AND length(digest)=32 THEN digest END,count,height FROM native_input_nodes WHERE id=?1")
        .map_err(|error| storage(error.to_string()))?;
    let row: (Vec<u8>, u64, u8) = statement
        .query_row([seq_to_i64(id)?], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| storage(e.to_string()))?;
    if row.1 == 0 || row.1 > MAX_ITEMS || row.2 == 0 || row.2 > MAX_HEIGHT {
        return Err(invalid("native selected input header bounds"));
    }
    Ok(NodeRef {
        id,
        digest: bytes32(row.0)?,
        count: row.1,
        height: row.2,
    })
}
type NodeRow = (
    Option<Vec<u8>>,
    Option<u64>,
    Option<u64>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
);
pub(super) fn node(
    connection: &Connection,
    expected: &NodeRef,
) -> Result<Node, ConversationStoreError> {
    let actual = header(connection, expected.id)?;
    if actual != *expected {
        return Err(invalid("native selected input node identity"));
    }
    #[cfg(test)]
    NODE_READ_WORK.with(|work| {
        let (headers, shapes) = work.get();
        work.set((headers, shapes + 1));
        crate::agent::execution::attribution::record_native(
            crate::agent::execution::attribution::NativeWork {
                index_queries: 1,
                ..Default::default()
            },
        );
    });
    let id = seq_to_i64(expected.id)?;
    let (item, left, right, left_digest, right_digest): NodeRow = connection
        .prepare_cached("SELECT CASE WHEN typeof(item_digest)='blob' AND length(item_digest)=32 THEN item_digest END,left_id,right_id,CASE WHEN typeof(left_digest)='blob' AND length(left_digest)=32 THEN left_digest END,CASE WHEN typeof(right_digest)='blob' AND length(right_digest)=32 THEN right_digest END FROM native_input_nodes WHERE id=?1")
        .and_then(|mut statement| {
            statement.query_row([id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })
        })
        .map_err(|error| storage(error.to_string()))?;
    let kind = match (item, left, right, left_digest, right_digest) {
        (Some(item), None, None, None, None) if actual.count == 1 && actual.height == 1 => {
            if digest_leaf(&bytes32(item)?) != actual.digest {
                return Err(invalid("native selected leaf hash"));
            }
            NodeKind::Leaf
        }
        (None, Some(left), Some(right), Some(lh), Some(rh)) => {
            let left = header(connection, left)?;
            let right = header(connection, right)?;
            if left.digest != bytes32(lh)?
                || right.digest != bytes32(rh)?
                || left.count.checked_add(right.count) != Some(actual.count)
                || 1 + left.height.max(right.height) != actual.height
                || left.height.abs_diff(right.height) > 1
                || digest_branch(left, right) != actual.digest
            {
                return Err(invalid("native selected branch hash/geometry"));
            }
            NodeKind::Branch { left, right }
        }
        _ => return Err(invalid("native selected node closed shape")),
    };
    Ok(Node { kind })
}
pub(super) fn leaf_input(
    connection: &Connection,
    expected: &NodeRef,
) -> Result<FrozenModelInput, ConversationStoreError> {
    if !matches!(node(connection, expected)?.kind, NodeKind::Leaf) {
        return Err(invalid("native selected input is not a leaf"));
    }
    let (json,digest):(Option<String>,Vec<u8>)=connection.query_row("SELECT CASE WHEN typeof(item_json)='text' AND octet_length(item_json)<=67108864 THEN item_json END,item_digest FROM native_input_nodes WHERE id=?1",[seq_to_i64(expected.id)?],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|storage(e.to_string()))?;
    let json = json.ok_or_else(|| invalid("native selected input leaf byte bound"))?;
    if <[u8; 32]>::from(Sha256::digest(json.as_bytes())) != bytes32(digest)? {
        return Err(invalid("native selected input leaf bytes"));
    }
    decode(&json, "native selected input leaf")
}
pub(super) fn persist(
    connection: &Connection,
    input: &crate::model::SelectedInputManifest,
    owner: &Owner,
) -> Result<Selection, ConversationStoreError> {
    if input
        .root
        .as_ref()
        .is_some_and(|root| root.frozen_count != root.count)
    {
        return Err(invalid(
            "native selected input lacks scoped frozen evidence",
        ));
    }
    let mut selected = Selection {
        root: None,
        seats: Vec::new(),
        epoch: owner.epoch.load(Ordering::Acquire),
    };
    if let Some(root) = &input.root {
        selected.root = Some(persist_node(connection, root, owner, &mut selected)?);
    }
    Ok(selected)
}
fn persist_node(
    connection: &Connection,
    input: &Arc<InputNode>,
    owner: &Owner,
    changed: &mut Selection,
) -> Result<NodeRef, ConversationStoreError> {
    if let Some(saved) = *input
        .stored
        .lock()
        .map_err(|_| storage("native input node poisoned"))?
        && saved.owner == owner.id
        && saved.epoch != u64::MAX
        && saved.epoch == owner.epoch.load(Ordering::Acquire)
    {
        return Ok(NodeRef {
            id: saved.id,
            digest: saved.digest,
            count: input.count as u64,
            height: input.height,
        });
    }
    let (digest, item_json, item_digest, left, right) = match &input.kind {
        InputNodeKind::Leaf(value) => {
            let value = value
                .as_deref()
                .ok_or_else(|| invalid("native selected input lacks frozen evidence"))?;
            let json = serde_json::to_string(value).map_err(|e| storage(e.to_string()))?;
            if json.len() > 64 * 1024 * 1024 {
                return Err(invalid("native input leaf byte budget"));
            }
            let ih: [u8; 32] = Sha256::digest(json.as_bytes()).into();
            (digest_leaf(&ih), Some(json), Some(ih), None, None)
        }
        InputNodeKind::Branch { left, right } => {
            let a = persist_node(connection, left, owner, changed)?;
            let b = persist_node(connection, right, owner, changed)?;
            (digest_branch(a, b), None, None, Some(a), Some(b))
        }
    };
    connection.execute("INSERT INTO native_input_nodes(digest,count,height,item_digest,item_json,left_id,right_id,left_digest,right_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![digest.as_slice(),input.count as u64,input.height,item_digest.as_ref().map(<[u8;32]>::as_slice),item_json,left.map(|r|r.id),right.map(|r|r.id),left.as_ref().map(|r|r.digest.as_slice()),right.as_ref().map(|r|r.digest.as_slice())]).map_err(|e|storage(e.to_string()))?;
    let reference = NodeRef {
        id: nonnegative(connection.last_insert_rowid(), "native input node id")?,
        digest,
        count: input.count as u64,
        height: input.height,
    };
    // Publication of this connection-local seat is deferred to successful COMMIT.
    changed.seats.push((Arc::downgrade(input), reference));
    Ok(reference)
}
pub(super) fn load(
    connection: &Connection,
    root: Option<NodeRef>,
) -> Result<crate::model::SelectedInputManifest, ConversationStoreError> {
    load_bounded(connection, root, 64 * 1024 * 1024)
}
pub(super) fn load_bounded(
    connection: &Connection,
    root: Option<NodeRef>,
    max_bytes: usize,
) -> Result<crate::model::SelectedInputManifest, ConversationStoreError> {
    let mut input = crate::model::SelectedInputManifest::default();
    let mut stack: Vec<_> = root.into_iter().collect();
    let mut bytes = 2usize;
    if bytes > max_bytes {
        return Err(invalid("native selected input byte budget"));
    }
    while let Some(reference) = stack.pop() {
        match node(connection, &reference)?.kind {
            NodeKind::Leaf => {
                let length: u64 = connection
                    .query_row(
                        "SELECT octet_length(item_json) FROM native_input_nodes WHERE id=?1",
                        [seq_to_i64(reference.id)?],
                        |r| r.get(0),
                    )
                    .map_err(|e| storage(e.to_string()))?;
                bytes = bytes
                    .checked_add(
                        usize::try_from(length).map_err(|_| invalid("native input byte size"))?,
                    )
                    .and_then(|n| n.checked_add(usize::from(!input.is_empty())))
                    .filter(|n| *n <= max_bytes)
                    .ok_or_else(|| invalid("native selected input byte budget"))?;
                input.push(leaf_input(connection, &reference)?);
            }
            NodeKind::Branch { left, right } => {
                stack.push(right);
                stack.push(left);
            }
        }
    }
    Ok(input)
}

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
pub(super) struct Owner {
    pub id: u64,
    pub epoch: AtomicU64,
    pub frontier: AtomicU64,
    pub journal_frontier: AtomicU64,
    versions: Mutex<Option<(u64, u64)>>,
    pub history: Mutex<Option<(u64, crate::durable::projection::HistoricalModelInput)>>,
}
struct MutationColumns {
    canonical: Vec<i32>,
    journal: Vec<i32>,
    activation: Vec<i32>,
    bootstrap: Vec<i32>,
}
impl MutationColumns {
    fn read(connection: &Connection) -> Self {
        // Resolve indexes from the validated schema rather than assuming column order.
        let columns = |table: &str| {
            connection
                .prepare(&format!("PRAGMA table_info({table})"))
                .expect("native dependency schema")
                .query_map([], |row| {
                    Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))
                })
                .expect("native dependency columns")
                .filter_map(|row| {
                    let (index, name) = row.expect("native dependency column");
                    let native = match table {
                        "message_ledger" => {
                            matches!(name.as_str(), "position" | "message_id" | "message_json")
                        }
                        "events" => matches!(name.as_str(), "sequence" | "event_id" | "event_json"),
                        _ => !name.starts_with("observation_"),
                    };
                    native.then_some(index)
                })
                .collect::<Vec<_>>()
        };
        let canonical = columns("message_ledger");
        assert_eq!(canonical.len(), 3);
        Self {
            canonical,
            journal: columns("events"),
            activation: columns("projection_activations"),
            bootstrap: columns("bootstrap_identity"),
        }
    }
}
fn at_or_before(row_id: i64, frontier: &AtomicU64) -> bool {
    u64::try_from(row_id).map_or(true, |position| {
        position <= frontier.load(Ordering::Relaxed)
    })
}
impl MutationColumns {
    fn invalidates(
        &self,
        watched: &Owner,
        table: &str,
        change: &rusqlite::hooks::PreUpdateCase,
    ) -> bool {
        use rusqlite::hooks::PreUpdateCase;
        match (table, change) {
            ("events", PreUpdateCase::Insert(new)) => {
                at_or_before(new.get_new_row_id(), &watched.journal_frontier)
            }
            (
                "events",
                PreUpdateCase::Update {
                    old_value_accessor: old,
                    new_value_accessor: new,
                },
            ) => self.journal.iter().any(|i| {
                match (old.get_old_column_value(*i), new.get_new_column_value(*i)) {
                    (Ok(a), Ok(b)) => a != b,
                    _ => true,
                }
            }),
            ("message_ledger", PreUpdateCase::Insert(new)) => {
                at_or_before(new.get_new_row_id(), &watched.frontier)
            }
            ("message_ledger", PreUpdateCase::Delete(old)) => {
                at_or_before(old.get_old_row_id(), &watched.frontier)
            }
            (
                "message_ledger",
                PreUpdateCase::Update {
                    old_value_accessor: old,
                    new_value_accessor: new,
                },
            ) => self.canonical.iter().any(|i| {
                match (old.get_old_column_value(*i), new.get_new_column_value(*i)) {
                    (Ok(a), Ok(b)) => a != b,
                    _ => true,
                }
            }),
            (
                "projection_activations" | "bootstrap_identity",
                PreUpdateCase::Update {
                    old_value_accessor: old,
                    new_value_accessor: new,
                },
            ) => {
                let columns = if table == "projection_activations" {
                    &self.activation
                } else {
                    &self.bootstrap
                };
                columns.iter().any(|i| {
                    match (old.get_old_column_value(*i), new.get_new_column_value(*i)) {
                        (Ok(a), Ok(b)) => a != b,
                        _ => true,
                    }
                })
            }
            (
                "native_input_nodes" | "projection_head" | "projection_checkpoints",
                PreUpdateCase::Insert(_),
            ) => table == "projection_head",
            (
                "events" | "projection_activations" | "bootstrap_identity",
                PreUpdateCase::Delete(_),
            )
            | ("native_input_nodes" | "projection_head" | "projection_checkpoints", _)
            | (_, PreUpdateCase::Unknown) => true,
            _ => false,
        }
    }
}
impl Owner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            id: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            epoch: AtomicU64::new(0),
            frontier: AtomicU64::new(0),
            journal_frontier: AtomicU64::new(0),
            versions: Mutex::new(None),
            history: Mutex::new(None),
        })
    }
    #[cfg(test)]
    pub(super) fn uninstalled() -> Arc<Self> {
        Self::new()
    }
    pub fn install(
        connection: &Connection,
        append: Arc<super::append_validation::Owner>,
    ) -> Result<Arc<Self>, ConversationStoreError> {
        let owner = Self::new();
        let watched = owner.clone();
        let columns = MutationColumns::read(connection);
        connection
            .preupdate_hook(Some(
                move |_: rusqlite::hooks::Action,
                      db: &str,
                      table: &str,
                      change: &rusqlite::hooks::PreUpdateCase| {
                    if db != "main" {
                        return;
                    }
                    // One SQLite hook dispatches to independent proof owners.
                    append.observe(table, change);
                    let invalidates = columns.invalidates(&watched, table, change);
                    if invalidates {
                        watched.invalidate();
                    }
                },
            ))
            .map_err(|error| storage(format!("install native input observer: {error}")))?;
        Ok(owner)
    }
    fn invalidate(&self) {
        let _ = self
            .epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1));
    }
    pub fn capture(&self, connection: &Connection) -> Result<u64, ConversationStoreError> {
        // The caller's native read/write transaction must own its SQLite snapshot
        // before sampling the external-connection invalidation token. In WAL,
        // data_version follows the pinned read snapshot; a concurrent commit is
        // observed after that transaction ends, at the next snapshot capture.
        connection
            .query_row("SELECT rootpage FROM sqlite_schema LIMIT 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|error| storage(error.to_string()))?;
        let actual = (
            connection
                .query_row("PRAGMA main.data_version", [], |r| r.get::<_, u64>(0))
                .map_err(|e| storage(e.to_string()))?,
            connection
                .query_row("PRAGMA main.schema_version", [], |r| r.get::<_, u64>(0))
                .map_err(|e| storage(e.to_string()))?,
        );
        let mut versions = self
            .versions
            .lock()
            .map_err(|_| storage("native input version owner"))?;
        if versions.is_some_and(|prior| prior != actual) {
            self.invalidate();
        }
        *versions = Some(actual);
        Ok(self.epoch.load(Ordering::Acquire))
    }
}

impl Selection {
    pub(super) fn commit(self, owner: &Owner) {
        if self.epoch != owner.epoch.load(Ordering::Acquire) {
            return;
        }
        for (weak, node) in self.seats {
            if let Some(input) = weak.upgrade() {
                *input.stored.lock().expect("committed native input seat") =
                    Some(StoredInputNode {
                        owner: owner.id,
                        epoch: self.epoch,
                        id: node.id,
                        digest: node.digest,
                    });
            }
        }
    }
}
