use super::*;
struct Selected {
    anchor: Anchor,
    fields: Vec<Field>,
    static_hole: Option<(u64, u64)>,
    item_prefix: Vec<String>,
    hole_root: Option<(u64, Field)>,
}
#[cfg(test)]
thread_local! {static READ_WORK:std::cell::Cell<(usize,usize)>=const{std::cell::Cell::new((0,0))};}
#[cfg(test)]
pub(in crate::durable::sqlite) fn take_read_work() -> (usize, usize) {
    READ_WORK.with(|work| work.replace((0, 0)))
}
#[cfg(test)]
pub(super) fn record_read(length: usize) {
    READ_WORK.with(|work| {
        let (calls, bytes) = work.get();
        work.set((calls + 1, bytes.max(length)));
    });
}
type AnchorRow = (
    String,
    Option<u64>,
    u64,
    u64,
    u64,
    Option<u64>,
    Option<u64>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    u64,
    u64,
    u64,
    Option<u64>,
    Option<Vec<u8>>,
    bool,
);
fn selected_item(
    connection: &Connection,
    id: &str,
    path: &[String],
) -> Result<u64, ConversationStoreError> {
    let invalid = || ConversationStoreError::InvalidReference("request item is unavailable".into());
    let item = path[1].parse::<u64>().map_err(|_| invalid())?;
    if item.to_string() != path[1] {
        return Err(invalid());
    }
    let total:Option<u64>=connection.prepare_cached("SELECT CASE WHEN typeof(children)='integer' AND children BETWEEN 0 AND 9007199254740991 THEN children END FROM request_observation_fields WHERE source_id=?1 AND path_json='[\"messages\"]'").map_err(|error|storage(error.to_string()))?
        .query_row([id],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?.flatten();
    let total = total.ok_or_else(|| storage("request message extent unavailable"))?;
    if item >= total {
        return Err(invalid());
    }
    Ok(item)
}
fn load_anchor(
    connection: &Connection,
    id: &str,
    path: &[String],
) -> Result<Option<Anchor>, ConversationStoreError> {
    if path.len() < 2 || path[0] != "messages" {
        return Ok(None);
    }
    let item = selected_item(connection, id, path)?;
    let root_path =
        serde_json::to_string(&path[..2]).map_err(|error| storage(error.to_string()))?;
    let row:AnchorRow=connection.prepare_cached("SELECT CASE WHEN i.layout_kind IN ('own','shape') THEN i.layout_kind END,i.shape_id,i.root_start,i.root_end,i.root_children,i.hole_start,i.hole_end,CASE WHEN typeof(i.shape_hash)='blob' AND length(i.shape_hash)=32 THEN i.shape_hash END,CASE WHEN typeof(i.header_digest)='blob' AND length(i.header_digest)=32 THEN i.header_digest END,CASE WHEN typeof(f.shape_witness)='blob' AND length(f.shape_witness)=32 THEN f.shape_witness END,f.start,f.end,f.children,i.max_path_bytes,CASE WHEN typeof(i.hole_hash)='blob' AND length(i.hole_hash)=32 THEN i.hole_hash END,(i.hole_hash IS NOT NULL OR i.hole_metadata IS NOT NULL) FROM request_item_layouts i JOIN request_observation_fields f ON f.source_id=i.source_id AND f.path_json=?3 WHERE i.source_id=?1 AND i.item_ordinal=?2").map_err(|error|storage(error.to_string()))?
        .query_row(params![id,item,root_path],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?,row.get(10)?,row.get(11)?,row.get(12)?,row.get(13)?,row.get(14)?,row.get(15)?))).map_err(|_|storage("request item ownership unavailable"))?;
    let (
        kind,
        shape_id,
        start,
        end,
        root_children,
        hole_start,
        hole_end,
        digest,
        header_digest,
        witness,
        field_start,
        field_end,
        field_children,
        max_path_bytes,
        hole_hash,
        hole_present,
    ) = row;
    if start != field_start || end != field_end || root_children != field_children {
        return Err(storage("request root field geometry differs"));
    }
    if kind == "own" {
        if shape_id.is_some()
            || digest.is_some()
            || hole_start.is_some()
            || hole_end.is_some()
            || witness.is_some()
            || max_path_bytes.is_some()
            || hole_present
        {
            return Err(storage("own request item has shape evidence"));
        }
        return Ok(None);
    }
    if kind != "shape" {
        return Err(storage("request item storage unavailable"));
    }
    let anchor = Anchor {
        hole_hash: hole_hash
            .as_deref()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| storage("request hole digest missing"))?,
        item,
        shape_id: shape_id.ok_or_else(|| storage("request shape missing"))?,
        shape_hash: digest
            .as_deref()
            .and_then(|digest| digest.try_into().ok())
            .ok_or_else(|| storage("request shape digest missing"))?,
        header_digest: header_digest
            .as_deref()
            .and_then(|digest| digest.try_into().ok())
            .ok_or_else(|| storage("request header digest missing"))?,
        root_start: start,
        root_end: end,
        root_children,
        max_path_bytes: max_path_bytes
            .filter(|max| *max > 0 && *max <= 3600)
            .ok_or_else(|| storage("request shape path bound unavailable"))?,
        hole_start: hole_start.ok_or_else(|| storage("request hole absent"))?,
        hole_end: hole_end.ok_or_else(|| storage("request hole absent"))?,
    };
    if root_children > 32
        || start > end
        || end > crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX
        || anchor.hole_start < start
        || anchor.hole_start >= anchor.hole_end
        || anchor.hole_end > end
        || witness.as_deref() != Some(certificate(id, &anchor).as_slice())
    {
        return Err(storage("request shape ownership witness differs"));
    }
    Ok(Some(anchor))
}
fn load(
    connection: &Connection,
    id: &str,
    path: &[String],
    children: bool,
) -> Result<Option<Selected>, ConversationStoreError> {
    let Some(anchor) = load_anchor(connection, id, path)? else {
        return Ok(None);
    };
    if !(path.len() > 2 || children && path.len() == 2) {
        return Ok(None);
    }
    let dynamic = path.get(2).is_some_and(|part| part == "provenance_source");
    let packed = if dynamic || path.len() == 2 {
        Some(hole::load(connection, id, &anchor)?)
    } else {
        None
    };
    if dynamic {
        let packed = packed.ok_or_else(|| storage("request own fields unavailable"))?;
        let mut prefix = path[..2].to_vec();
        prefix.push("provenance_source".into());
        return Ok(Some(Selected {
            fields: packed.fields,
            static_hole: None,
            anchor,
            item_prefix: prefix,
            hole_root: None,
        }));
    }
    let raw:Option<Vec<u8>>=connection.prepare_cached("SELECT CASE WHEN typeof(metadata)='blob' AND length(metadata)<=65536 THEN metadata END FROM request_field_shapes WHERE shape_id=?1 AND typeof(shape_hash)='blob' AND length(shape_hash)=32 AND shape_hash=?2").map_err(|error|storage(error.to_string()))?
        .query_row(params![anchor.shape_id,anchor.shape_hash.as_slice()],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?.flatten();
    let raw = raw.ok_or_else(|| storage("request shape metadata unavailable"))?;
    #[cfg(test)]
    record_read(raw.len());
    if hash(&raw) != anchor.shape_hash {
        return Err(storage("request shape metadata digest differs"));
    }
    let metadata: Metadata =
        serde_json::from_slice(&raw).map_err(|_| storage("request shape metadata corrupt"))?;
    validate(&metadata, &anchor, &path[..2])?;
    let hole_root = packed.map(|packed| {
        let root = packed
            .fields
            .into_iter()
            .find(|field| field.path == "[]")
            .expect("validated own root");
        (packed.ordinal, root)
    });
    Ok(Some(Selected {
        anchor,
        static_hole: Some((metadata.hole_start, metadata.hole_end)),
        fields: metadata.fields,
        item_prefix: path[..2].to_vec(),
        hole_root,
    }))
}
fn validate(
    metadata: &Metadata,
    anchor: &Anchor,
    prefix: &[String],
) -> Result<(), ConversationStoreError> {
    if metadata.template_end > 16_384
        || metadata.hole_start >= metadata.hole_end
        || metadata.hole_end > metadata.template_end
        || anchor.hole_start - anchor.root_start != metadata.hole_start
        || anchor.root_end - anchor.hole_end != metadata.template_end - metadata.hole_end
        || metadata.fields.is_empty()
        || metadata.fields.len() > MAX_FIELDS
    {
        return Err(storage("request shape geometry differs"));
    }
    let mut previous = None;
    for field in &metadata.fields {
        if previous.is_some_and(|path: &str| path >= field.path.as_str())
            || field.start > field.end
            || field.end > metadata.template_end
            || field.children > crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX
        {
            return Err(storage("request shape field order or span invalid"));
        }
        previous = Some(field.path.as_str());
        let relative: Vec<String> =
            serde_json::from_str(&field.path).map_err(|_| storage("request shape path corrupt"))?;
        if relative.is_empty() || relative[0] == "provenance_source" {
            return Err(storage("request shape contains dynamic field"));
        }
        let mut full = prefix.to_vec();
        full.extend(relative);
        let limit = if full.last().is_some_and(|part| part == "$key") {
            4096
        } else {
            3600
        };
        if super::super::encoded_path(&full)?.len() > limit {
            return Err(storage("request shape path exceeds bound"));
        }
        if field.start < metadata.hole_end && field.end > metadata.hole_start {
            return Err(storage("static request field crosses hole"));
        }
    }
    Ok(())
}
impl Selected {
    fn position(&self, relative: u64) -> Result<u64, ConversationStoreError> {
        let Some((hole_start, hole_end)) = self.static_hole else {
            return self
                .anchor
                .hole_start
                .checked_add(relative)
                .filter(|end| *end <= self.anchor.hole_end)
                .ok_or_else(|| storage("own request span unavailable"));
        };
        let value = if relative <= hole_start {
            self.anchor.root_start.checked_add(relative)
        } else if relative >= hole_end {
            self.anchor.hole_end.checked_add(relative - hole_end)
        } else {
            None
        };
        value
            .filter(|value| *value <= self.anchor.root_end)
            .ok_or_else(|| storage("relative request span unavailable"))
    }
    fn find(&self, path: &[String]) -> Result<&Field, ConversationStoreError> {
        let relative = serde_json::to_string(&path[self.item_prefix.len()..])
            .map_err(|error| storage(error.to_string()))?;
        self.fields
            .binary_search_by(|field| field.path.cmp(&relative))
            .ok()
            .map(|index| &self.fields[index])
            .ok_or_else(|| {
                ConversationStoreError::InvalidReference("request field is unavailable".into())
            })
    }
    fn stored(&self, field: &Field) -> Result<super::super::StoredField, ConversationStoreError> {
        if field.kind != "inline" && field.kind != "original_span"
            || field
                .header
                .as_ref()
                .is_some_and(|bytes| bytes.len() > 16_384)
        {
            return Err(storage("request shape header unavailable"));
        }
        Ok(super::super::StoredField {
            start: self.position(field.start)?,
            end: self.position(field.end)?,
            children: field.children,
            kind: Some(field.kind.clone()),
            nodes: field.nodes,
            bytes: field.header.clone(),
            absent: field.header.is_none(),
        })
    }
    fn current_path(&self, relative: &str) -> Result<Vec<String>, ConversationStoreError> {
        let mut path = self.item_prefix.clone();
        path.extend(
            serde_json::from_str::<Vec<String>>(relative)
                .map_err(|_| storage("request shape path corrupt"))?,
        );
        Ok(path)
    }
}
pub(in crate::durable::sqlite::observation_request) fn coordinates(
    connection: &Connection,
    id: &str,
    path: &[String],
) -> Result<Option<(u64, u64, u64)>, ConversationStoreError> {
    let Some(selected) = load(connection, id, path, false)? else {
        return Ok(None);
    };
    let field = selected.find(path)?;
    Ok(Some((
        selected.position(field.start)?,
        selected.position(field.end)?,
        field.children,
    )))
}
type SelectedField = (super::super::StoredField, Option<Vec<u8>>);
pub(in crate::durable::sqlite::observation_request) fn field(
    connection: &Connection,
    id: &str,
    path: &[String],
) -> Result<Option<SelectedField>, ConversationStoreError> {
    let Some(selected) = load(connection, id, path, false)? else {
        return Ok(None);
    };
    let field = selected.find(path)?;
    Ok(Some((selected.stored(field)?, selected.key(field)?)))
}
impl Selected {
    fn key(&self, field: &Field) -> Result<Option<Vec<u8>>, ConversationStoreError> {
        field
            .key_header
            .as_ref()
            .map(|bytes| {
                let mut key: crate::durable::observation::RequestObservationKey =
                    serde_json::from_slice(bytes)
                        .map_err(|_| storage("relative key header corrupt"))?;
                let mut path = self.item_prefix.clone();
                path.extend(key.path);
                key.path = path;
                serde_json::to_vec(&key).map_err(|error| storage(error.to_string()))
            })
            .transpose()
    }
}
type PageField = (u64, Vec<String>, super::super::StoredField, Option<Vec<u8>>);
impl Selected {
    fn page_candidates(
        &self,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<Vec<PageField>, ConversationStoreError> {
        let relative = serde_json::to_string(&path[self.item_prefix.len()..])
            .map_err(|error| storage(error.to_string()))?;
        let mut candidates: Vec<PageField> = Vec::new();
        let mut borrowed = self
            .fields
            .iter()
            .filter_map(|field| {
                field
                    .parent
                    .as_ref()
                    .filter(|(parent, ordinal)| *parent == relative && *ordinal >= offset)
                    .map(|(_, ordinal)| (*ordinal, field))
            })
            .collect::<Vec<_>>();
        borrowed.sort_by_key(|(ordinal, _)| *ordinal);
        borrowed.truncate(limit);
        for (ordinal, field) in borrowed {
            candidates.push((
                ordinal,
                self.current_path(&field.path)?,
                self.stored(field)?,
                self.key(field)?,
            ));
        }
        if let Some((ordinal, root)) = &self.hole_root
            && *ordinal >= offset
            && path.len() == 2
        {
            let mut path = self.item_prefix.clone();
            path.push("provenance_source".into());
            let field = super::super::StoredField {
                start: self.anchor.hole_start,
                end: self.anchor.hole_end,
                children: root.children,
                kind: Some(root.kind.clone()),
                nodes: root.nodes,
                bytes: None,
                absent: true,
            };
            candidates.push((*ordinal, path, field, None));
        }
        candidates.sort_by_key(|field| field.0);
        candidates.truncate(limit);
        Ok(candidates)
    }
}
pub(in crate::durable::sqlite::observation_request) fn page(
    connection: &Connection,
    id: &str,
    path: &[String],
    offset: u64,
    limit: usize,
) -> Result<Option<crate::durable::observation::RequestObservationPage>, ConversationStoreError> {
    let Some(selected) = load(connection, id, path, true)? else {
        return Ok(None);
    };
    let total = if path.len() == 2 {
        selected.anchor.root_children
    } else {
        selected.find(path)?.children
    };
    if offset > total {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let candidates = selected.page_candidates(path, offset, limit)?;
    let mut fields = Vec::new();
    for (index, (ordinal, path, field, key)) in candidates.into_iter().enumerate() {
        if ordinal != offset + index as u64 {
            return Err(storage(
                "request shape page has missing or duplicate member",
            ));
        }
        let (start, end, children_total, bytes) =
            field.materialize(connection, "request_observation_fields", id)?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| storage("request shape header corrupt"))?;
        fields.push(crate::durable::observation::RequestObservationField {
            content: super::super::metadata_list(
                "request_observation_fields",
                &path,
                &bytes,
                "content",
            )?,
            collections: super::super::metadata_list(
                "request_observation_fields",
                &path,
                &bytes,
                "collections",
            )?,
            deferred: end - start > bytes.len() as u64
                || super::super::message_descriptor_deferred(
                    "request_observation_fields",
                    &path,
                    &value,
                ),
            path,
            value,
            key: key
                .map(|key| {
                    serde_json::from_slice(&key).map_err(|_| storage("request field key corrupt"))
                })
                .transpose()?,
            encoded_bytes: end - start,
            children_total,
        });
    }
    if fields.is_empty() && offset < total {
        return Err(storage("request shape page cannot advance"));
    }
    while crate::runtime_client::checkpoint::bounded_bytes(&fields, 524_288).is_none() {
        if fields.len() <= 1 {
            return Err(storage("request shape field cannot fit bounded page"));
        }
        fields.pop();
    }
    let next = offset + fields.len() as u64;
    Ok(Some(crate::durable::observation::RequestObservationPage {
        total,
        offset,
        fields,
        next_offset: (next < total).then_some(next),
    }))
}

pub(in crate::durable::sqlite::observation_request) fn validate_header(
    connection: &Connection,
    id: &str,
    path: &[String],
    bytes: &[u8],
) -> Result<(), ConversationStoreError> {
    if path.len() != 2 || path[0] != "messages" {
        return Ok(());
    }
    load(connection, id, path, false)?;
    let expected:Option<Vec<u8>>=connection.prepare_cached("SELECT CASE WHEN typeof(header_digest)='blob' AND length(header_digest)=32 THEN header_digest END FROM request_item_layouts WHERE source_id=?1 AND item_ordinal=?2").map_err(|error|storage(error.to_string()))?
        .query_row(params![id,path[1].parse::<u64>().map_err(|_|storage("invalid request ordinal"))?],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
    if expected.as_deref() != Some(hash(bytes).as_slice()) {
        return Err(storage("request item header digest differs"));
    }
    Ok(())
}

/// Full cfg oracle enumeration, resolving the same verified metadata as bounded
/// production reads. This deliberately counts logical fields rather than rows.
#[cfg(test)]
pub(in crate::durable::sqlite) fn normalized_field_kinds(
    connection: &Connection,
    id: &str,
) -> Result<std::collections::BTreeMap<Vec<String>, String>, ConversationStoreError> {
    let mut fields = std::collections::BTreeMap::new();
    let mut statement = connection
        .prepare("SELECT path_json,header_kind FROM request_observation_fields WHERE source_id=?1")
        .map_err(|error| storage(error.to_string()))?;
    let rows = statement
        .query_map([id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| storage(error.to_string()))?;
    for row in rows {
        let (path, kind) = row.map_err(|error| storage(error.to_string()))?;
        let path = serde_json::from_str(&path).map_err(|error| storage(error.to_string()))?;
        if fields.insert(path, kind).is_some() {
            return Err(storage("duplicate own request field"));
        }
    }
    let mut statement = connection
        .prepare("SELECT item_ordinal FROM request_item_layouts WHERE source_id=?1 AND layout_kind='shape' ORDER BY item_ordinal")
        .map_err(|error| storage(error.to_string()))?;
    let items = statement
        .query_map([id], |row| row.get::<_, u64>(0))
        .map_err(|error| storage(error.to_string()))?;
    for item in items {
        let item = item.map_err(|error| storage(error.to_string()))?;
        let path = ["messages".into(), item.to_string()];
        let selected = load(connection, id, &path, true)?
            .ok_or_else(|| storage("shared request fields unavailable"))?;
        for field in &selected.fields {
            if fields
                .insert(selected.current_path(&field.path)?, field.kind.clone())
                .is_some()
            {
                return Err(storage("duplicate shared request field"));
            }
        }
        let path = [
            "messages".into(),
            item.to_string(),
            "provenance_source".into(),
        ];
        let packed = load(connection, id, &path, true)?
            .ok_or_else(|| storage("own request fields unavailable"))?;
        for field in &packed.fields {
            if fields
                .insert(packed.current_path(&field.path)?, field.kind.clone())
                .is_some()
            {
                return Err(storage("duplicate packed request field"));
            }
        }
    }
    Ok(fields)
}
