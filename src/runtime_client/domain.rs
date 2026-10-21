//! Bounded value reads over immutable native display-domain values.
//! Selection and encoding happen after the Projection lock releases its Arc.
use super::checkpoint::CheckpointValueChunk;
use super::types::RuntimeClientError;
use crate::observation::cut::CheckpointReadCut;
use base64::Engine as _;
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) enum DomainSource {
    AttemptModel(Arc<SourceValue<crate::model::session::AttemptModelReadSource>>),
    Model(Arc<SourceValue<crate::model::session::SessionModelReadSource>>),
    Catalog(Arc<SourceValue<crate::model::session::ModelCatalogReadSource>>),
    Capability(Arc<SourceValue<crate::observation::capability_source::CapabilityReadSource>>),
    Resources(Arc<SourceValue<crate::observation::resource_source::ResourceReadSource>>),
    Workflow(Arc<SourceValue<crate::observation::workflow_source::WorkflowReadSource>>),
    Context(Arc<SourceValue<super::snapshot::RuntimeClientContextView>>),
    Goal(Arc<SourceValue<crate::observation::goal::RuntimeClientGoalView>>),
    Todo(Arc<SourceValue<crate::tools::todo::TodoSnapshot>>),
    Job(Arc<SourceValue<super::snapshot::RuntimeClientJob>>),
    Agent(Arc<SourceValue<super::snapshot::RuntimeClientAgent>>),
}
impl DomainSource {
    pub(crate) fn encoded_weight(&self) -> usize {
        // Indexed sources retain no open reader or SQLite cache while idle.
        // Reserve one reader's configured 1 MiB cache per retained handle;
        // the existing 16 MiB source budget therefore also bounds concurrent
        // cache potential without scanning or cloning original native values.
        1024 * 1024 + std::mem::size_of_val(self)
    }

