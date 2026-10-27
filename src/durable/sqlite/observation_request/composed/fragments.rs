use super::{
    Connection, ConversationStoreError, Fact, Fragment, Leaf, Template, bounded, index, params,
    storage, weights,
};
use crate::runtime::read_layout::{
    FieldPreviewPolicy, ObservationByteReader, SeekableObservationSource,
};
use base64::Engine as _;
const CHUNK: u64 = 8192;
#[path = "fact_spool.rs"]
mod spool;
use spool::FactSpool;
#[cfg(test)]
#[path = "child_reference_tests.rs"]
mod child_reference_tests;
#[cfg(test)]
#[path = "metadata_tests.rs"]
mod metadata_tests;
#[cfg(test)]
#[path = "reader_tests.rs"]
mod reader_tests;
pub(super) fn geometry(
    source: &SeekableObservationSource,
    path: &[String],
) -> Result<(u64, u64, u64), ConversationStoreError> {
    let mut selected = None;
    source
        .visit_fields_selecting_previews(
            |_| FieldPreviewPolicy::Skip,
            |candidate, _, _, start, end, children, _, _| {
                if candidate == path {
                    selected = Some((start, end, children));
                }
                Ok(())
            },
        )
        .map_err(storage)?;
    selected.ok_or_else(|| storage("composed producer field unavailable"))
}
fn chunk_key(offset: u64) -> String {
    format!("{offset:020}")
}
fn child_key(parent: &[String], ordinal: u64) -> Result<String, ConversationStoreError> {
    Ok(format!(
        "C{}:{ordinal:020}",
        super::super::encoded_path(parent)?
    ))
}
fn field_key(path: &[String]) -> Result<String, ConversationStoreError> {
    Ok(format!("F{}", super::super::field_path(path)?))
}
fn source_bytes(
    reader: &ObservationByteReader<'_>,
    start: u64,
    end: u64,
) -> Result<Vec<u8>, ConversationStoreError> {
    let count = end
        .checked_sub(start)
        .filter(|count| *count <= 65536)
        .ok_or_else(|| storage("composed producer byte range exceeds bound"))?;
    let bytes = reader
        .indexed_span(
            start,
            end,
            u32::try_from(count.max(1)).map_err(|_| storage("producer range exceeds bound"))?,
        )
        .map_err(|error| storage(error.to_string()))?;
    if bytes.len() as u64 != count {
        return Err(storage("composed producer bytes incomplete"));
    }
    Ok(bytes)
}
fn persist_key_fact(
    spool: &mut FactSpool<'_>,
    path: &[String],
    start: u64,
    end: u64,
) -> Result<u64, ConversationStoreError> {
    let mut path = path.to_vec();
    path.push("$key".into());
    let length = super::super::encoded_path(&path)?.len() as u64;
    persist_fact(
        spool,
        &Fact {
            path,
            parent: None,
            ordinal: None,
            start,
            end,
            children: 0,
            nodes: 1,
            object: false,
            container: false,
            header: base64::engine::general_purpose::STANDARD.encode(b"\"\""),
            key: None,
            name: None,
        },
    )?;
    Ok(length)
}
fn capture_facts(
    spool: &mut FactSpool<'_>,
    source: &SeekableObservationSource,
    reader: &ObservationByteReader<'_>,
    start: u64,
    end: u64,
) -> Result<(u64, u64), ConversationStoreError> {
    let mut failure = None;
    let mut max_path = 0_u64;
    let mut max_key_path = 0_u64;
    let mut validated_subtree = None;
    let result = source.visit_field_metadata(start, end, |field, value| {
        let result = (|| {
            let path = field.path;
            let key_span = field.key_span;
            let field_start = field.start;
            let field_end = field.end;
            max_path = max_path.max(field.encoded_path.len() as u64);
            let header = crate::runtime_client::checkpoint::bounded_bytes(value, 16384)
                .unwrap_or_else(|| b"null".to_vec());
            let span = field_end - field_start;
            let nodes = if span > 16384 {
                33
            } else if field.recursive_nodes <= 32 {
                // The full preview just validated this complete bounded subtree.
                field.recursive_nodes
            } else if validated_subtree
                .is_some_and(|(start, end)| field_start >= start && field_end <= end)
            {
                // Both the Source parser and original_node_count cap at 33.
                // A fully validated ancestor already proves this descendant's
                // numeric/syntax limits and stricter relative serde depth.
                33
            } else {
                // A truncated preview alone does not validate the full subtree.
                // Preorder traversal lets descendants reuse this successful
                // bounded validation without retaining bytes or changing facts.
                let nodes = super::super::original_node_count(&source_bytes(
                    reader,
                    field_start,
                    field_end,
                )?)?;
                validated_subtree = Some((field_start, field_end));
                nodes
            };
            let fact = Fact {
                path: path.to_vec(),
                parent: field.parent.map(|(path, _, _)| path.to_vec()),
                ordinal: field.parent.map(|(_, _, ordinal)| ordinal),
                start: field_start - start,
                end: field_end - start,
                children: field.children,
                nodes,
                object: field.object,
                container: field.container,
                header: base64::engine::general_purpose::STANDARD.encode(header),
                key: None,
                name: key_span
                    .filter(|(start, end)| end - start <= 256)
                    .map(|_| {
                        field
                            .key
                            .map(str::to_owned)
                            .ok_or_else(|| storage("original field key unavailable"))
                    })
                    .transpose()?,
            };
            let fact = if path.last().is_some_and(|part| part.starts_with('@')) {
                Fact {
                    key: Some(
                        key_span
                            .map(|(start, end)| end - start)
                            .ok_or_else(|| storage("original aliased key span unavailable"))?,
                    ),
                    ..fact
                }
            } else {
                fact
            };
            persist_encoded_fact(
                spool,
                &fact,
                field.encoded_path,
                field.parent.map(|(_, encoded, ordinal)| (encoded, ordinal)),
            )?;
            if let Some((key_start, key_end)) = key_span
                && path.last().is_some_and(|part| part.starts_with('@'))
            {
                max_key_path = max_key_path.max(persist_key_fact(
                    spool,
                    path,
                    key_start - start,
                    key_end - start,
                )?);
            }
            Ok(())
        })();
        result.map_err(|error| {
            failure = Some(error);
            "composed fact capture failed"
        })
    });
    result.map_err(|error| failure.unwrap_or_else(|| storage(error)))?;
    Ok((max_path, max_key_path))
}
pub(super) fn persist_part(
    connection: &Connection,
    source: &SeekableObservationSource,
    reader: &ObservationByteReader<'_>,
    start: u64,
    end: u64,
) -> Result<Fragment, ConversationStoreError> {
    let mut spool = FactSpool::new(connection);
    let (max_path, max_key_path) = capture_facts(&mut spool, source, reader, start, end)?;
    let fields = spool.finish()?;
    let mut builder = index::Builder::new(connection);
    let mut offset = start;
    while offset < end {
        let through = (offset + CHUNK).min(end);
        builder.push(
            chunk_key(offset - start),
            source_bytes(reader, offset, through)?,
        )?;
        offset = through;
    }
    let bytes = builder
        .finish()?
        .ok_or_else(|| storage("empty composed byte fragment"))?;
    #[cfg(test)]
    super::WORK.with(|work| {
        let mut value = work.get();
        value.new_fragment_bytes += usize::try_from(end - start).unwrap_or(usize::MAX);
        work.set(value);
    });
    Ok(Fragment {
        bytes,
        fields,
        length: end - start,
        max_path,
        max_key_path,
    })
}
fn persist_fact(spool: &mut FactSpool<'_>, fact: &Fact) -> Result<(), ConversationStoreError> {
    let encoded = super::super::field_path(&fact.path)?;
    let parent = fact
        .parent
        .as_ref()
        .map(|path| super::super::encoded_path(path))
        .transpose()?;
    persist_encoded_fact(spool, fact, &encoded, parent.as_deref().zip(fact.ordinal))
}
fn persist_encoded_fact(
    spool: &mut FactSpool<'_>,
    fact: &Fact,
    encoded: &str,
    parent: Option<(&str, u64)>,
) -> Result<(), ConversationStoreError> {
    // The source traversal supplies canonical JSON encoding. Preserve the
    // request locator's independent component/count/encoded-byte limits.
    if fact.path.len() > 256
        || fact.path.iter().any(|part| part.len() > 256)
        || encoded.len() > 4096
    {
        return Err(ConversationStoreError::InvalidReference(
            "request field locator exceeds bound".into(),
        ));
    }
    let bytes = bounded(fact, 40960)?;
    let field = format!("F{encoded}");
    spool.push(field, bytes)?;
    if let Some((parent, ordinal)) = parent {
        // The complete Fact remains in F; C stores only its exact encoded path.
        spool.push(
            format!("C{parent}:{ordinal:020}"),
            encoded.as_bytes().to_vec(),
        )?;
    }
    Ok(())
}
#[cfg(test)]
pub(super) fn scalar(
    connection: &Connection,
    source: &SeekableObservationSource,
) -> Result<(Fragment, u64, u64), ConversationStoreError> {
    let (start, end, _) = geometry(source, &["messages".into()])?;
    let length = source
        .read(&[], 0, 1)
        .map_err(|error| storage(error.to_string()))?
        .readable_end;
    let reader = source
        .byte_reader()
        .map_err(|error| storage(error.to_string()))?;
    Ok((
        persist_part(connection, source, &reader, 0, length)?,
        start,
        end,
    ))
}
fn header_parts(value: &serde_json::Value) -> Result<(String, String), ConversationStoreError> {
    let bytes = bounded(value, 16384)?;
    let marker = b",\"provenance_source\":";
    let start = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .ok_or_else(|| storage("native provenance source key unavailable"))?
        + marker.len();
    // The typed field's encoded value is exact and has a bounded structural form.
    let hole = serde_json::to_vec(&value["provenance_source"])
        .map_err(|error| storage(error.to_string()))?;
    let end = start
        .checked_add(hole.len())
        .filter(|end| *end <= bytes.len() && bytes[start..*end] == hole)
        .ok_or_else(|| storage("native provenance source geometry differs"))?;
    Ok((
        String::from_utf8(bytes[..start].to_vec())
            .map_err(|_| storage("header prefix UTF8 unavailable"))?,
        String::from_utf8(bytes[end..].to_vec())
            .map_err(|_| storage("header suffix UTF8 unavailable"))?,
    ))
}
struct RawMessage<'a> {
    value: &'a serde_json::Value,
    message: &'a crate::model::ModelInputMessage,
    view: &'a crate::runtime_client::trace::TraceRequestMessage,
    frozen: &'a crate::model::projected::FrozenModelInput,
}
impl schemars::JsonSchema for RawMessage<'_> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        <crate::runtime_client::trace::TraceRequestMessage as schemars::JsonSchema>::schema_name()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <crate::runtime_client::trace::TraceRequestMessage as schemars::JsonSchema>::json_schema(
            generator,
        )
    }
}
impl serde::Serialize for RawMessage<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.message {
            crate::model::ModelInputMessage::Canonical(_) => serde::Serialize::serialize(
                &super::super::provenance::Canonical {
                    value: self.value,
                    provenance: super::super::provenance::Raw::frozen(
                        self.frozen,
                        &self.view.provenance,
                    ),
                },
                serializer,
            ),
            _ => serde::Serialize::serialize(
                &super::super::input::Input {
                    original: self.message,
                    view: self.view,
                },
                serializer,
            ),
        }
    }
}
fn display(
    view: &crate::runtime_client::trace::TraceRequestMessage,
    header: Option<&crate::durable::observation::MessageHeader>,
) -> Result<serde_json::Value, ConversationStoreError> {
    match header {
        Some(header) => super::super::message_value_capture(header, view).map(|result| result.0),
        None => super::super::bounded_message_descriptor(view, view.blocks.len(), false),
    }
}
fn template_node_count(
    reader: &ObservationByteReader<'_>,
    total: u64,
) -> Result<u64, ConversationStoreError> {
    if total <= 16384 {
        super::super::original_node_count(&source_bytes(reader, 0, total)?)
    } else {
        Ok(33)
    }
}
fn capture_suffix(
    connection: &Connection,
    value: &serde_json::Value,
) -> Result<Fragment, ConversationStoreError> {
    let source = SeekableObservationSource::new(value).map_err(storage)?;
    let (_, end, _) = geometry(&source, &["provenance_source".into()])?;
    let total = source
        .read(&[], 0, 1)
        .map_err(|error| storage(error.to_string()))?
        .readable_end;
    let reader = source
        .byte_reader()
        .map_err(|error| storage(error.to_string()))?;
    persist_part(connection, &source, &reader, end, total)
}
pub(super) fn capture(
    connection: &Connection,
    scope: &crate::runtime::identity::ConversationId,
    message: &crate::model::ModelInputMessage,
    frozen: &crate::model::projected::FrozenModelInput,
) -> Result<Leaf, ConversationStoreError> {
    let (mut view, header) = super::super::input_view(connection, scope, message, frozen)?;
    let provenance_bytes =
        super::super::provenance::Raw::frozen(frozen, &view.provenance).encoded_bytes()?;
    view.provenance_source = Some(super::super::provenance_source(1, 0, provenance_bytes));
    let minimum = display(&view, header.as_ref())?;
    let source = SeekableObservationSource::new(&RawMessage {
        value: &minimum,
        message,
        view: &view,
        frozen,
    })
    .map_err(storage)?;
    let (hole_start, hole_end, _) = geometry(&source, &["provenance_source".into()])?;
    let total = source
        .read(&[], 0, 1)
        .map_err(|error| storage(error.to_string()))?
        .readable_end;
    let reader = source
        .byte_reader()
        .map_err(|error| storage(error.to_string()))?;
    let prefix = persist_part(connection, &source, &reader, 0, hole_start)?;
    let suffix = persist_part(connection, &source, &reader, hole_end, total)?;
    let (header_prefix, header_suffix) = header_parts(&minimum)?;
    let template = Template {
        prefix,
        suffix,
        header_prefix,
        header_suffix,
        provenance_bytes,
        children: minimum.as_object().map_or(0, serde_json::Map::len) as u64,
        nodes: template_node_count(&reader, total)?,
    };
    view.provenance_source = Some(super::super::provenance_source(
        u64::MAX,
        8191,
        provenance_bytes,
    ));
    let maximum = display(&view, header.as_ref())?;
    let max_parts = header_parts(&maximum)?;
    if max_parts
        == (
            template.header_prefix.clone(),
            template.header_suffix.clone(),
        )
    {
        return Ok(Leaf {
            variants: vec![0; weights::CLASSES],
            templates: vec![store_template(connection, &template)?],
        });
    }
    let mut templates = vec![template];
    let mut leaf = Leaf {
        variants: vec![0],
        templates: Vec::new(),
    };
    for class in 1..weights::CLASSES {
        let (sequence, rank) = weights::representative(class)?;
        view.provenance_source = Some(super::super::provenance_source(
            sequence,
            rank,
            provenance_bytes,
        ));
        let value = display(&view, header.as_ref())?;
        let parts = header_parts(&value)?;
        if let Some(index) = templates.iter().position(|template| {
            template.header_prefix == parts.0 && template.header_suffix == parts.1
        }) {
            leaf.variants
                .push(u8::try_from(index).map_err(|_| storage("native variant count invalid"))?);
            continue;
        }
        let suffix = if header.is_some() {
            capture_suffix(connection, &value)?
        } else {
            templates[0].suffix.clone()
        };
        leaf.variants.push(
            u8::try_from(templates.len()).map_err(|_| storage("native variant count invalid"))?,
        );
        templates.push(Template {
            prefix: templates[0].prefix.clone(),
            suffix,
            header_prefix: parts.0,
            header_suffix: parts.1,
            provenance_bytes,
            children: value.as_object().map_or(0, serde_json::Map::len) as u64,
            nodes: 33,
        });
    }
    for template in templates {
        leaf.templates.push(store_template(connection, &template)?);
    }
    Ok(leaf)
}
pub(super) fn fact(
    connection: &Connection,
    fragment: &Fragment,
    path: &[String],
) -> Result<Option<Fact>, ConversationStoreError> {
    index::get(connection, &fragment.fields, &field_key(path)?)?
        .map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|_| storage("fragment field metadata malformed"))
        })
        .transpose()
}
pub(super) fn child(
    connection: &Connection,
    fragment: &Fragment,
    path: &[String],
    ordinal: u64,
) -> Result<Option<Vec<String>>, ConversationStoreError> {
    index::get(connection, &fragment.fields, &child_key(path, ordinal)?)?
        .map(|bytes| child_path(&bytes, path))
        .transpose()
}
fn child_path(bytes: &[u8], parent: &[String]) -> Result<Vec<String>, ConversationStoreError> {
    if bytes.len() > 4096 {
        return Err(storage("fragment child path exceeds bound"));
    }
    let path: Vec<String> =
        serde_json::from_slice(bytes).map_err(|_| storage("fragment child path malformed"))?;
    if path.len() != parent.len().saturating_add(1)
        || !path.starts_with(parent)
        || super::super::field_path(&path)
            .map_err(|_| storage("fragment child path exceeds bound"))?
            .as_bytes()
            != bytes
    {
        return Err(storage("fragment child path evidence differs"));
    }
    Ok(path)
}
pub(super) fn bytes(
    connection: &Connection,
    fragment: &Fragment,
    start: u64,
    end: u64,
) -> Result<Vec<u8>, ConversationStoreError> {
    if start > end || end > fragment.length || end - start > 65536 {
        return Err(storage("fragment byte range exceeds bound"));
    }
    let mut output = Vec::with_capacity(
        usize::try_from(end - start).map_err(|_| storage("fragment byte range exceeds bound"))?,
    );
    let mut offset = start;
    while offset < end {
        let chunk_start = offset / CHUNK * CHUNK;
        let bytes = index::get(connection, &fragment.bytes, &chunk_key(chunk_start))?
            .ok_or_else(|| storage("original fragment segment unavailable"))?;
        let expected = (fragment.length - chunk_start).min(CHUNK);
        if bytes.len() as u64 != expected {
            return Err(storage("original fragment segment length differs"));
        }
        let through = end.min(chunk_start + expected);
        output.extend_from_slice(
            &bytes[usize::try_from(offset - chunk_start)
                .map_err(|_| storage("fragment offset invalid"))?
                ..usize::try_from(through - chunk_start)
                    .map_err(|_| storage("fragment offset invalid"))?],
        );
        offset = through;
    }
    Ok(output)
}

