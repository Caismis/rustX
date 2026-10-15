//! Store-owned immutable relative field metadata. Original bytes always remain
//! on each request. A selected field loads one bounded index; a message-root
//! page loads at most a 64KiB static index and a 16KiB request-owned index.
//! These are separate from the <=2 original64KiB content segments per range.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::runtime::read_layout::{
    CertifiedMessageShape, FieldPreviewPolicy, SeekableObservationSource,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
mod hole;
mod origin;
mod read;
use origin::existing_shape;
fn item_prefix_bytes(item: u64) -> usize {
    15 + item.to_string().len()
}
mod schema;
pub(super) use read::{coordinates, field, page, validate_header};
#[cfg(test)]
pub(in crate::durable::sqlite) use read::{normalized_field_kinds, take_read_work};
pub(in crate::durable::sqlite) use schema::create_schema;
pub(super) use schema::verify_schema;
const MAX_BYTES: usize = 65_536;
const MAX_FIELDS: usize = 512;
const MAX_ITEMS: usize = 8192;
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Field {
    path: String,
    parent: Option<(String, u64)>,
    start: u64,
    end: u64,
    children: u64,
    kind: String,
    nodes: Option<u64>,
    header: Option<Vec<u8>>,
    key_path: Option<String>,
    key_header: Option<Vec<u8>>,
}
#[derive(Serialize, Deserialize)]
struct Metadata {
    static_digest: [u8; 32],
    template_end: u64,
    hole_start: u64,
    hole_end: u64,
    #[serde(deserialize_with = "bounded_fields")]
    fields: Vec<Field>,
}
fn bounded_fields<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Vec<Field>, D::Error> {
    bounded_field_list::<D, MAX_FIELDS>(decoder)
}
fn bounded_field_list<'de, D: serde::Deserializer<'de>, const MAX: usize>(
    decoder: D,
) -> Result<Vec<Field>, D::Error> {
    struct Fields<const MAX: usize>;
    impl<'de, const MAX: usize> serde::de::Visitor<'de> for Fields<MAX> {
        type Value = Vec<Field>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "at most{MAX} relative fields")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut fields = Vec::new();
            for _ in 0..MAX {
                let Some(field) = seq.next_element()? else {
                    return Ok(fields);
                };
                fields.reserve_exact(1);
                fields.push(field);
            }
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom("relative field count"));
            }
            Ok(fields)
        }
    }
    decoder.deserialize_seq(Fields::<MAX>)
}
fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
#[derive(Clone)]
struct Anchor {
    item: u64,
    shape_id: u64,
    shape_hash: [u8; 32],
    header_digest: [u8; 32],
    hole_hash: [u8; 32],
    root_start: u64,
    root_end: u64,
    root_children: u64,
    max_path_bytes: u64,
    hole_start: u64,
    hole_end: u64,
}
fn certificate(id: &str, anchor: &Anchor) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"rustx-request-field-anchor-v2");
    hash.update((id.len() as u64).to_le_bytes());
    hash.update(id.as_bytes());
    for value in [
        anchor.item,
        anchor.shape_id,
        anchor.root_start,
        anchor.root_end,
        anchor.root_children,
        anchor.max_path_bytes,
        anchor.hole_start,
        anchor.hole_end,
    ] {
        hash.update(value.to_le_bytes());
    }
    hash.update(anchor.shape_hash);
    hash.update(anchor.header_digest);
    hash.update(anchor.hole_hash);
    hash.finalize().into()
}
pub(super) fn capture_item(
    connection: &Connection,
    id: &str,
    item: u64,
    header: &[u8],
    key: Option<&super::fragments::Key>,
) -> Result<(), ConversationStoreError> {
    connection.prepare_cached("INSERT INTO request_item_layouts(source_id,item_ordinal,layout_kind,producer_hash,header_digest,root_start,root_end,root_children) VALUES(?1,?2,'own',?3,?4,0,0,0)").map_err(|error|storage(error.to_string()))?
        .execute(params![id,item,key.map(super::fragments::Key::producer_hash).as_ref().map(<[u8;32]>::as_slice),hash(header).as_slice()]).map_err(|error|storage(error.to_string()))?;
    Ok(())
}
pub(super) fn finish_item(
    connection: &Connection,
    id: &str,
    item: u64,
    start: u64,
    end: u64,
    children: u64,
) -> Result<(), ConversationStoreError> {
    if connection.prepare_cached("UPDATE request_item_layouts SET root_start=?3,root_end=?4,root_children=?5 WHERE source_id=?1 AND item_ordinal=?2 AND layout_kind='own'").map_err(|error|storage(error.to_string()))?
        .execute(params![id,item,start,end,children]).map_err(|error|storage(error.to_string()))?!=1 {return Err(storage("request item layout unavailable"));}
    Ok(())
}