    pub(crate) fn page(
        &self,
        cut: &CheckpointReadCut,
        domain: &str,
        path: &[String],
        offset: u64,
        limit: usize,
        singleton: bool,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        macro_rules! page {
            ($source:expr) => {
                source_page(
                    $source.as_ref(),
                    cut,
                    domain,
                    path,
                    offset,
                    limit,
                    singleton,
                )
            };
        }
        match self {
            Self::AttemptModel(value) => page!(value),
            Self::Model(value) => page!(value),
            Self::Catalog(value) => catalog_page(value, cut, domain, path, offset, limit),
            Self::Capability(value) => {
                capability_page(value, cut, domain, path, offset, limit, singleton)
            }
            Self::Resources(value) => {
                resource_page(value, cut, domain, path, offset, limit, singleton)
            }
            Self::Workflow(value) => page!(value),
            Self::Context(value) => page!(value),
            Self::Goal(value) => page!(value),
            Self::Todo(value) => page!(value),
            Self::Job(value) => page!(value),
            Self::Agent(value) => page!(value),
        }
    }
    pub(crate) fn read(
        &self,
        cut: &CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<CheckpointValueChunk, RuntimeClientError> {
        if max_bytes == 0
            || max_bytes > 65_536
            || offset > 9_007_199_254_740_991
            || domain.len() > 256
        {
            return Err(invalid());
        }
        let text = match self {
            Self::Model(value) => value.value().selected_text(path),
            Self::AttemptModel(value) => value.value().selected_text(path),
            Self::Catalog(value) => value.value().selected_text(path),
            Self::Capability(value) => path.first().and_then(|field| {
                path.get(1)
                    .and_then(|position| ordinal(position).ok())
                    .and_then(|position| value.value().selected_text(field, position, &path[2..]))
            }),
            _ => None,
        };
        if let Some(text) = text {
            let chunk = crate::runtime::read_layout::text_chunk(text, offset, max_bytes)
                .map_err(source_error)?;
            return Ok(CheckpointValueChunk {
                representation: super::checkpoint::CheckpointValueRepresentation::Utf8Text,
                cut: cut.clone(),
                domain: domain.into(),
                item_offset,
                path: path.to_vec(),
                offset,
                bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
                next_offset: chunk.next_offset,
                readable_end: chunk.readable_end,
            });
        }
        macro_rules! read {
            ($value:expr) => {
                read_source(
                    $value.as_ref(),
                    cut,
                    domain,
                    item_offset,
                    path,
                    offset,
                    max_bytes,
                )
            };
        }
        match self {
            Self::AttemptModel(value) => read!(value),
            Self::Model(value) => read!(value),
            Self::Catalog(value) => {
                let (layout, remaining) = catalog_row(value, path)?;
                Ok(chunk_result(
                    layout
                        .read(remaining, offset, max_bytes)
                        .map_err(source_error)?,
                    cut,
                    domain,
                    item_offset,
                    path,
                    offset,
                ))
            }
            Self::Capability(value) => {
                let (layout, remaining) = capability_row(value, path)?;
                Ok(chunk_result(
                    layout
                        .read(remaining, offset, max_bytes)
                        .map_err(source_error)?,
                    cut,
                    domain,
                    item_offset,
                    path,
                    offset,
                ))
            }
            Self::Resources(value) => {
                resource_read(value, cut, domain, item_offset, path, offset, max_bytes)
            }
            Self::Workflow(value) => read!(value),
            Self::Context(value) => read!(value),
            Self::Goal(value) => read!(value),
            Self::Todo(value) => read!(value),
            Self::Job(value) => read!(value),
            Self::Agent(value) => read!(value),
        }
    }
}
/// Immutable native authority and its once-built seekable read layout.
/// The layout belongs to this exact source generation and disappears with it.
pub(crate) struct SourceValue<T> {
    value: Arc<T>,
    layout: crate::runtime::read_layout::ObservationLayoutCache,
    rows: std::sync::Mutex<
        std::collections::VecDeque<(
            Vec<String>,
            Arc<crate::runtime::read_layout::SeekableObservationSource>,
        )>,
    >,
}
impl<T> SourceValue<T> {
    pub(crate) fn new(value: Arc<T>) -> Self {
        Self {
            value,
            layout: Arc::new(std::sync::OnceLock::new()),
            rows: std::sync::Mutex::default(),
        }
    }
    pub(crate) fn value(&self) -> &T {
        self.value.as_ref()
    }
    fn row<R: Serialize + schemars::JsonSchema>(
        &self,
        key: Vec<String>,
        value: &R,
    ) -> Result<Arc<crate::runtime::read_layout::SeekableObservationSource>, &'static str> {
        if let Some(found) = self
            .rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|(candidate, _)| candidate == &key)
            .map(|(_, layout)| Arc::clone(layout))
        {
            return Ok(found);
        }
        // Native values are immutable at this source generation. Construction
        // and filesystem work never hold the cache or semantic owner lock.
        let candidate = Arc::new(crate::runtime::read_layout::SeekableObservationSource::new(
            value,
        )?);
        let mut rows = self
            .rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, existing)) = rows.iter().find(|(candidate, _)| candidate == &key) {
            return Ok(Arc::clone(existing));
        }
        if rows.len() == 8 {
            rows.pop_front();
        }
        rows.push_back((key, Arc::clone(&candidate)));
        Ok(candidate)
    }
}
impl<T: Serialize + schemars::JsonSchema> SourceValue<T> {
    pub(crate) fn indexed(
        &self,
    ) -> Result<&crate::runtime::read_layout::SeekableObservationSource, &'static str> {
        self.layout
            .get_or_init(|| {
                crate::runtime::read_layout::SeekableObservationSource::new(self.value())
            })
            .as_ref()
            .map_err(|error| *error)
    }
    pub(crate) fn page(
        &self,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<
        crate::runtime::read_layout::ObservationPage,
        crate::runtime::read_layout::ObservationReadError,
    > {
        self.indexed()?.page(path, offset, limit)
    }
    pub(crate) fn header(
        &self,
        path: &[String],
    ) -> Result<Option<serde_json::Value>, crate::runtime::read_layout::ObservationReadError> {
        self.indexed()?.value(path)
    }
    pub(crate) fn read(
        &self,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<
        crate::runtime::read_layout::ObservationChunk,
        crate::runtime::read_layout::ObservationReadError,
    > {
        self.indexed()?.read(path, offset, max_bytes)
    }
}
pub(crate) fn read_source<T: Serialize + schemars::JsonSchema>(
    value: &SourceValue<T>,
    cut: &CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<CheckpointValueChunk, RuntimeClientError> {
    if max_bytes == 0 || max_bytes > 65_536 || offset > 9_007_199_254_740_991 || domain.len() > 256
    {
        return Err(invalid());
    }
    let chunk = value.read(path, offset, max_bytes).map_err(source_error)?;
    Ok(CheckpointValueChunk {
        representation: super::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: path.to_vec(),
        offset,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        next_offset: chunk.next_offset,
        readable_end: chunk.readable_end,
    })
}
fn invalid() -> RuntimeClientError {
    RuntimeClientError::InvalidRequest {
        message: "invalid bounded domain value range".into(),
    }
}
pub(crate) fn source_error(
    error: impl Into<crate::runtime::read_layout::ObservationReadError>,
) -> RuntimeClientError {
    RuntimeClientError::observation_read_error(error.into())
}

fn source_page<T: Serialize + schemars::JsonSchema>(
    source: &SourceValue<T>,
    cut: &CheckpointReadCut,
    domain: &str,
    path: &[String],
    offset: u64,
    limit: usize,
    singleton: bool,
) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
    use super::checkpoint::CheckpointPage;
    use crate::observation::cut::{CheckpointKey, CheckpointPageItem, CollectionFrontier};
    if limit == 0 || limit > 32 || domain.len() > 256 {
        return Err(invalid());
    }
    let mut collections = Vec::new();
    let (total, items, next_offset) = if singleton {
        if offset > 1 {
            return Err(invalid());
        }
        let value = source.header(path).map_err(source_error)?;
        let items = if offset == 0 {
            vec![CheckpointPageItem {
                body_complete: None,
                key: None,
                offset: 0,
                identity: domain.into(),
                deferred: value.is_none(),
                value,
                content: Vec::new(),
            }]
        } else {
            Vec::new()
        };
        (1, items, None)
    } else {
        let page = source.page(path, offset, limit).map_err(source_error)?;
        let items = page
            .fields
            .into_iter()
            .map(|field| {
                if field.children > 0 {
                    collections.push(CollectionFrontier {
                        domain: format!("{domain}.{}", field.ordinal),
                        revision: cut.revision,
                        total: field.children,
                        loaded: 0,
                        next_offset: Some(0),
                    });
                }
                let key = field
                    .key_bytes
                    .filter(|_| !field.key_complete)
                    .map(|encoded_bytes| CheckpointKey {
                        path: vec!["$key".into()],
                        preview: field.key.clone().unwrap_or_default(),
                        encoded_bytes,
                    });
                CheckpointPageItem {
                    body_complete: None,
                    key,
                    offset: field.ordinal,
                    identity: field
                        .key
                        .unwrap_or_else(|| format!("{domain}:{}", field.ordinal)),
                    deferred: field.value.is_none(),
                    value: field.value,
                    content: Vec::new(),
                }
            })
            .collect();
        (page.total, items, page.next_offset)
    };
    Ok(CheckpointPage {
        collections,
        cursor: super::types::RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: domain.into(),
        total,
        items,
        next_offset,
    })
}

fn ordinal(part: &str) -> Result<u64, RuntimeClientError> {
    part.strip_prefix('@')
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())
}
fn chunk_result(
    chunk: crate::runtime::read_layout::ObservationChunk,
    cut: &CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    offset: u64,
) -> CheckpointValueChunk {
    CheckpointValueChunk {
        representation: super::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: path.to_vec(),
        offset,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        next_offset: chunk.next_offset,
        readable_end: chunk.readable_end,
    }
}
fn bounded_page(
    cut: &CheckpointReadCut,
    domain: &str,
    total: u64,
    items: Vec<crate::observation::cut::CheckpointPageItem>,
    next_offset: Option<u64>,
    collections: Vec<crate::observation::cut::CollectionFrontier>,
) -> super::checkpoint::CheckpointPage {
    super::checkpoint::CheckpointPage {
        cursor: super::types::RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: domain.into(),
        total,
        items,
        next_offset,
        collections,
    }
}
fn row_item<T: Serialize + schemars::JsonSchema>(
    domain: &str,
    offset: u64,
    row: &T,
) -> crate::observation::cut::CheckpointPageItem {
    let value = crate::observation::bounded::structural_value(row);
    crate::observation::cut::CheckpointPageItem {
        body_complete: None,
        key: None,
        offset,
        identity: format!("{domain}:{offset}"),
        deferred: value.is_none(),
        value,
        content: Vec::new(),
    }
}
fn next_page(offset: u64, count: usize, total: u64) -> Option<u64> {
    let next = offset + count as u64;
    (next < total).then_some(next)
}
fn indexed_page(
    layout: &crate::runtime::read_layout::SeekableObservationSource,
    cut: &CheckpointReadCut,
    domain: &str,
    path: &[String],
    offset: u64,
    limit: usize,
) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
    use crate::observation::cut::{CheckpointKey, CheckpointPageItem, CollectionFrontier};
    let page = layout.page(path, offset, limit).map_err(source_error)?;
    let mut collections = Vec::new();
    let items = page
        .fields
        .into_iter()
        .map(|field| {
            if field.children > 0 {
                collections.push(CollectionFrontier {
                    domain: format!("{domain}.{}", field.ordinal),
                    revision: cut.revision,
                    total: field.children,
                    loaded: 0,
                    next_offset: Some(0),
                });
            }
            let key = field
                .key_bytes
                .filter(|_| !field.key_complete)
                .map(|encoded_bytes| CheckpointKey {
                    path: vec!["$key".into()],
                    preview: field.key.clone().unwrap_or_default(),
                    encoded_bytes,
                });
            CheckpointPageItem {
                body_complete: None,
                key,
                offset: field.ordinal,
                identity: field
                    .key
                    .unwrap_or_else(|| format!("{domain}:{}", field.ordinal)),
                deferred: field.value.is_none(),
                value: field.value,
                content: Vec::new(),
            }
        })
        .collect();
    Ok(bounded_page(
        cut,
        domain,
        page.total,
        items,
        page.next_offset,
        collections,
    ))
}
fn catalog_row<'a>(
    source: &SourceValue<crate::model::session::ModelCatalogReadSource>,
    path: &'a [String],
) -> Result<
    (
        Arc<crate::runtime::read_layout::SeekableObservationSource>,
        &'a [String],
    ),
    RuntimeClientError,
