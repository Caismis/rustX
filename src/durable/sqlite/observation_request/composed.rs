//! Composed `RequestStart` inspection over the native immutable selected-input tree.
//! Native nodes own occurrence order. These optional augmentations own only
//! allowlisted encoded bytes and authenticated field geometry.
use super::{Connection, ConversationStoreError, params, storage};
use crate::durable::sqlite::manifest;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
mod fragments;
mod index;
#[cfg(test)]
mod measurement;
mod nodes;
#[cfg(feature = "issue-387-profile")]
mod profile;
mod read;
mod scalar;
#[cfg(test)]
mod scalar_benchmark;
mod schema;
#[cfg(test)]
mod schema_tests;
mod weights;
#[cfg(test)]
mod width_tests;
pub(super) use read::{coordinates, field, page, range};
pub(in crate::durable::sqlite) use schema::{create_schema, verify_schema};
#[cfg(test)]
pub(in crate::durable::sqlite) use width_tests::{assert_variants, boundary_length};
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Work {
    pub index_pages: usize,
    pub index_bytes: usize,
    pub max_index_page: usize,
    pub native_nodes: usize,
    pub source_nodes: usize,
    pub source_metadata_bytes: usize,
    pub max_source_metadata: usize,
    pub new_leaves: usize,
    pub new_nodes: usize,
    pub new_fragment_bytes: usize,
    pub header_decodes: usize,
    pub template_loads: usize,
    pub leaf_width_calls: usize,
    pub width_hole_encodes: usize,
}
#[cfg(test)]
thread_local! {static WORK:std::cell::Cell<Work>=const{std::cell::Cell::new(Work{index_pages:0,index_bytes:0,max_index_page:0,native_nodes:0,source_nodes:0,source_metadata_bytes:0,max_source_metadata:0,new_leaves:0,new_nodes:0,new_fragment_bytes:0,header_decodes:0,template_loads:0,leaf_width_calls:0,width_hole_encodes:0})};}
#[cfg(test)]
pub(in crate::durable::sqlite) fn take_work() -> Work {
    WORK.with(|work| work.replace(Work::default()))
}
#[cfg(test)]
fn record_metadata(bytes: usize) {
    WORK.with(|work| {
        let mut value = work.get();
        value.source_nodes += 1;
        value.source_metadata_bytes += bytes;
        value.max_source_metadata = value.max_source_metadata.max(bytes);
        work.set(value);
    });
}
#[cfg(test)]
fn record_index(bytes: usize) {
    WORK.with(|work| {
        let mut value = work.get();
        value.index_pages += 1;
        value.index_bytes += bytes;
        value.max_index_page = value.max_index_page.max(bytes);
        work.set(value);
    });
}
const SAFE: u64 = 9_007_199_254_740_991;
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Fragment {
    bytes: index::Reference,
    fields: index::Reference,
    length: u64,
    max_path: u64,
    max_key_path: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Template {
    prefix: Fragment,
    suffix: Fragment,
    header_prefix: String,
    header_suffix: String,
    provenance_bytes: u64,
    children: u64,
    nodes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Leaf {
    /// Width classes reference deduplicated template metadata, never body copies.
    variants: Vec<u8>,
    templates: Vec<index::Hash>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Content {
    Leaf {
        leaf: Leaf,
    },
    Branch {
        left: manifest::NodeRef,
        left_hash: index::Hash,
        right: manifest::NodeRef,
        right_hash: index::Hash,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Augmentation {
    native: manifest::NodeRef,
    /// Sum of static bytes at each occurrence width; actual J/rank digits are
    /// generated at read time. Branch crossing a rank width descends its children.
    widths: Vec<u64>,
    max_path: u64,
    max_key_path: u64,
    content: Content,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RootKind {
    Request,
    #[cfg(test)]
    Scalar,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Root {
    kind: RootKind,
    sequence: u64,
    native: Option<manifest::NodeRef>,
    source_hash: Option<index::Hash>,
    scalar: scalar::Scalar,
    messages_start: u64,
    messages_end: u64,
    length: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Fact {
    path: Vec<String>,
    parent: Option<Vec<String>>,
    ordinal: Option<u64>,
    start: u64,
    end: u64,
    children: u64,
    nodes: u64,
    object: bool,
    container: bool,
    header: String,
    key: Option<u64>,
    name: Option<String>,
}
fn bounded<T: Serialize>(value: &T, max: usize) -> Result<Vec<u8>, ConversationStoreError> {
    crate::runtime_client::checkpoint::bounded_bytes(value, max)
        .ok_or_else(|| storage("composed observation metadata exceeds bound"))
}
fn hash(bytes: &[u8]) -> index::Hash {
    Sha256::digest(bytes).into()
}
fn add(a: u64, b: u64) -> Result<u64, ConversationStoreError> {
    a.checked_add(b)
        .filter(|value| *value <= SAFE)
        .ok_or_else(|| storage("composed observation byte offset exceeds bound"))
}
fn invalid() -> ConversationStoreError {
    ConversationStoreError::InvalidReference("request field is unavailable".into())
}
fn root_checksum(root: &[u8]) -> index::Hash {
    let mut sha = Sha256::new();
    sha.update(b"rustx-composed-request-source-v1\0");
    sha.update(root);
    sha.finalize().into()
}
pub(super) fn index_selection(
    connection: &Connection,
    sequence: u64,
    scope: &crate::runtime::identity::ConversationId,
    input: &crate::model::SelectedModelInput,
    snapshot: &crate::model::RequestSnapshot,
    selection: &manifest::Selection,
) -> Result<(), ConversationStoreError> {
    #[cfg(feature = "issue-387-profile")]
    let mut profile = profile::Request::new(sequence, selection.root.map_or(0, |root| root.count));
    #[cfg(test)]
    let _measurement =
        measurement::Measurement::new(crate::agent::execution::attribution::SourceScope::Request);
    let source_hash = {
        #[cfg(feature = "issue-387-profile")]
        let _profile = profile::Timer::new(profile::Phase::Capture);
        #[cfg(test)]
        let _timer = crate::agent::execution::attribution::Timer::new(
            crate::agent::execution::attribution::Phase::SourceCapture,
        );
        nodes::capture(connection, scope, input, snapshot, selection)?
    };
    let (scalar, messages_start, messages_end) = {
        #[cfg(feature = "issue-387-profile")]
        let _profile = profile::Timer::new(profile::Phase::Scalar);
        #[cfg(test)]
        let _timer = crate::agent::execution::attribution::Timer::new(
            crate::agent::execution::attribution::Phase::SourceScalar,
        );
        let value = crate::runtime_client::trace::frozen_request_detail(
            snapshot,
            std::iter::empty::<u8>().collect::<Vec<_>>(),
        );
        let source =
            crate::runtime::read_layout::SeekableObservationSource::new(&value).map_err(storage)?;
        scalar::capture(connection, &source)?
    };
    {
        #[cfg(feature = "issue-387-profile")]
        let _profile = profile::Timer::new(profile::Phase::Paths);
        #[cfg(test)]
        let _timer = crate::agent::execution::attribution::Timer::new(
            crate::agent::execution::attribution::Phase::SourcePaths,
        );
        if let (Some(native), Some(digest)) = (selection.root, source_hash) {
            nodes::validate_paths(connection, native, digest, 0)?;
        }
    }
    let messages_length = {
        #[cfg(feature = "issue-387-profile")]
        let _profile = profile::Timer::new(profile::Phase::Length);
        #[cfg(test)]
        let _timer = crate::agent::execution::attribution::Timer::new(
            crate::agent::execution::attribution::Phase::SourceLength,
        );
        match (selection.root, source_hash) {
            (Some(native), Some(digest)) => {
                add(nodes::length(connection, native, digest, 0, sequence)?, 2)?
            }
            (None, None) => 2,
            _ => return Err(storage("composed selected root unavailable")),
        }
    };
    let length = add(
        scalar
            .length()
            .checked_sub(2)
            .ok_or_else(|| storage("scalar messages geometry unavailable"))?,
        messages_length,
    )?;
    let root = Root {
        kind: RootKind::Request,
        sequence,
        native: selection.root,
        source_hash,
        scalar,
        messages_start,
        messages_end,
        length,
    };
    let bytes = bounded(&root, 16_384)?;
    let checksum = root_checksum(&bytes);
    connection
        .execute(
            "INSERT INTO request_source_roots(source_id,metadata,checksum) VALUES(?1,?2,?3)",
            params![sequence.to_string(), bytes, checksum.as_slice()],
        )
        .map_err(|error| storage(error.to_string()))?;
    connection
        .execute(
            "INSERT INTO content_frontiers VALUES('trace_request',?1,0,'canonical_json_v1',1,?2)",
            params![sequence.to_string(), length],
        )
        .map_err(|error| storage(error.to_string()))?;
    #[cfg(feature = "issue-387-profile")]
    profile.layout_completed();
    Ok(())
}
type RootRow = (
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<u64>,
    Option<Vec<u8>>,
    u64,
    u64,
);
fn load_root(connection: &Connection, id: &str) -> Result<Root, ConversationStoreError> {
    let sequence: u64 = id.parse().map_err(|_| invalid())?;
    if sequence == 0 || sequence.to_string() != id {
        return Err(invalid());
    }
    let row: RootRow = connection.prepare_cached("SELECT CASE WHEN typeof(r.metadata)='blob' AND length(r.metadata)<=16384 THEN r.metadata END,CASE WHEN typeof(r.checksum)='blob' AND length(r.checksum)=32 THEN r.checksum END,s.input_root,CASE WHEN typeof(s.input_root_digest)='blob' AND length(s.input_root_digest)=32 THEN s.input_root_digest END,s.input_count,f.readable_end FROM request_source_roots r JOIN request_snapshots s ON s.observation_source_id=r.source_id JOIN content_frontiers f ON f.source_kind='trace_request' AND f.source_id=r.source_id AND f.block_index=0 AND f.representation='canonical_json_v1' AND f.revision=1 WHERE r.source_id=?1").map_err(|error|storage(error.to_string()))?.query_row([id], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).map_err(|error|storage(error.to_string()))?;
    let bytes = row
        .0
        .ok_or_else(|| storage("composed request root unavailable"))?;
    #[cfg(test)]
    record_metadata(bytes.len());
    let digest = root_checksum(&bytes);
    if row.1.as_deref() != Some(digest.as_slice()) {
        return Err(storage("composed request root checksum differs"));
    }
    let root: Root =
        serde_json::from_slice(&bytes).map_err(|_| storage("composed request root malformed"))?;
    root.scalar.validate_extent()?;
    if root.sequence != sequence
        || root.length > SAFE
        || root.length != row.5
        || root.scalar.length() > root.length
        || root.kind != RootKind::Request && root.length != root.scalar.length()
        || root.native.is_some_and(|node| {
            node.count == 0
                || node.count > manifest::MAX_ITEMS
                || node.height == 0
                || node.height > manifest::MAX_HEIGHT
        })
        || root.kind == RootKind::Request
            && (root.messages_start >= root.messages_end
                || root.messages_end - root.messages_start != 2)
        || root.messages_end > root.scalar.length()
        || root.native.map(|node| node.id) != row.2
        || root.native.map_or(0, |node| node.count) != row.4
        || root.native.as_ref().map(|node| node.digest.as_slice()) != row.3.as_deref()
        || root.kind == RootKind::Request && root.native.is_some() != root.source_hash.is_some()
    {
        return Err(storage("original composed request identity differs"));
    }
    Ok(root)
}

#[cfg(test)]
pub(super) fn index_scalar<T: serde::Serialize + schemars::JsonSchema>(
    connection: &Connection,
    sequence: u64,
    value: &T,
) -> Result<(), ConversationStoreError> {
    let source =
        crate::runtime::read_layout::SeekableObservationSource::new(value).map_err(storage)?;
    let scalar = scalar::whole(connection, &source)?;
    let native=connection.query_row("SELECT s.input_root,s.input_root_digest,s.input_count,n.height FROM request_snapshots s LEFT JOIN native_input_nodes n ON n.id=s.input_root WHERE s.observation_source_id=?1",[sequence.to_string()],|row|{let id:Option<u64>=row.get(0)?;let digest:Option<Vec<u8>>=row.get(1)?;let count:u64=row.get(2)?;let height:Option<u8>=row.get(3)?;Ok((id,digest,count,height))}).map_err(|error|storage(error.to_string()))?;
    let native = match native {
        (Some(id), Some(digest), count, Some(height)) => Some(manifest::NodeRef {
            id,
            digest: digest
                .try_into()
                .map_err(|_| storage("native selected root digest unavailable"))?,
            count,
            height,
        }),
        (None, None, 0, None) => None,
        _ => return Err(storage("native selected root unavailable")),
    };
    let root = Root {
        kind: RootKind::Scalar,
        sequence,
        native,
        source_hash: None,
        length: scalar.length(),
        scalar,
        messages_start: 0,
        messages_end: 0,
    };
    let bytes = bounded(&root, 16384)?;
    let checksum = root_checksum(&bytes);
    connection
        .execute(
            "DELETE FROM request_source_roots WHERE source_id=?1",
            [sequence.to_string()],
        )
        .map_err(|error| storage(error.to_string()))?;
    connection
        .execute(
            "INSERT INTO request_source_roots(source_id,metadata,checksum) VALUES(?1,?2,?3)",
            params![sequence.to_string(), bytes, checksum.as_slice()],
        )
        .map_err(|error| storage(error.to_string()))?;
    connection
        .execute(
            "INSERT INTO content_frontiers VALUES('trace_request',?1,0,'canonical_json_v1',1,?2)",
            params![sequence.to_string(), root.length],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
pub(in crate::durable::sqlite) fn assert_single_native_validation(
    connection: &Connection,
    sequence: u64,
) {
    let root = load_root(connection, &sequence.to_string()).unwrap();
    let native = root.native.unwrap();
    assert!(native.height > 1);
    manifest::take_node_read_work();
    manifest::node(connection, &native).unwrap();
    let expected = manifest::take_node_read_work();
    nodes::load(connection, native, root.source_hash.unwrap()).unwrap();
    let actual = manifest::take_node_read_work();
    assert!(expected.0 > 0);
    assert_eq!(expected.1, 1);
    assert_eq!(
        actual, expected,
        "same-function Source validation reuses its actual native proof"
    );
}

#[cfg(test)]
pub(in crate::durable::sqlite) fn inspect_index_page(bytes: &[u8]) -> serde_json::Value {
    index::inspect_page(bytes)
}
#[cfg(test)]
pub(in crate::durable::sqlite) fn encode_test_index_page(page: serde_json::Value) -> Vec<u8> {
    index::encode_test_page(page)
}