pub(super) fn store_template(
    connection: &Connection,
    template: &Template,
) -> Result<index::Hash, ConversationStoreError> {
    index::store_blob(connection, &bounded(template, 65536)?)
}
pub(super) fn load_template(
    connection: &Connection,
    digest: index::Hash,
) -> Result<Template, ConversationStoreError> {
    #[cfg(test)]
    super::WORK.with(|work| {
        let mut value = work.get();
        value.template_loads += 1;
        work.set(value);
    });
    let bytes = index::load_blob(connection, digest)?;
    let template: Template = serde_json::from_slice(&bytes)
        .map_err(|_| storage("composed message template malformed"))?;
    if template.header_prefix.len() + template.header_suffix.len() > 16384
        || template.children > 64
        || template.prefix.length > super::SAFE
        || template.suffix.length > super::SAFE
    {
        return Err(storage("composed message template exceeds bound"));
    }
    Ok(template)
}

#[cfg(test)]
pub(super) fn whole(
    connection: &Connection,
    source: &SeekableObservationSource,
) -> Result<Fragment, ConversationStoreError> {
    let length = source
        .read(&[], 0, 1)
        .map_err(|error| storage(error.to_string()))?
        .readable_end;
    let reader = source
        .byte_reader()
        .map_err(|error| storage(error.to_string()))?;
    persist_part(connection, source, &reader, 0, length)
}
