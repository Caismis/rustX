//! Per-request geometry for the generated source descriptor, never its bytes.
use super::{
    Anchor, CertifiedMessageShape, Connection, ConversationStoreError, Deserialize, Field,
    Serialize, bounded_field_list, hash, params, storage,
};
pub(super) const MAX_HOLE_BYTES: usize = 16_384;
const MAX_HOLE_FIELDS: usize = 32;
#[derive(Serialize, Deserialize)]
pub(super) struct HoleMetadata {
    pub(super) ordinal: u64,
    #[serde(deserialize_with = "bounded_hole_fields")]
    pub(super) fields: Vec<Field>,
}
fn bounded_hole_fields<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Vec<Field>, D::Error> {
    bounded_field_list::<D, MAX_HOLE_FIELDS>(decoder)
}
pub(super) fn capture(
    reader: &crate::runtime::read_layout::MessageShapeReader<'_>,
    cert: &CertifiedMessageShape<'_>,
) -> Option<Vec<u8>> {
    if !(1..=32).contains(&cert.hole.recursive_nodes)
        || cert.hole.end - cert.hole.start > MAX_HOLE_BYTES as u64
    {
        return None;
    }
    let mut fields = Vec::new();
    reader
        .visit_message_hole_fields(
            cert,
            |path, encoded, parent, start, end, children, _, preview| {
                let crate::runtime::read_layout::FieldPreview::OriginalSpan { recursive_nodes } =
                    preview
                else {
                    return Err("own hole preview exceeds bound");
                };
                if fields.len() == MAX_HOLE_FIELDS
                    || path.len() > 32
                    || encoded.len() > 4096
                    || path
                        .last()
                        .is_some_and(|key| key.starts_with('@') || key == "$key")
                {
                    return Err("own hole fields exceed bound");
                }
                fields.push(Field {
                    path: encoded.into(),
                    parent: parent.map(|(path, ordinal)| (path.into(), ordinal)),
                    start,
                    end,
                    children,
                    kind: "original_span".into(),
                    nodes: Some(recursive_nodes),
                    header: None,
                    key_path: None,
                    key_header: None,
                });
                Ok(())
            },
        )
        .ok()?;
    fields.sort_by(|a, b| a.path.cmp(&b.path));
    let metadata = HoleMetadata {
        ordinal: cert.hole_ordinal,
        fields,
    };
    validate(
        &metadata,
        cert.hole.end - cert.hole.start,
        cert.root.children,
    )
    .ok()?;
    crate::runtime_client::checkpoint::bounded_bytes(&metadata, MAX_HOLE_BYTES)
}
pub(super) fn validate(
    metadata: &HoleMetadata,
    length: u64,
    root_children: u64,
) -> Result<(), ConversationStoreError> {
    if length == 0
        || length > MAX_HOLE_BYTES as u64
        || metadata.ordinal >= root_children
        || metadata.fields.is_empty()
        || metadata.fields.len() > MAX_HOLE_FIELDS
    {
        return Err(storage("own hole geometry unavailable"));
    }
    let mut previous = None;
    for field in &metadata.fields {
        if previous.is_some_and(|path: &str| path >= field.path.as_str())
            || field.start >= field.end
            || field.end > length
            || field.kind != "original_span"
            || field.header.is_some()
            || field.key_path.is_some()
            || field.key_header.is_some()
            || !field.nodes.is_some_and(|n| (1..=32).contains(&n))
            || field.children > 32
        {
            return Err(storage("own hole field corrupt"));
        }
        previous = Some(field.path.as_str());
        let path: Vec<String> =
            serde_json::from_str(&field.path).map_err(|_| storage("own hole path corrupt"))?;
        if path.len() > 32 || path.iter().any(|part| part.len() > 256) || field.path.len() > 3600 {
            return Err(storage("own hole path exceeds bound"));
        }
        let parent = if path.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(&path[..path.len() - 1])
                    .map_err(|_| storage("own hole parent corrupt"))?,
            )
        };
        if field.parent.as_ref().map(|(path, _)| path) != parent.as_ref() {
            return Err(storage("own hole parent differs"));
        }
        let children = metadata
            .fields
            .iter()
            .filter(|child| child.parent.as_ref().is_some_and(|(p, _)| p == &field.path))
            .collect::<Vec<_>>();
        if children.len() as u64 != field.children {
            return Err(storage("own hole children missing"));
        }
        let mut ordinals = children
            .iter()
            .map(|child| child.parent.as_ref().unwrap().1)
            .collect::<Vec<_>>();
        ordinals.sort_unstable();
        if ordinals.iter().copied().ne(0..field.children)
            || children
                .iter()
                .any(|child| child.start < field.start || child.end > field.end)
        {
            return Err(storage("own hole child geometry differs"));
        }
    }
    let root = metadata
        .fields
        .iter()
        .find(|field| field.path == "[]")
        .ok_or_else(|| storage("own hole root missing"))?;
    if root.start != 0 || root.end != length || root.nodes != Some(metadata.fields.len() as u64) {
        return Err(storage("own hole root differs"));
    }
    Ok(())
}

pub(super) fn load(
    connection: &Connection,
    id: &str,
    anchor: &Anchor,
) -> Result<HoleMetadata, ConversationStoreError> {
    let raw:Option<Vec<u8>>=connection.prepare_cached("SELECT CASE WHEN typeof(hole_metadata)='blob' AND length(hole_metadata)<=16384 THEN hole_metadata END FROM request_item_layouts WHERE source_id=?1 AND item_ordinal=?2 AND layout_kind='shape' AND typeof(hole_hash)='blob' AND length(hole_hash)=32 AND hole_hash=?3").map_err(|error|storage(error.to_string()))?
        .query_row(params![id,anchor.item,anchor.hole_hash.as_slice()],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    let raw = raw.ok_or_else(|| storage("own request metadata unavailable"))?;
    #[cfg(test)]
    super::read::record_read(raw.len());
    if hash(&raw) != anchor.hole_hash {
        return Err(storage("own request metadata digest differs"));
    }
    let metadata: HoleMetadata =
        serde_json::from_slice(&raw).map_err(|_| storage("own request metadata corrupt"))?;
    validate(
        &metadata,
        anchor.hole_end - anchor.hole_start,
        anchor.root_children,
    )?;
    Ok(metadata)
}
