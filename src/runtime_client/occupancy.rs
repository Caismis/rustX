//! Finite display of a native occupancy reading. Original models remain source-owned.

pub(crate) enum OccupancyModelSource {
    Journal(u64),
    Inherited,
}

pub(crate) fn selection(domain: &str) -> Option<OccupancyModelSource> {
    let value = domain
        .strip_prefix("context.occupancy.")?
        .strip_suffix(".model")?;
    if value == "inherited" {
        return Some(OccupancyModelSource::Inherited);
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .map(OccupancyModelSource::Journal)
}

pub(crate) fn value_read(
    store: &dyn crate::durable::ConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<super::checkpoint::CheckpointValueChunk, super::RuntimeClientError> {
    use base64::Engine as _;
    if cut.source_cut.is_none() || &cut.conversation_id != store.conversation_id() {
        return Err(super::RuntimeClientError::ReadCutExpired);
    }
    let sequence = selection(domain).ok_or_else(|| super::RuntimeClientError::InvalidRequest {
        message: "invalid occupancy model source".into(),
    })?;
    let sequence = match sequence {
        OccupancyModelSource::Journal(sequence) => Some(sequence),
        OccupancyModelSource::Inherited => None,
    };
    if item_offset != 0 || !path.is_empty() {
        return Err(super::RuntimeClientError::InvalidRequest {
            message: "occupancy model source is a selected immutable field".into(),
        });
    }
    let chunk = store
        .observation_occupancy_model_value_read(sequence, cut.through, offset, max_bytes)
        .map_err(super::RuntimeClientError::observation_store_error)?;
    Ok(super::checkpoint::CheckpointValueChunk {
        representation: super::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: Vec::new(),
        offset,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        next_offset: chunk.next_offset,
        readable_end: chunk.readable_end,
    })
}

pub(crate) fn page(
    store: &dyn crate::durable::ConversationStore,
    cut: &crate::observation::cut::CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<super::checkpoint::CheckpointPage, super::RuntimeClientError> {
    use base64::Engine as _;
    if limit == 0 || limit > 32 || offset > 1 {
        return Err(super::RuntimeClientError::InvalidRequest {
            message: "invalid occupancy model page bounds".into(),
        });
    }
    let mut items = Vec::new();
    let mut loaded = 0;
    if offset == 0 {
        let chunk = value_read(store, cut, domain, 0, &[], 0, 1024)?;
        let value = if chunk.next_offset.is_none() {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&chunk.bytes_base64)
                .map_err(|_| super::RuntimeClientError::ObservationUnavailable {
                    message: "occupancy model bytes unavailable".into(),
                })?;
            Some(serde_json::from_slice(&bytes).map_err(|_| {
                super::RuntimeClientError::ObservationUnavailable {
                    message: "occupancy model source is corrupt".into(),
                }
            })?)
        } else {
            None
        };
        loaded = u64::from(value.is_some());
        items.push(crate::observation::cut::CheckpointPageItem {
            body_complete: None,
            key: None,
            offset: 0,
            identity: "@0".into(),
            deferred: value.is_none(),
            value,
            content: Vec::new(),
        });
    }
    Ok(super::checkpoint::CheckpointPage {
        cursor: super::RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: domain.into(),
        total: 1,
        items,
        next_offset: None,
        collections: vec![crate::observation::cut::CollectionFrontier {
            domain: domain.into(),
            revision: cut.through,
            total: 1,
            loaded,
            next_offset: (loaded == 0).then_some(0),
        }],
    })
}
