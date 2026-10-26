use super::{
    Connection, Content, ConversationStoreError, Fact, Root, Template, add, fragments, invalid,
    load_root, nodes, scalar, storage,
};
use crate::durable::observation::{
    ObservationValueChunk, RequestObservationField, RequestObservationPage,
};
use base64::Engine as _;

/// One external operation owns at most one decoded bounded scalar. Reuse it
/// through page/preview recursion; message-only reads need no scalar decoding.
struct ReadRoot {
    metadata: Root,
    original: Option<scalar::Original>,
}
impl ReadRoot {
    fn load(
        connection: &Connection,
        id: &str,
        path: &[String],
    ) -> Result<Self, ConversationStoreError> {
        let root = load_root(connection, id)?;
        let original = if root.kind == super::RootKind::Request
            && path.first().is_some_and(|part| part == "messages")
        {
            None
        } else {
            scalar::Original::load(connection, &root.scalar)?
        };
        if root.kind == super::RootKind::Request
            && let Some(original) = &original
        {
            let messages = original
                .fact(&["messages".into()])?
                .ok_or_else(|| storage("original scalar messages unavailable"))?;
            if (messages.start, messages.end, messages.children)
                != (root.messages_start, root.messages_end, 0)
            {
                return Err(storage("original scalar messages geometry differs"));
            }
        }
        Ok(Self {
            metadata: root,
            original,
        })
    }
    fn original(&self) -> Result<&scalar::Original, ConversationStoreError> {
        self.original
            .as_ref()
            .ok_or_else(|| storage("original scalar reader unavailable"))
    }
    fn scalar_fact(
        &self,
        connection: &Connection,
        path: &[String],
    ) -> Result<Option<Fact>, ConversationStoreError> {
        match &self.metadata.scalar {
            scalar::Scalar::Indexed { fragment } => fragments::fact(connection, fragment, path),
            scalar::Scalar::Original { .. } => self.original()?.fact(path),
        }
    }
    fn scalar_child(
        &self,
        connection: &Connection,
        path: &[String],
        ordinal: u64,
    ) -> Result<Option<Vec<String>>, ConversationStoreError> {
        match &self.metadata.scalar {
            scalar::Scalar::Indexed { fragment } => {
                fragments::child(connection, fragment, path, ordinal)
            }
            scalar::Scalar::Original { .. } => self.original()?.child(path, ordinal),
        }
    }
    fn scalar_bytes(
        &self,
        connection: &Connection,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, ConversationStoreError> {
        match &self.metadata.scalar {
            scalar::Scalar::Indexed { fragment } => {
                fragments::bytes(connection, fragment, start, end)
            }
            scalar::Scalar::Original { .. } => self.original()?.bytes(start, end),
        }
    }
}
struct Selected {
    fact: Fact,
    start: u64,
    end: u64,
    override_header: Option<(Template, u64)>,
}
fn header(
    template: &Template,
    sequence: u64,
    rank: u64,
) -> Result<Vec<u8>, ConversationStoreError> {
    let hole = nodes::hole(sequence, rank, template.provenance_bytes)?;
    let mut value = Vec::with_capacity(
        template.header_prefix.len() + hole.len() + template.header_suffix.len(),
    );
    value.extend_from_slice(template.header_prefix.as_bytes());
    value.extend_from_slice(&hole);
    value.extend_from_slice(template.header_suffix.as_bytes());
    if value.len() > 16384 {
        return Err(storage("captured occurrence header exceeds bound"));
    }
    Ok(value)
}
fn synthetic(path: &[String], start: u64, end: u64, children: u64, object: bool) -> Fact {
    Fact {
        path: path.to_vec(),
        parent: None,
        ordinal: None,
        start,
        end,
        children,
        nodes: 33,
        object,
        container: true,
        header: String::new(),
        key: None,
        name: path
            .last()
            .filter(|part| part.as_str() != "messages")
            .cloned(),
    }
}
fn rank(path: &[String], count: u64) -> Result<u64, ConversationStoreError> {
    let part = path.get(1).ok_or_else(invalid)?;
    let rank: u64 = part.parse().map_err(|_| invalid())?;
    if rank >= count || part != &rank.to_string() {
        return Err(invalid());
    }
    Ok(rank)
}
fn count(root: &ReadRoot) -> u64 {
    root.metadata.native.map_or(0, |node| node.count)
}
fn selected(
    connection: &Connection,
    root: &ReadRoot,
    path: &[String],
) -> Result<Selected, ConversationStoreError> {
    if root.metadata.kind == super::RootKind::Request
        && path.first().is_some_and(|part| part == "messages")
    {
        let messages_end = add(
            root.metadata.messages_start,
            root.metadata.length - (root.metadata.scalar.length() - 2),
        )?;
        if path.len() == 1 {
            return Ok(Selected {
                fact: Fact {
                    name: Some("messages".into()),
                    ..synthetic(
                        path,
                        root.metadata.messages_start,
                        messages_end,
                        count(root),
                        false,
                    )
                },
                start: root.metadata.messages_start,
                end: messages_end,
                override_header: None,
            });
        }
        let rank = rank(path, count(root))?;
        let leaf = nodes::select(
            connection,
            root.metadata.native.ok_or_else(invalid)?,
            root.metadata.source_hash.ok_or_else(invalid)?,
            root.metadata.sequence,
            rank,
        )?;
        let start = add(add(root.metadata.messages_start, 1)?, leaf.start)?;
        let end = add(start, leaf.length)?;
        let suffix = add(
            leaf.template.prefix.length,
            nodes::hole(root.metadata.sequence, rank, leaf.template.provenance_bytes)?.len() as u64,
        )?;
        if path.len() == 2 {
            return Ok(Selected {
                fact: Fact {
                    nodes: leaf.template.nodes,
                    ..synthetic(path, start, end, leaf.template.children, true)
                },
                start,
                end,
                override_header: Some((leaf.template, rank)),
            });
        }
        let relative = &path[2..];
        if relative
            .first()
            .is_some_and(|part| part == "provenance_source")
        {
            let bytes = nodes::hole(root.metadata.sequence, rank, leaf.template.provenance_bytes)?;
            let mut fact = hole_fact(&bytes, &relative[1..])?;
            fact.path = path.to_vec();
            fact.name = path.last().cloned();
            let offset = add(start, leaf.template.prefix.length)?;
            return Ok(Selected {
                start: add(offset, fact.start)?,
                end: add(offset, fact.end)?,
                fact,
                override_header: None,
            });
        }
        if let Some(fact) = fragments::fact(connection, &leaf.template.prefix, relative)? {
            return Ok(Selected {
                start: add(start, fact.start)?,
                end: add(start, fact.end)?,
                fact,
                override_header: None,
            });
        }
        if let Some(fact) = fragments::fact(connection, &leaf.template.suffix, relative)? {
            let start = add(start, suffix)?;
            return Ok(Selected {
                start: add(start, fact.start)?,
                end: add(start, fact.end)?,
                fact,
                override_header: None,
            });
        }
        return Err(invalid());
    }
    let mut fact = root.scalar_fact(connection, path)?.ok_or_else(invalid)?;
    // The scalar prototype contains an empty array. A genuine selected message
    // adds at least the eight typed descriptor fields, so the full Request root
    // (24 direct fields already) necessarily exceeds the 32-node preview gate.
    if path.is_empty() && root.metadata.kind == super::RootKind::Request && count(root) > 0 {
        fact.nodes = 33;
    }
    let delta = root.metadata.length - root.metadata.scalar.length();
    let start = if fact.start >= root.metadata.messages_end {
        add(fact.start, delta)?
    } else {
        fact.start
    };
    let end = if fact.end >= root.metadata.messages_end {
        add(fact.end, delta)?
    } else {
        fact.end
    };
    Ok(Selected {
        fact,
        start,
        end,
        override_header: None,
    })
}
fn hole_fact(bytes: &[u8], path: &[String]) -> Result<Fact, ConversationStoreError> {
    fn find(
        value: &serde_json::Value,
        path: &[String],
        offset: u64,
    ) -> Result<Fact, ConversationStoreError> {
        let raw = serde_json::to_vec(value).map_err(|error| storage(error.to_string()))?;
        if path.is_empty() {
            return Ok(Fact {
                path: Vec::new(),
                parent: None,
                ordinal: None,
                start: offset,
                end: offset + raw.len() as u64,
                children: value.as_object().map_or_else(
                    || value.as_array().map_or(0, Vec::len),
                    serde_json::Map::len,
                ) as u64,
                nodes: super::super::original_node_count(&raw)?,
                object: value.is_object(),
                container: value.is_object() || value.is_array(),
                header: base64::engine::general_purpose::STANDARD.encode(raw),
                key: None,
                name: None,
            });
        }
        let mut position = offset + 1;
        match value {
            serde_json::Value::Object(values) => {
                for (ordinal, (key, value)) in values.iter().enumerate() {
                    let key_bytes =
                        serde_json::to_vec(key).map_err(|error| storage(error.to_string()))?;
                    if ordinal > 0 {
                        position += 1;
                    }
                    position += key_bytes.len() as u64 + 1;
                    if key == &path[0] {
                        return find(value, &path[1..], position);
                    }
                    position += serde_json::to_vec(value)
                        .map_err(|error| storage(error.to_string()))?
                        .len() as u64;
                }
            }
            serde_json::Value::Array(values) => {
                for (ordinal, value) in values.iter().enumerate() {
                    if ordinal > 0 {
                        position += 1;
                    }
                    if path[0] == ordinal.to_string() {
                        return find(value, &path[1..], position);
                    }
                    position += serde_json::to_vec(value)
                        .map_err(|error| storage(error.to_string()))?
                        .len() as u64;
                }
            }
            _ => {}
        }
        Err(invalid())
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| storage("native occurrence JSON unavailable"))?;
    find(&value, path, 0)
}
fn child(
    connection: &Connection,
    root: &ReadRoot,
    path: &[String],
    ordinal: u64,
) -> Result<Vec<String>, ConversationStoreError> {
    if root.metadata.kind == super::RootKind::Request && path == ["messages"] {
        let mut path = path.to_vec();
        path.push(ordinal.to_string());
        return Ok(path);
    }
    if root.metadata.kind == super::RootKind::Request
        && path.first().is_some_and(|part| part == "messages")
        && path.len() >= 2
    {
        let rank = rank(path, count(root))?;
        let leaf = nodes::select(
            connection,
            root.metadata.native.ok_or_else(invalid)?,
            root.metadata.source_hash.ok_or_else(invalid)?,
            root.metadata.sequence,
            rank,
        )?;
        let relative = &path[2..];
        if relative
            .first()
            .is_some_and(|part| part == "provenance_source")
        {
            let bytes = nodes::hole(root.metadata.sequence, rank, leaf.template.provenance_bytes)?;
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| storage("native occurrence JSON unavailable"))?;
            let mut at = &value;
            for component in &relative[1..] {
                at = match at {
                    serde_json::Value::Object(values) => values.get(component),
                    serde_json::Value::Array(values) => component
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| values.get(index)),
                    _ => None,
                }
                .ok_or_else(invalid)?;
            }
            let component = if let Some(values) = at.as_object() {
                values
                    .keys()
                    .nth(usize::try_from(ordinal).map_err(|_| invalid())?)
                    .cloned()
            } else if let Some(values) = at.as_array() {
                (ordinal < values.len() as u64).then(|| ordinal.to_string())
            } else {
                None
            }
            .ok_or_else(invalid)?;
            let mut child = path.to_vec();
            child.push(component);
            return Ok(child);
        }
        if relative.is_empty() && ordinal == 2 {
            let mut child = path.to_vec();
            child.push("provenance_source".into());
            return Ok(child);
        }
        let child = fragments::child(connection, &leaf.template.prefix, relative, ordinal)?
            .or(fragments::child(
                connection,
                &leaf.template.suffix,
                relative,
                ordinal,
            )?)
            .ok_or_else(|| storage("original composed child unavailable"))?;
        let mut path = path[..2].to_vec();
        path.extend(child);
        return Ok(path);
    }
    root.scalar_child(connection, path, ordinal)?
        .ok_or_else(|| storage("original scalar child unavailable"))
}
fn preview(
    connection: &Connection,
    root: &ReadRoot,
    path: &[String],
    budget: &mut usize,
) -> Result<serde_json::Value, ConversationStoreError> {
    if *budget == 0 {
        return Ok(serde_json::Value::Null);
    }
    *budget -= 1;
    let current = selected(connection, root, path)?;
    if current.fact.nodes <= 32 && current.end - current.start <= 16384 {
        *budget = budget.saturating_sub(
            usize::try_from(current.fact.nodes.saturating_sub(1))
                .map_err(|_| storage("preview node count exceeds bound"))?,
        );
        return serde_json::from_slice(&read_exact(connection, root, current.start, current.end)?)
            .map_err(|_| storage("original composed preview unavailable"));
    }
    if !current.fact.container {
        return if current.end - current.start <= 1024 {
            serde_json::from_slice(&read_exact(connection, root, current.start, current.end)?)
                .map_err(|_| storage("original composed scalar preview unavailable"))
        } else {
            Ok(serde_json::Value::Null)
        };
    }
    let mut object = serde_json::Map::new();
    let mut array = Vec::new();
    for ordinal in 0..current.fact.children.min(32) {
        if *budget == 0 {
            break;
        }
        let path = child(connection, root, path, ordinal)?;
        if current.fact.object {
            let name = selected(connection, root, &path)?.fact.name;
            if let Some(name) = name {
                object.insert(name, preview(connection, root, &path, budget)?);
            }
        } else {
            array.push(preview(connection, root, &path, budget)?);
        }
    }
    Ok(if current.fact.object {
        serde_json::Value::Object(object)
    } else {
        serde_json::Value::Array(array)
    })
}
pub(in crate::durable::sqlite::observation_request) fn coordinates(
    connection: &Connection,
    id: &str,
    path: &[String],
) -> Result<(u64, u64, u64), ConversationStoreError> {
    let root = ReadRoot::load(connection, id, path)?;
    let field = selected(connection, &root, path)?;
    Ok((field.start, field.end, field.fact.children))
}
pub(in crate::durable::sqlite::observation_request) fn field(
    connection: &Connection,
    id: &str,
    path: &[String],
) -> Result<super::super::FieldWithKey, ConversationStoreError> {
    let root = ReadRoot::load(connection, id, path)?;
    field_in(connection, &root, path)
}
fn field_in(
    connection: &Connection,
    root: &ReadRoot,
    path: &[String],
) -> Result<super::super::FieldWithKey, ConversationStoreError> {
    let field = selected(connection, root, path)?;
    #[cfg(test)]
    super::WORK.with(|work| {
        let mut value = work.get();
        value.header_decodes += 1;
        work.set(value);
    });
    let header = if let Some((template, rank)) = field.override_header {
        header(&template, root.metadata.sequence, rank)?
    } else if path.is_empty() || path == ["messages"] {
        let value = preview(connection, root, path, &mut 32)?;
        crate::runtime_client::checkpoint::bounded_bytes(&value, 16384)
            .unwrap_or_else(|| b"null".to_vec())
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(&field.fact.header)
            .map_err(|_| storage("composed field header encoding unavailable"))?
    };
    if header.len() > 16384 {
        return Err(storage("composed field header exceeds bound"));
    }
    let key = field
        .fact
        .key
        .map(|encoded_bytes| {
            let mut key_path = path.to_vec();
            key_path.push("$key".into());
            crate::runtime_client::checkpoint::bounded_bytes(
                &crate::durable::observation::RequestObservationKey {
                    path: key_path,
                    preview: String::new(),
                    encoded_bytes,
                },
                4096,
            )
            .ok_or_else(|| storage("composed key descriptor exceeds bound"))
        })
        .transpose()?;
    Ok((field.start, field.end, field.fact.children, header, key))
}
pub(in crate::durable::sqlite::observation_request) fn page(
    connection: &Connection,
    id: &str,
    path: &[String],
    offset: u64,
    limit: usize,
) -> Result<RequestObservationPage, ConversationStoreError> {
    if limit == 0 || limit > 32 {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let root = ReadRoot::load(connection, id, path)?;
    let selected = selected(connection, &root, path)?;
    let total = selected.fact.children;
    if offset > total {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let mut fields = Vec::with_capacity(limit);
    for ordinal in offset..offset.saturating_add(limit as u64).min(total) {
        let path = child(connection, &root, path, ordinal)?;
        let (start, end, children_total, bytes, key) = field_in(connection, &root, &path)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| storage("composed field header JSON unavailable"))?;
        let content =
            super::super::metadata_list("request_source_roots", &path, &bytes, "content")?;
        let collections =
            super::super::metadata_list("request_source_roots", &path, &bytes, "collections")?;
        let deferred = end - start > bytes.len() as u64
            || super::super::message_descriptor_deferred("request_source_roots", &path, &value);
        fields.push(RequestObservationField {
            path,
            value,
            key: key
                .map(|bytes| {
                    serde_json::from_slice(&bytes)
                        .map_err(|_| storage("composed key JSON unavailable"))
                })
                .transpose()?,
            content,
            collections,
            encoded_bytes: end - start,
            children_total,
            deferred,
        });
    }
    while crate::runtime_client::checkpoint::bounded_bytes(&fields, 524_288).is_none() {
        if fields.len() <= 1 {
            return Err(storage("composed field page exceeds bound"));
        }
        fields.pop();
    }
    let next = offset + fields.len() as u64;
    if next == offset && next < total {
        return Err(storage("composed field page cannot advance"));
    }
    Ok(RequestObservationPage {
        total,
        offset,
        fields,
        next_offset: (next < total).then_some(next),
    })
}
fn leaf_piece(
    connection: &Connection,
    template: &Template,
    sequence: u64,
    rank: u64,
    offset: u64,
    max: usize,
) -> Result<Vec<u8>, ConversationStoreError> {
    let hole = nodes::hole(sequence, rank, template.provenance_bytes)?;
    if offset < template.prefix.length {
        return fragments::bytes(
            connection,
            &template.prefix,
            offset,
            (offset + max as u64).min(template.prefix.length),
        );
    }
    let at = offset - template.prefix.length;
    if at < hole.len() as u64 {
        let at =
            usize::try_from(at).map_err(|_| storage("native occurrence offset exceeds bound"))?;
        return Ok(hole[at..(at + max).min(hole.len())].to_vec());
    }
    let at = at - hole.len() as u64;
    fragments::bytes(
        connection,
        &template.suffix,
        at,
        (at + max as u64).min(template.suffix.length),
    )
}
fn sequence_piece(
    connection: &Connection,
    root: &ReadRoot,
    mut offset: u64,
    max: usize,
) -> Result<Vec<u8>, ConversationStoreError> {
    if offset == 0 {
        return Ok(vec![b'[']);
    }
    offset -= 1;
    let Some(mut native) = root.metadata.native else {
        return Ok(vec![b']']);
    };
    let mut digest = root.metadata.source_hash.ok_or_else(invalid)?;
    let mut rank = 0;
    let size = nodes::length(connection, native, digest, rank, root.metadata.sequence)?;
    if offset == size {
        return Ok(vec![b']']);
    }
    for _ in 0..super::manifest::MAX_HEIGHT {
        let node = nodes::load(connection, native, digest)?;
        match node.content {
            Content::Leaf { leaf } => {
                let class = super::weights::class(root.metadata.sequence, rank)?;
                let template = fragments::load_template(
                    connection,
                    *leaf
                        .templates
                        .get(usize::from(*leaf.variants.get(class).ok_or_else(invalid)?))
                        .ok_or_else(invalid)?,
                )?;
                return leaf_piece(
                    connection,
                    &template,
                    root.metadata.sequence,
                    rank,
                    offset,
                    max,
                );
            }
            Content::Branch {
                left,
                left_hash,
                right,
                right_hash,
            } => {
                let length =
                    nodes::length(connection, left, left_hash, rank, root.metadata.sequence)?;
                match offset.cmp(&length) {
                    std::cmp::Ordering::Less => {
                        native = left;
                        digest = left_hash;
                    }
                    std::cmp::Ordering::Equal => return Ok(vec![b',']),
                    std::cmp::Ordering::Greater => {
                        offset -= length + 1;
                        rank += left.count;
                        native = right;
                        digest = right_hash;
                    }
                }
            }
        }
    }
    Err(storage("composed byte descent exceeds native height"))
}
fn piece(
    connection: &Connection,
    root: &ReadRoot,
    offset: u64,
    max: usize,
) -> Result<Vec<u8>, ConversationStoreError> {
    #[cfg(test)]
    if root.metadata.kind == super::RootKind::Scalar {
        return root.scalar_bytes(
            connection,
            offset,
            (offset + max as u64).min(root.metadata.length),
        );
    }
    if offset < root.metadata.messages_start {
        return root.scalar_bytes(
            connection,
            offset,
            (offset + max as u64).min(root.metadata.messages_start),
        );
    }
    let messages_length = root.metadata.length - (root.metadata.scalar.length() - 2);
    if offset < root.metadata.messages_start + messages_length {
        return sequence_piece(connection, root, offset - root.metadata.messages_start, max);
    }
    let at = offset - (root.metadata.length - root.metadata.scalar.length());
    root.scalar_bytes(
        connection,
        at,
        (at + max as u64).min(root.metadata.scalar.length()),
    )
}
fn read_exact(
    connection: &Connection,
    root: &ReadRoot,
    start: u64,
    end: u64,
) -> Result<Vec<u8>, ConversationStoreError> {
    if start > end || end > root.metadata.length || end - start > 65536 {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(end - start).map_err(|_| storage("composed range exceeds bound"))?,
    );
    let mut pieces = 0;
    while start + (bytes.len() as u64) < end {
        pieces += 1;
        if pieces > 512 {
            return Err(storage("composed preview exceeds bounded pieces"));
        }
        let at = start + bytes.len() as u64;
        let value = piece(
            connection,
            root,
            at,
            usize::try_from(end - at).map_err(|_| storage("composed range exceeds bound"))?,
        )?;
        if value.is_empty() {
            return Err(storage("composed range cannot advance"));
        }
        bytes.extend(value);
    }
    Ok(bytes)
}
pub(in crate::durable::sqlite::observation_request) fn range(
    connection: &Connection,
    id: &str,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<ObservationValueChunk, ConversationStoreError> {
    if max_bytes == 0 || max_bytes > 65536 {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let root = ReadRoot::load(connection, id, path)?;
    let field = selected(connection, &root, path)?;
    let readable_end = field.end - field.start;
    if offset > readable_end {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let end = offset
        .saturating_add(u64::from(max_bytes))
        .min(readable_end);
    let mut bytes = Vec::with_capacity(
        usize::try_from(end - offset).map_err(|_| storage("composed range exceeds bound"))?,
    );
    for _ in 0..64 {
        let at = offset + bytes.len() as u64;
        if at == end {
            break;
        }
        let part = piece(
            connection,
            &root,
            field.start + at,
            usize::try_from(end - at).map_err(|_| storage("composed range exceeds bound"))?,
        )?;
        if part.is_empty() {
            return Err(storage("composed range cannot advance"));
        }
        bytes.extend(part);
    }
    let end_offset = offset + bytes.len() as u64;
    Ok(ObservationValueChunk {
        bytes,
        end_offset,
        readable_end,
        next_offset: (end_offset < readable_end).then_some(end_offset),
    })
}
