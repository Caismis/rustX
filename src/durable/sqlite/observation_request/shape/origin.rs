use super::{
    Anchor, Connection, ConversationStoreError, OptionalExtension, certificate, item_prefix_bytes,
    params, storage,
};
type OriginEvidence = (
    u64,
    u64,
    u64,
    u64,
    u64,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    u64,
    u64,
    u64,
    u64,
    Vec<u8>,
);
// A later request may not launder a rewritten shape+digest into a new witness.
// Reuse requires the fixed original request/item certificate that first captured
// this shape. No original body/current canonical header is read or re-stamped.
pub(super) fn existing_shape(
    connection: &Connection,
    producer: &[u8],
    static_digest: &[u8; 32],
    item: u64,
) -> Result<Option<(u64, Vec<u8>, u64)>, ConversationStoreError> {
    let row:Option<(u64,Vec<u8>,String,u64,usize)>=connection.prepare_cached("SELECT shape_id,CASE WHEN typeof(shape_hash)='blob' AND length(shape_hash)=32 THEN shape_hash END,CASE WHEN typeof(origin_source_id)='text' AND length(CAST(origin_source_id AS BLOB))<=20 THEN origin_source_id END,origin_item,max_path_bytes FROM request_field_shapes WHERE producer_hash=?1 AND static_digest=?2").map_err(|error|storage(error.to_string()))?
        .query_row(params![producer,static_digest.as_slice()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional().map_err(|error|storage(error.to_string()))?;
    let Some((shape_id, shape_hash, source, origin_item, max_path)) = row else {
        return Ok(None);
    };
    if source
        .parse::<u64>()
        .ok()
        .is_none_or(|value| value == 0 || value.to_string() != source)
    {
        return Err(storage("request shape original source invalid"));
    }
    if max_path == 0 || max_path > 3600 || item_prefix_bytes(item) + max_path - 1 > 3600 {
        return Err(storage("request shape path exceeds bound"));
    }
    let path = serde_json::to_string(&["messages".to_owned(), origin_item.to_string()])
        .map_err(|error| storage(error.to_string()))?;
    let row:OriginEvidence=connection.prepare_cached("SELECT i.root_start,i.root_end,i.root_children,i.hole_start,i.hole_end,CASE WHEN typeof(i.shape_hash)='blob' AND length(i.shape_hash)=32 THEN i.shape_hash END,CASE WHEN typeof(i.header_digest)='blob' AND length(i.header_digest)=32 THEN i.header_digest END,CASE WHEN typeof(f.shape_witness)='blob' AND length(f.shape_witness)=32 THEN f.shape_witness END,f.start,f.end,f.children,i.max_path_bytes,CASE WHEN typeof(i.hole_hash)='blob' AND length(i.hole_hash)=32 THEN i.hole_hash END FROM request_item_layouts i JOIN request_observation_fields f ON f.source_id=i.source_id AND f.path_json=?3 WHERE i.source_id=?1 AND i.item_ordinal=?2 AND i.layout_kind='shape' AND i.shape_id=?4 AND i.producer_hash=?5").map_err(|error|storage(error.to_string()))?
        .query_row(params![source,origin_item,path,shape_id,producer],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?,row.get(10)?,row.get(11)?,row.get(12)?))).map_err(|_|storage("request shape original certificate unavailable"))?;
    let (
        start,
        end,
        children,
        hole_start,
        hole_end,
        original_hash,
        header,
        witness,
        field_start,
        field_end,
        field_children,
        original_max_path,
        hole_hash,
    ) = row;
    let anchor = Anchor {
        hole_hash: hole_hash
            .as_slice()
            .try_into()
            .map_err(|_| storage("original hole digest invalid"))?,
        item: origin_item,
        shape_id,
        shape_hash: original_hash
            .as_slice()
            .try_into()
            .map_err(|_| storage("original shape digest invalid"))?,
        header_digest: header
            .as_slice()
            .try_into()
            .map_err(|_| storage("original header digest invalid"))?,
        root_start: start,
        root_end: end,
        root_children: children,
        max_path_bytes: original_max_path,
        hole_start,
        hole_end,
    };
    if u64::try_from(max_path).ok() != Some(original_max_path)
        || shape_hash != original_hash
        || certificate(&source, &anchor).as_slice() != witness
        || field_start != start
        || field_end != end
        || field_children != children
    {
        return Err(storage("request shape original certificate differs"));
    }
    Ok(Some((shape_id, shape_hash, original_max_path)))
}