> {
    if path.first().map(String::as_str) != Some("models") || path.len() < 2 {
        return Err(invalid());
    }
    let model = ordinal(&path[1])?;
    if path.get(2).map(String::as_str) == Some("profiles") {
        let profile = ordinal(path.get(3).ok_or_else(invalid)?)?;
        let row = source
            .value()
            .profile_at(model, profile)
            .ok_or_else(invalid)?;
        return Ok((
            source.row(path[..4].to_vec(), &row).map_err(source_error)?,
            &path[4..],
        ));
    }
    let row = source.value().model_header_at(model).ok_or_else(invalid)?;
    Ok((
        source.row(path[..2].to_vec(), &row).map_err(source_error)?,
        &path[2..],
    ))
}
fn catalog_page(
    source: &SourceValue<crate::model::session::ModelCatalogReadSource>,
    cut: &CheckpointReadCut,
    domain: &str,
    path: &[String],
    offset: u64,
    limit: usize,
) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
    use crate::observation::cut::CollectionFrontier;
    if limit == 0 || limit > 32 || domain.len() > 256 {
        return Err(invalid());
    }
    if path == ["models"] {
        let total = source.value().model_count() as u64;
        if offset > total {
            return Err(invalid());
        }
        let mut items = Vec::new();
        let mut collections = Vec::new();
        for position in offset..total.min(offset + limit as u64) {
            let row = source
                .value()
                .model_header_at(position)
                .ok_or_else(invalid)?;
            if row.profile_count > 0 {
                collections.push(CollectionFrontier {
                    domain: format!("{domain}.{position}.profiles"),
                    revision: cut.revision,
                    total: row.profile_count,
                    loaded: 0,
                    next_offset: Some(0),
                });
            }
            items.push(row_item(domain, position, &row));
        }
        let next = next_page(offset, items.len(), total);
        return Ok(bounded_page(cut, domain, total, items, next, collections));
    }
    if path.len() == 3 && path[0] == "models" && path[2] == "profiles" {
        let model = ordinal(&path[1])?;
        let total = source.value().profile_len(model).ok_or_else(invalid)? as u64;
        if offset > total {
            return Err(invalid());
        }
        let mut items = Vec::new();
        for position in offset..total.min(offset + limit as u64) {
            items.push(row_item(
                domain,
                position,
                &source
                    .value()
                    .profile_at(model, position)
                    .ok_or_else(invalid)?,
            ));
        }
        let next = next_page(offset, items.len(), total);
        return Ok(bounded_page(cut, domain, total, items, next, Vec::new()));
    }
    let (row, path) = catalog_row(source, path)?;
    indexed_page(&row, cut, domain, path, offset, limit)
}
fn capability_row<'a>(
    source: &SourceValue<crate::observation::capability_source::CapabilityReadSource>,
    path: &'a [String],
) -> Result<
    (
        Arc<crate::runtime::read_layout::SeekableObservationSource>,
        &'a [String],
    ),
    RuntimeClientError,