fn capture(
    reader: &crate::runtime::read_layout::MessageShapeReader<'_>,
    source: &SeekableObservationSource,
    cert: &CertifiedMessageShape<'_>,
) -> Option<Metadata> {
    let mut fields = Vec::new();
    let result = reader.visit_message_shape_fields(
        cert,
        |_| FieldPreviewPolicy::OriginalSpanIfSmall,
        |path, encoded, parent, start, end, children, key, preview| {
            let (kind, nodes, header) =
                super::stored_preview(source, path, start, end, true, preview)
                    .map_err(|_| "relative preview unavailable")?;
            fields.push(Field {
                path: encoded.to_owned(),
                parent: parent.map(|(path, ordinal)| (path.to_owned(), ordinal)),
                start,
                end,
                children,
                kind: kind.into(),
                nodes,
                header,
                key_path: None,
                key_header: None,
            });
            if let Some((start, end)) = key
                && path.last().is_some_and(|part| part.starts_with('@'))
            {
                let mut key_path = path.to_vec();
                key_path.push("$key".into());
                let encoded = serde_json::to_string(&key_path).map_err(|_| "relative key path")?;
                let descriptor = crate::durable::observation::RequestObservationKey {
                    path: key_path,
                    preview: String::new(),
                    encoded_bytes: end - start,
                };
                if let Some(field) = fields.last_mut() {
                    field.key_path = Some(encoded.clone());
                    field.key_header =
                        Some(serde_json::to_vec(&descriptor).map_err(|_| "relative key header")?);
                }
                fields.push(Field {
                    path: encoded,
                    parent: None,
                    start,
                    end,
                    children: 0,
                    kind: "inline".into(),
                    nodes: None,
                    header: Some(b"\"\"".to_vec()),
                    key_path: None,
                    key_header: None,
                });
            }
            if fields.len() > MAX_FIELDS {
                return Err("relative fields exceed bound");
            }
            Ok(())
        },
    );
    if result.is_err() {
        return None;
    }
    fields.sort_by(|a, b| a.path.cmp(&b.path));
    if fields.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return None;
    }
    Some(Metadata {
        static_digest: cert.static_digest,
        template_end: cert.root.end - cert.root.start,
        hole_start: cert.hole.start - cert.root.start,
        hole_end: cert.hole.end - cert.root.start,
        fields,
    })
}
type Candidate = (u64, Vec<u8>, Vec<u8>, Option<Vec<u8>>);
pub(super) fn prepare<'a>(
    connection: &Connection,
    id: &str,
    source: &'a SeekableObservationSource,
    reader: &crate::runtime::read_layout::MessageShapeReader<'a>,
) -> Result<Vec<CertifiedMessageShape<'a>>, ConversationStoreError> {
    let mut certificates = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_ITEMS {
        let row:Option<Candidate>=connection.prepare_cached("SELECT i.item_ordinal,CASE WHEN typeof(i.producer_hash)='blob' AND length(i.producer_hash)=32 THEN i.producer_hash END,CASE WHEN typeof(i.header_digest)='blob' AND length(i.header_digest)=32 THEN i.header_digest END,CASE WHEN typeof(f.header_json)='blob' AND length(f.header_json)<=16384 THEN f.header_json END FROM request_item_layouts i JOIN request_observation_fields f ON f.source_id=i.source_id AND f.path_json=json_array('messages',CAST(i.item_ordinal AS TEXT)) WHERE i.source_id=?1 AND i.producer_hash IS NOT NULL AND i.item_ordinal>=?2 ORDER BY i.item_ordinal LIMIT 1").map_err(|error|storage(error.to_string()))?
            .query_row(params![id,offset],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(|error|storage(error.to_string()))?;
        let Some((item, producer, header_digest, header)) = row else {
            break;
        };
        offset = item
            .checked_add(1)
            .ok_or_else(|| storage("request item overflow"))?;
        let header = header.ok_or_else(|| storage("request shape header unavailable"))?;
        if hash(&header).as_slice() != header_digest {
            return Err(storage("request shape header changed"));
        }
        let Some(cert) = reader
            .certify_message_shape(item, &header)
            .map_err(storage)?
        else {
            continue;
        };
        let Some(hole_metadata) = hole::capture(reader, &cert) else {
            continue;
        };
        let hole_hash = hash(&hole_metadata);
        let existing = existing_shape(connection, &producer, &cert.static_digest, item)?;
        let (shape_id, shape_hash, max_path_bytes) = if let Some(existing) = existing {
            existing
        } else {
            let Some(metadata) = capture(reader, source, &cert) else {
                continue;
            };
            let max_path = metadata
                .fields
                .iter()
                .filter(|field| !field.path.ends_with(",\"$key\"]"))
                .map(|field| field.path.len())
                .max()
                .unwrap_or(1);
            if item_prefix_bytes(item) + max_path - 1 > 3600 {
                return Err(storage("request shape path exceeds bound"));
            }
            let Some(metadata) =
                crate::runtime_client::checkpoint::bounded_bytes(&metadata, MAX_BYTES)
            else {
                continue;
            };
            let digest = hash(&metadata);
            connection.prepare_cached("INSERT INTO request_field_shapes(producer_hash,static_digest,shape_hash,metadata,origin_source_id,origin_item,max_path_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7)").map_err(|error|storage(error.to_string()))?
                .execute(params![producer,cert.static_digest.as_slice(),digest.as_slice(),metadata,id,item,max_path]).map_err(|error|storage(error.to_string()))?;
            (
                u64::try_from(connection.last_insert_rowid())
                    .map_err(|_| storage("request shape identity overflow"))?,
                digest.to_vec(),
                u64::try_from(max_path)
                    .map_err(|_| storage("request shape path length invalid"))?,
            )
        };
        let anchor = Anchor {
            hole_hash,
            item,
            shape_id,
            shape_hash: shape_hash
                .as_slice()
                .try_into()
                .map_err(|_| storage("request shape digest invalid"))?,
            header_digest: header_digest
                .as_slice()
                .try_into()
                .map_err(|_| storage("request header digest invalid"))?,
            root_start: cert.root.start,
            root_end: cert.root.end,
            root_children: cert.root.children,
            max_path_bytes,
            hole_start: cert.hole.start,
            hole_end: cert.hole.end,
        };
        connection.prepare_cached("UPDATE request_item_layouts SET layout_kind='shape',shape_id=?3,shape_hash=?4,root_start=?5,root_end=?6,hole_start=?7,hole_end=?8,root_children=?9,max_path_bytes=?10,hole_hash=?11,hole_metadata=?12 WHERE source_id=?1 AND item_ordinal=?2 AND layout_kind='own'").map_err(|error|storage(error.to_string()))?
            .execute(params![id,item,shape_id,anchor.shape_hash.as_slice(),anchor.root_start,anchor.root_end,anchor.hole_start,anchor.hole_end,anchor.root_children,anchor.max_path_bytes,anchor.hole_hash.as_slice(),hole_metadata]).map_err(|error|storage(error.to_string()))?;
        let path = serde_json::to_string(&["messages".to_owned(), item.to_string()])
            .map_err(|error| storage(error.to_string()))?;
        if connection.prepare_cached(r#"UPDATE request_observation_fields SET shape_witness=?3,start=?5,end=?6,children=?7 WHERE source_id=?1 AND path_json=?2 AND parent_path='["messages"]' AND ordinal=?4 AND start=0 AND end=0 AND header_kind='inline' AND header_nodes IS NULL"#).map_err(|error|storage(error.to_string()))?
            .execute(params![id,path,certificate(id,&anchor).as_slice(),item,anchor.root_start,anchor.root_end,anchor.root_children]).map_err(|error|storage(error.to_string()))?!=1 {return Err(storage("request shape witness unavailable"));}
        certificates.push(cert);
    }
    Ok(certificates)
}