> {
    let field = path.first().ok_or_else(invalid)?;
    if field == "revision" {
        return Ok((
            source
                .row(path[..1].to_vec(), &source.value().revision())
                .map_err(source_error)?,
            &path[1..],
        ));
    }
    let position = ordinal(path.get(1).ok_or_else(invalid)?)?;
    let row = source.value().row_at(field, position).ok_or_else(invalid)?;
    Ok((
        source.row(path[..2].to_vec(), &row).map_err(source_error)?,
        &path[2..],
    ))
}
fn capability_page(
    source: &SourceValue<crate::observation::capability_source::CapabilityReadSource>,
    cut: &CheckpointReadCut,
    domain: &str,
    path: &[String],
    offset: u64,
    limit: usize,
    singleton: bool,
) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
    if limit == 0 || limit > 32 || domain.len() > 256 {
        return Err(invalid());
    }
    if singleton {
        if offset > 1 {
            return Err(invalid());
        }
        let header = source.value().header();
        let collections = source.value().frontiers();
        let complete = collections.iter().all(|field| field.loaded == field.total);
        let mut item = row_item(domain, 0, &header);
        item.body_complete = Some(complete);
        return Ok(bounded_page(
            cut,
            domain,
            1,
            if offset == 0 { vec![item] } else { Vec::new() },
            None,
            collections,
        ));
    }
    if path.len() == 1 && path[0] != "revision" {
        let field = &path[0];
        let total = source.value().field_len(field).ok_or_else(invalid)? as u64;
        if offset > total {
            return Err(invalid());
        }
        let mut items = Vec::new();
        for position in offset..total.min(offset + limit as u64) {
            items.push(row_item(
                domain,
                position,
                &source.value().row_at(field, position).ok_or_else(invalid)?,
            ));
        }
        let next = next_page(offset, items.len(), total);
        return Ok(bounded_page(cut, domain, total, items, next, Vec::new()));
    }
    let (row, path) = capability_row(source, path)?;
    indexed_page(&row, cut, domain, path, offset, limit)
}

fn resource_frontiers(
    source: &SourceValue<crate::observation::resource_source::ResourceReadSource>,
    cut: &CheckpointReadCut,
    domain: &str,
    node: &crate::observation::resource_source::ResourceNode<'_>,
) -> Result<Vec<crate::observation::cut::CollectionFrontier>, RuntimeClientError> {
    use crate::observation::cut::CollectionFrontier;
    let mut collections = Vec::new();
    if let Some(count) = node.children() {
        for position in 0..count.min(32) {
            let (name, child) = source.value().child(node, position).ok_or_else(invalid)?;
            if let Some(count) = child.children() {
                collections.push(CollectionFrontier {
                    domain: format!(
                        "{domain}.{}",
                        name.map_or_else(|| position.to_string(), str::to_owned)
                    ),
                    revision: cut.revision,
                    total: count as u64,
                    loaded: 0,
                    next_offset: (count > 0).then_some(0),
                });
            }
        }
    }
    Ok(collections)
}

fn resource_page(
    source: &SourceValue<crate::observation::resource_source::ResourceReadSource>,
    cut: &CheckpointReadCut,
    domain: &str,
    path: &[String],
    offset: u64,
    limit: usize,
    singleton: bool,
) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
    use crate::observation::cut::{CheckpointPageItem, CollectionFrontier};
    if limit == 0 || limit > 32 || domain.len() > 256 {
        return Err(invalid());
    }
    if path.is_empty() && singleton {
        if offset > 1 {
            return Err(invalid());
        }
        let mut item = row_item(domain, 0, &source.value().header());
        item.body_complete = Some(false);
        return Ok(bounded_page(
            cut,
            domain,
            1,
            if offset == 0 { vec![item] } else { Vec::new() },
            None,
            source.value().frontiers(),
        ));
    }
    if path == ["context_files"] {
        return resource_context_page(source, cut, domain, offset, limit);
    }
    let node = source.value().node(path).ok_or_else(invalid)?;
    if singleton {
        if offset > 1 {
            return Err(invalid());
        }
        let collections = resource_frontiers(source, cut, domain, &node)?;
        let item = resource_item(domain, 0, &node);
        return Ok(bounded_page(
            cut,
            domain,
            1,
            if offset == 0 { vec![item] } else { Vec::new() },
            None,
            collections,
        ));
    }
    let Some(count) = node.children() else {
        if node.text().is_some() {
            if offset > 1 {
                return Err(invalid());
            }
            return Ok(bounded_page(
                cut,
                domain,
                1,
                if offset == 0 {
                    vec![resource_item(domain, 0, &node)]
                } else {
                    Vec::new()
                },
                None,
                Vec::new(),
            ));
        }
        let row = source.row(path.to_vec(), &node).map_err(source_error)?;
        return indexed_page(&row, cut, domain, &[], offset, limit);
    };
    let total = count as u64;
    if offset > total {
        return Err(invalid());
    }
    let mut items = Vec::new();
    let mut collections = Vec::new();
    for position in offset..total.min(offset + limit as u64) {
        let (name, child) = source
            .value()
            .child(&node, usize::try_from(position).map_err(|_| invalid())?)
            .ok_or_else(invalid)?;
        if let Some(children) = child.children() {
            collections.push(CollectionFrontier {
                domain: format!("{domain}.{position}"),
                revision: cut.revision,
                total: children as u64,
                loaded: 0,
                next_offset: (children > 0).then_some(0),
            });
        }
        let mut item: CheckpointPageItem = resource_item(domain, position, &child);
        if let Some(name) = name {
            item.identity = name.into();
        }
        items.push(item);
    }
    let next = next_page(offset, items.len(), total);
    Ok(bounded_page(cut, domain, total, items, next, collections))
}
fn resource_read(
    source: &SourceValue<crate::observation::resource_source::ResourceReadSource>,
    cut: &CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<CheckpointValueChunk, RuntimeClientError> {
    if path.len() >= 2 && path[0] == "context_files" {
        let position = ordinal(&path[1])?;
        let row = source
            .value()
            .context_file_at(position)
            .ok_or_else(invalid)?;
        let index = source.row(path[..2].to_vec(), &row).map_err(source_error)?;
        return Ok(chunk_result(
            index
                .read(&path[2..], offset, max_bytes)
                .map_err(source_error)?,
            cut,
            domain,
            item_offset,
            path,
            offset,
        ));
    }
    for prefix in (1..=path.len()).rev() {
        if let Some(node) = source.value().node(&path[..prefix]) {
            if prefix == path.len()
                && let Some(text) = node.text()
            {
                let chunk = crate::runtime::read_layout::text_chunk(text, offset, max_bytes)
                    .map_err(source_error)?;
                return Ok(CheckpointValueChunk {
                    representation: super::checkpoint::CheckpointValueRepresentation::Utf8Text,
                    cut: cut.clone(),
                    domain: domain.into(),
                    item_offset,
                    path: path.to_vec(),
                    offset,
                    bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
                    next_offset: chunk.next_offset,
                    readable_end: chunk.readable_end,
                });
            }
            if matches!(
                node,
                crate::observation::resource_source::ResourceNode::Inspection(_)
                    | crate::observation::resource_source::ResourceNode::Agents(_)
                    | crate::observation::resource_source::ResourceNode::Workflows(_)
                    | crate::observation::resource_source::ResourceNode::Sources(_)
            ) || node.children().is_some_and(|count| count > 32)
            {
                return Err(RuntimeClientError::ObservationUnavailable{message:"growing native resource containers are read through their advancing field pages".into()});
            }
            let index = source
                .row(path[..prefix].to_vec(), &node)
                .map_err(source_error)?;
            return Ok(chunk_result(
                index
                    .read(&path[prefix..], offset, max_bytes)
                    .map_err(source_error)?,
                cut,
                domain,
                item_offset,
                path,
                offset,
            ));
        }
    }
    Err(invalid())
}

fn resource_item(
    domain: &str,
    position: u64,
    node: &crate::observation::resource_source::ResourceNode<'_>,
) -> crate::observation::cut::CheckpointPageItem {
    use crate::observation::resource_source::ResourceNode as Node;
    let growing = matches!(
        node,
        Node::Inspection(_) | Node::Agents(_) | Node::Workflows(_) | Node::Sources(_)
    ) || node.children().is_some_and(|count| count > 32);
    let long = node.text().is_some_and(|text| text.len() > 8192);
    let growing = growing || long;
    if growing {
        crate::observation::cut::CheckpointPageItem {
            body_complete: Some(false),
            key: None,
            offset: position,
            identity: format!("{domain}:{position}"),
            deferred: true,
            value: None,
            content: Vec::new(),
        }
    } else {
        row_item(domain, position, node)
    }
}

fn resource_context_page(
    source: &SourceValue<crate::observation::resource_source::ResourceReadSource>,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
    let total = source.value().context_file_count() as u64;
    if offset > total {
        return Err(invalid());
    }
    let mut items = Vec::new();
    for position in offset..total.min(offset + limit as u64) {
        items.push(row_item(
            domain,
            position,
            &source
                .value()
                .context_file_at(position)
                .ok_or_else(invalid)?,
        ));
    }
    let next = next_page(offset, items.len(), total);
    Ok(bounded_page(cut, domain, total, items, next, Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::SourceValue;
    use serde::Serialize;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[derive(schemars::JsonSchema)]
    struct Counted {
        #[schemars(skip)]
        encodings: Arc<AtomicUsize>,
        values: Vec<String>,
    }
    impl Serialize for Counted {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeStruct as _;
            self.encodings.fetch_add(1, Ordering::SeqCst);
            let mut value = serializer.serialize_struct("Counted", 1)?;
            value.serialize_field("values", &self.values)?;
            value.end()
        }
    }
    #[test]
    fn t08_t15_t20_domain_native_source_indexes_once_and_pages_large_members() {
        let encodings = Arc::new(AtomicUsize::new(0));
        let native = Arc::new(Counted {
            encodings: Arc::clone(&encodings),
            values: (0..73)
                .map(|index| {
                    if index == 0 {
                        "雪😀".repeat(400_000)
                    } else {
                        format!("value{index}")
                    }
                })
                .collect(),
        });
        let source = Arc::new(SourceValue::new(Arc::clone(&native)));
        assert!(source.header(&[]).unwrap().is_none());
        let first = source.page(&["values".into()], 0, 32).unwrap();
        assert_eq!((first.total, first.next_offset), (73, Some(32)));
        assert!(first.fields[0].value.is_none());
        assert!(first.fields[1..].iter().all(|field| field.value.is_some()));
        let selected = ["values".into(), "@0".into()];
        let first = source.read(&selected, 0, 65_536).unwrap();
        assert_eq!(first.bytes.len(), 65_536);
        assert_eq!(first.next_offset, Some(65_536));
        let second = source.read(&selected, 65_536, 65_536).unwrap();
        assert_eq!(second.readable_end, first.readable_end);
        assert_eq!(
            encodings.load(Ordering::SeqCst),
            1,
            "later reads and pages reuse the immutable index"
        );
        crate::runtime::read_layout::READ_WORK.with(|work| assert_eq!(work.get(), (1, 65_536)));
        let tail = source.page(&["values".into()], 64, 32).unwrap();
        assert_eq!((tail.fields.len(), tail.next_offset), (9, None));
        assert!(source.read(&selected, first.readable_end + 1, 1).is_err());
        assert_eq!(encodings.load(Ordering::SeqCst), 1);
        let weak = Arc::downgrade(&source);
        drop(source);
        assert!(
            weak.upgrade().is_none(),
            "the original owner does not retain an observation layout"
        );
        assert_eq!(Arc::strong_count(&native), 1);
    }
}
