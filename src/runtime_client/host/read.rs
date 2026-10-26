//! Bounded content, collection and selected-value reads at an exact checkpoint cut.
use super::{
    Arc, ClientInner, ConversationRuntime, InteractionRef, RuntimeClientCursor, RuntimeClientError,
    RuntimeClientResult, TranscriptFieldSource,
};

pub(super) fn transcript_native_field(domain: &str) -> Option<(u64, &str)> {
    let tail = domain.strip_prefix("transcript.entries.")?;
    let (cursor, field) = tail.split_once('.')?;
    let cursor = cursor.parse().ok()?;
    matches!(field, "completed_response.models" | "turn_process.failure").then_some((cursor, field))
}
pub(super) fn validate_pending_path(path: &[String]) -> Result<(), RuntimeClientError> {
    if path.len() > 32 || path.iter().any(|part| part.len() > 256) {
        Err(RuntimeClientError::InvalidRequest {
            message: "invalid pending source path".into(),
        })
    } else {
        Ok(())
    }
}
pub(super) fn collection_identity<T: serde::de::DeserializeOwned>(
    id: &str,
) -> Result<T, RuntimeClientError> {
    serde_json::from_value(serde_json::Value::String(id.into())).map_err(|_| {
        RuntimeClientError::InvalidRequest {
            message: "invalid native collection identity".into(),
        }
    })
}
pub(super) fn indexed_collection_read(
    source: &crate::runtime::read_layout::SeekableObservationSource,
    cut: &crate::observation::cut::CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    range: (u64, u32),
) -> Result<super::super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
    use base64::Engine;
    let chunk = source
        .read(path, range.0, range.1)
        .map_err(RuntimeClientError::observation_read_error)?;
    Ok(super::super::checkpoint::CheckpointValueChunk {
        representation:
            crate::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: path.to_vec(),
        offset: range.0,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        readable_end: chunk.readable_end,
        next_offset: chunk.next_offset,
    })
}
pub(super) fn direct_collection_read(
    text: &str,
    cut: &crate::observation::cut::CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    range: (u64, u32),
) -> Result<super::super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
    use base64::Engine;
    let chunk = crate::runtime::read_layout::text_chunk(text, range.0, range.1)
        .map_err(RuntimeClientError::observation_read_error)?;
    Ok(super::super::checkpoint::CheckpointValueChunk {
        representation: super::super::checkpoint::CheckpointValueRepresentation::Utf8Text,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: path.to_vec(),
        offset: range.0,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        readable_end: chunk.readable_end,
        next_offset: chunk.next_offset,
    })
}
pub(super) fn collection_page_item<T: serde::Serialize + schemars::JsonSchema>(
    value: &T,
    identity: String,
    offset: u64,
) -> crate::observation::cut::CheckpointPageItem {
    let value = crate::observation::bounded::structural_value(value);
    let deferred = value.is_none();
    crate::observation::cut::CheckpointPageItem {
        offset,
        identity,
        value,
        deferred,
        content: Vec::new(),
        key: None,
        body_complete: None,
    }
}
pub(super) fn model_name_page_item(
    offset: u64,
    identity: String,
    model: &super::super::snapshot::RuntimeClientModelName,
) -> crate::observation::cut::CheckpointPageItem {
    crate::observation::cut::CheckpointPageItem {
        body_complete: Some(false),
        key: None,
        offset,
        identity,
        deferred: model.value.is_none(),
        value: crate::observation::bounded::structural_value(model),
        content: Vec::new(),
    }
}
/// Native metadata keys have fixed names; numeric segments select bounded
/// source ordinals, never an ambiguous opaque JSON object key.
pub(super) fn source_domain_path(domain: &str) -> Option<(Vec<String>, bool)> {
    if matches!(
        domain,
        "model" | "capabilities" | "resources" | "attempt.model" | "workflows"
    ) {
        return Some((Vec::new(), true));
    }
    if domain == "resources.inspection" {
        return Some((vec!["inspection".into()], true));
    }
    let relative = domain
        .strip_prefix("model.")
        .or_else(|| domain.strip_prefix("attempt.model."))
        .or_else(|| domain.strip_prefix("capabilities."))
        .or_else(|| domain.strip_prefix("resources."))
        .or_else(|| domain.strip_prefix("model_catalog."))
        .or_else(|| domain.strip_prefix("workflows."))?;
    Some((
        relative
            .split('.')
            .map(|part| {
                if part.parse::<u64>().is_ok() {
                    format!("@{part}")
                } else {
                    part.to_owned()
                }
            })
            .collect(),
        false,
    ))
}

impl ClientInner {
    pub(crate) fn content_read(
        &self,
        reference: &crate::observation::content::ContentRef,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::observation::content::ContentChunk, RuntimeClientError> {
        if reference.conversation_id != self.conversation_id {
            return Err(RuntimeClientError::ContentSourceUnavailable);
        }
        let live = matches!(
            reference.source,
            crate::observation::content::ContentSource::LivePublicationPrefix { .. }
        );
        let source_cut = {
            let state = self.state.lock().expect("published native content cut");
            state.projection.validate_live_content_ref(reference)?;
            if live {
                None
            } else {
                state.projection.validate_published_source()?;
                Some(
                    state
                        .projection
                        .checkpoint_read_cut()
                        .source_cut
                        .ok_or(RuntimeClientError::ReadCutExpired)?,
                )
            }
        };
        if let Some(source_cut) = source_cut {
            self.store
                .validate_content_source_cut(reference, &source_cut)
                .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?;
        }
        self.store
            .content_read(reference, offset, max_bytes)
            .map_err(RuntimeClientError::content_read_error)
    }

    pub(crate) fn domain_page(
        &self,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let cut = self.lock_published_state().projection.checkpoint_read_cut();
        self.checkpoint_page(&cut, domain, offset, limit)
            .map(|page| RuntimeClientResult::DomainPage { page })
    }

    pub(super) fn collection_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        let native = *cut
            .collection_cuts
            .get(domain)
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let mut items = Vec::new();
        let next_offset;
        match domain {
            "jobs" => {
                let source = runtime
                    .background_registry()
                    .observation_sources_page(native, offset, limit)
                    .map_err(RuntimeClientError::observation_collection_error)?;
                next_offset = source.next_offset;
                for (index, source) in source.items.iter().enumerate() {
                    source.with_value(|value| {
                        items.push(collection_page_item(
                            &super::super::projection::views::background_source_view(value),
                            value.job_id.to_string(),
                            offset + index as u64,
                        ));
                    });
                }
            }
            "agents" => {
                let source = self
                    .agent_registry()?
                    .observation_sources_page(native, offset, limit)
                    .map_err(RuntimeClientError::observation_collection_error)?;
                next_offset = source.next_offset;
                for (index, source) in source.items.iter().enumerate() {
                    source.with_value(|value| {
                        items.push(collection_page_item(
                            &super::super::projection::views::agent_source_view(value),
                            value.agent_id.to_string(),
                            offset + index as u64,
                        ));
                    });
                }
            }
            _ => return Err(RuntimeClientError::ReadCutExpired),
        }
        Ok(super::super::checkpoint::CheckpointPage {
            read_cut: cut.clone(),
            cursor: RuntimeClientCursor::new(cut.revision),
            domain: domain.into(),
            total: native.total,
            items,
            next_offset,
            collections: Vec::new(),
        })
    }

    pub(super) fn collection_identity_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        if limit == 0 || limit > 32 || offset > 1 {
            return Err(RuntimeClientError::InvalidRequest {
                message: "invalid native identity page bounds".into(),
            });
        }
        let (owner, id) = domain
            .split_once('.')
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let native = *cut
            .collection_cuts
            .get(owner)
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let items = match owner {
            "jobs" => {
                let id = collection_identity(id)?;
                runtime
                    .background_registry()
                    .with_observation_value_by_id(native, &id, |value| {
                        (offset == 0)
                            .then(|| {
                                collection_page_item(
                                    &super::super::projection::views::background_source_view(value),
                                    value.job_id.to_string(),
                                    0,
                                )
                            })
                            .into_iter()
                            .collect()
                    })
            }
            "agents" => {
                let id = collection_identity(id)?;
                self.agent_registry()?
                    .with_observation_value_by_id(native, &id, |value| {
                        (offset == 0)
                            .then(|| {
                                collection_page_item(
                                    &super::super::projection::views::agent_source_view(value),
                                    value.agent_id.to_string(),
                                    0,
                                )
                            })
                            .into_iter()
                            .collect()
                    })
            }
            _ => return Err(RuntimeClientError::ReadCutExpired),
        }
        .map_err(RuntimeClientError::observation_collection_error)?;
        Ok(super::super::checkpoint::CheckpointPage {
            read_cut: cut.clone(),
            cursor: RuntimeClientCursor::new(cut.revision),
            domain: domain.into(),
            total: 1,
            items,
            next_offset: None,
            collections: Vec::new(),
        })
    }

    pub(super) fn collection_value_read(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        range: (u64, u32),
    ) -> Result<super::super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        let (owner, id) = domain
            .split_once('.')
            .map_or((domain, None), |(owner, id)| (owner, Some(id)));
        let native = *cut
            .collection_cuts
            .get(owner)
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        match (owner, id) {
            ("jobs", None) => runtime.background_registry().with_observation_source(
                native,
                item_offset,
                |source| {
                    if let Some(text) = source.direct_text(path) {
                        return direct_collection_read(text, cut, domain, item_offset, path, range);
                    }
                    let indexed = source.indexed().map_err(|message| {
                        RuntimeClientError::ObservationUnavailable {
                            message: message.into(),
                        }
                    })?;
                    indexed_collection_read(indexed, cut, domain, item_offset, path, range)
                },
            ),
            ("agents", None) => {
                self.agent_registry()?
                    .with_observation_source(native, item_offset, |source| {
                        if let Some(text) = source.direct_text(path) {
                            return direct_collection_read(
                                text,
                                cut,
                                domain,
                                item_offset,
                                path,
                                range,
                            );
                        }
                        let indexed = source.indexed().map_err(|message| {
                            RuntimeClientError::ObservationUnavailable {
                                message: message.into(),
                            }
                        })?;
                        indexed_collection_read(indexed, cut, domain, item_offset, path, range)
                    })
            }
            ("jobs", Some(id)) if item_offset == 0 => {
                let id = collection_identity(id)?;
                runtime
                    .background_registry()
                    .with_observation_source_by_id(native, &id, |source| {
                        if let Some(text) = source.direct_text(path) {
                            return direct_collection_read(
                                text,
                                cut,
                                domain,
                                item_offset,
                                path,
                                range,
                            );
                        }
                        let indexed = source.indexed().map_err(|message| {
                            RuntimeClientError::ObservationUnavailable {
                                message: message.into(),
                            }
                        })?;
                        indexed_collection_read(indexed, cut, domain, item_offset, path, range)
                    })
            }
            ("agents", Some(id)) if item_offset == 0 => {
                let id = collection_identity(id)?;
                self.agent_registry()?
                    .with_observation_source_by_id(native, &id, |source| {
                        if let Some(text) = source.direct_text(path) {
                            return direct_collection_read(
                                text,
                                cut,
                                domain,
                                item_offset,
                                path,
                                range,
                            );
                        }
                        let indexed = source.indexed().map_err(|message| {
                            RuntimeClientError::ObservationUnavailable {
                                message: message.into(),
                            }
                        })?;
                        indexed_collection_read(indexed, cut, domain, item_offset, path, range)
                    })
            }
            _ => return Err(RuntimeClientError::ReadCutExpired),
        }
        .map_err(RuntimeClientError::observation_collection_error)?
    }

    pub(in crate::runtime_client) fn pending_read_header(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        source_offset: u64,
    ) -> Result<crate::observation::interaction::RuntimeClientInteractionHeader, RuntimeClientError>
    {
        {
            let state = self.state.lock().expect("runtime client state lock");
            state.projection.validate_checkpoint_cut(cut)?;
            state
                .projection
                .validate_domain_version(cut, "pending_interactions")?;
            if let Some(header) = state
                .projection
                .snapshot_ref()
                .pending_interactions
                .iter()
                .find(|header| header.source_offset == source_offset)
            {
                return Ok(header.clone());
            }
        }
        if !source_offset.is_multiple_of(2) {
            let native_cut = *cut
                .collection_cuts
                .get("pending_children")
                .ok_or(RuntimeClientError::ReadCutExpired)?;
            return self
                .runtime
                .as_ref()
                .and_then(ConversationRuntime::subagent_registry)
                .ok_or(RuntimeClientError::ReadCutExpired)?
                .pending_interaction_source_at(native_cut, source_offset / 2)
                .map_err(RuntimeClientError::observation_collection_error);
        }
        let native_cut = *cut
            .collection_cuts
            .get("pending_primary")
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let source = self
            .runtime
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?
            .pending_interaction_source_at(native_cut, source_offset / 2)
            .map_err(RuntimeClientError::observation_collection_error)?;
        let mut header = source.header().clone();
        header.source_offset = source_offset;
        Ok(header)
    }

    pub(super) fn live_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        let live = cut
            .live_publication
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let source = if domain == "attempt.foreground" {
            self.store
                .live_tool_header_page(&live.source, live.revision, offset, limit)
        } else {
            self.store
                .live_block_header_page(&live.source, live.revision, offset, limit)
        }
        .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?;
        let items = source
            .headers
            .into_iter()
            .map(|header| {
                let content = header
                    .content
                    .iter()
                    .map(|reference| crate::observation::cut::ContentBinding {
                        path: vec![if reference.source.key().0 == "live_block_metadata" {
                            "metadata".into()
                        } else {
                            "payload".into()
                        }],
                        source_path: None,
                        content_ref: reference.clone(),
                        deferred: true,
                    })
                    .collect();
                Ok(crate::observation::cut::CheckpointPageItem {
                    offset: header.position,
                    identity: format!("{}:{}", header.kind, header.block_index.get()),
                    value: Some(
                        crate::observation::bounded::structural_value(&header)
                            .ok_or(RuntimeClientError::ReadCutExpired)?,
                    ),
                    body_complete: None,
                    key: None,
                    content,
                    deferred: true,
                })
            })
            .collect::<Result<Vec<_>, RuntimeClientError>>()?;
        Ok(super::super::checkpoint::CheckpointPage {
            read_cut: cut.clone(),
            cursor: RuntimeClientCursor::new(cut.revision),
            domain: domain.into(),
            total: source.total,
            items,
            next_offset: source.next_offset,
            collections: Vec::new(),
        })
    }

    pub(super) fn canonical_foreground_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        let foreground = cut
            .canonical_foreground
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let seed = self
            .state
            .lock()
            .expect("native published foreground")
            .projection
            .checkpoint_seed();
        let mut page = super::super::tool_observation::page(
            self.store.as_ref(),
            cut,
            "attempt.foreground",
            &foreground.locator,
            offset,
            limit,
        )?;
        if page.total != foreground.total {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        seed.retain_foreground_progress(&mut page)?;
        Ok(page)
    }

    pub(super) fn pending_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        if limit == 0 || limit > 32 {
            return Err(RuntimeClientError::InvalidRequest {
                message: "invalid pending page bounds".into(),
            });
        }
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let primary_cut = *cut
            .collection_cuts
            .get("pending_primary")
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let proof = runtime
            .pending_interaction_page(
                primary_cut,
                offset.checked_sub(1).map(|offset| offset / 2),
                limit,
            )
            .map_err(RuntimeClientError::observation_collection_error)?;
        let mut total = proof.cut.total;
        let mut next = proof
            .next_sequence
            .and_then(|sequence| sequence.checked_mul(2));
        let mut headers: Vec<_> = proof
            .head
            .into_iter()
            .map(|(sequence, source)| {
                let mut header = source.header().clone();
                header.source_offset = sequence.checked_mul(2).expect("bounded source offset");
                header
            })
            .collect();
        if let Some(registry) = runtime.subagent_registry() {
            let child_cut = *cut
                .collection_cuts
                .get("pending_children")
                .ok_or(RuntimeClientError::ReadCutExpired)?;
            let child = registry
                .pending_interaction_page(
                    child_cut,
                    offset.checked_sub(2).map(|offset| offset / 2),
                    limit,
                )
                .map_err(RuntimeClientError::observation_collection_error)?;
            total = total
                .checked_add(child.cut.total)
                .ok_or(RuntimeClientError::ReadCutExpired)?;
            let child_next = child
                .next_sequence
                .and_then(|sequence| sequence.checked_mul(2))
                .and_then(|offset| offset.checked_add(1));
            next = next.into_iter().chain(child_next).min();
            headers.extend(child.head);
        }
        headers.sort_by_key(|header| header.source_offset);
        if let Some(header) = headers.get(limit) {
            next = next.into_iter().chain(Some(header.source_offset)).min();
        }
        headers.truncate(limit);
        let items = headers
            .into_iter()
            .map(|header| {
                Ok(crate::observation::cut::CheckpointPageItem {
                    offset: header.source_offset,
                    identity: header.interaction.to_string(),
                    value: Some(
                        crate::observation::bounded::structural_value(&header)
                            .ok_or(RuntimeClientError::ReadCutExpired)?,
                    ),
                    deferred: true,
                    body_complete: Some(false),
                    key: None,
                    content: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>, RuntimeClientError>>()?;
        Ok(super::super::checkpoint::CheckpointPage {
            read_cut: cut.clone(),
            cursor: RuntimeClientCursor::new(cut.revision),
            domain: "pending_interactions".into(),
            total,
            items,
            next_offset: next,
            collections: Vec::new(),
        })
    }

    pub(super) fn primary_pending_source(
        &self,
        interaction: &InteractionRef,
    ) -> Result<Arc<crate::runtime::interaction::PendingInteractionSource>, RuntimeClientError>
    {
        if interaction.conversation_id != self.conversation_id {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        self.runtime
            .as_ref()
            .and_then(|runtime| runtime.pending_interaction_source(interaction))
            .ok_or(RuntimeClientError::ReadCutExpired)
    }

    pub(crate) fn interaction_fields(
        &self,
        interaction: &InteractionRef,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        validate_pending_path(path)?;
        let source = self.primary_pending_source(interaction)?;
        let page = source
            .page(path, offset, limit)
            .map_err(RuntimeClientError::observation_read_error)?;
        let fields = page
            .fields
            .into_iter()
            .map(|field| super::super::types::InteractionFieldItem {
                offset: field.ordinal,
                key: field.key,
                key_complete: field.key_complete,
                key_bytes: field.key_bytes,
                encoded_bytes: field.encoded_bytes,
                children: field.children,
                deferred: field.value.is_none(),
                value: field.value,
            })
            .collect();
        Ok(RuntimeClientResult::InteractionFields {
            interaction: interaction.clone(),
            path: path.to_vec(),
            total: page.total,
            fields,
            next_offset: page.next_offset,
        })
    }

    pub(crate) fn interaction_value_read(
        &self,
        interaction: &InteractionRef,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        use base64::Engine as _;
        validate_pending_path(path)?;
        let source = self.primary_pending_source(interaction)?;
        let chunk = source
            .read(path, offset, max_bytes)
            .map_err(RuntimeClientError::observation_read_error)?;
        Ok(RuntimeClientResult::InteractionValueChunk {
            interaction: interaction.clone(),
            path: path.to_vec(),
            offset,
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
            readable_end: chunk.readable_end,
            next_offset: chunk.next_offset,
        })
    }

    pub(super) fn pending_value_read(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        range: (u64, u32),
    ) -> Result<super::super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        let selection = super::super::interaction_observation::selection(domain);
        let (ordinal, mut selected) = selection.clone().unwrap_or((item_offset, Vec::new()));
        if selection.is_some() {
            selected.splice(0..0, ["kind".to_owned(), "arguments".to_owned()]);
        }
        let header = self.pending_read_header(cut, ordinal)?;
        let source = self.primary_pending_source(&header.interaction)?;
        if domain == "pending_interactions" && path.is_empty() {
            let source_end = source
                .read(&[], 0, 1)
                .map_err(RuntimeClientError::observation_read_error)?
                .readable_end;
            let plan = super::super::pending_observation::routed_range(
                &header, source_end, range.0, range.1,
            )?;
            let chunk = source
                .read(&[], plan.native_offset, plan.native_max_bytes)
                .map_err(RuntimeClientError::observation_read_error)?;
            let (bytes, end, next) =
                super::super::pending_observation::compose_routed(&plan, &chunk.bytes)?;
            return Ok(super::super::pending_observation::checkpoint_chunk(
                cut,
                domain,
                item_offset,
                path,
                range.0,
                (bytes, end, next),
            ));
        }
        if !selected.is_empty() {
            selected.push(format!("@{item_offset}"));
        }
        selected.extend_from_slice(path);
        let chunk = source
            .read(&selected, range.0, range.1)
            .map_err(RuntimeClientError::observation_read_error)?;
        Ok(super::super::pending_observation::checkpoint_chunk(
            cut,
            domain,
            item_offset,
            path,
            range.0,
            (chunk.bytes, chunk.readable_end, chunk.next_offset),
        ))
    }

    pub(super) fn state_source_available(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
    ) -> Result<bool, RuntimeClientError> {
        let available =
            super::super::state_observation::available(self.store.as_ref(), cut, domain)?;
        if !available && domain.split('.').next() == Some("goal") {
            let state = self.state.lock().expect("native projection lock");
            if state
                .projection
                .snapshot_ref()
                .goal
                .as_ref()
                .is_some_and(|goal| goal.current.is_some())
            {
                return Err(RuntimeClientError::ReadCutExpired);
            }
        }
        Ok(available)
    }

    pub(crate) fn checkpoint_value_read(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        if domain.len() > 256
            || path.len() > 32
            || path.iter().any(|part| part.len() > 256)
            || offset > 9_007_199_254_740_991
            || item_offset > 9_007_199_254_740_991
            || max_bytes == 0
            || max_bytes > 65_536
            || offset
                .checked_add(u64::from(max_bytes))
                .is_none_or(|end| end > 9_007_199_254_740_991)
        {
            return Err(RuntimeClientError::InvalidRequest {
                message: "invalid native domain value read bounds".into(),
            });
        }
        if super::super::journal_observation::selection(domain).is_some() {
            self.state
                .lock()
                .expect("native source cut")
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::super::journal_observation::value_read(
                self.store.as_ref(),
                cut,
                domain,
                item_offset,
                path,
                offset,
                max_bytes,
            );
        }
        if let Some(cursor) = super::super::response::identity_selection(domain) {
            self.state
                .lock()
                .expect("native projection lock")
                .projection
                .validate_checkpoint_cut(cut)?;
            if !path.is_empty() {
                return Err(RuntimeClientError::InvalidRequest {
                    message: "identity source read requires empty field path".into(),
                });
            }
            return super::super::response::identity_read(
                self.store.as_ref(),
                cut,
                domain,
                cursor,
                item_offset,
                (offset, max_bytes),
            );
        }
        self.selected_value_read(cut, domain, item_offset, path, (offset, max_bytes))
    }

    pub(super) fn immutable_source_value_read(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        (offset, max_bytes): (u64, u32),
    ) -> Result<Option<super::super::checkpoint::CheckpointValueChunk>, RuntimeClientError> {
        if domain == "trace.records" {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::super::trace::header_read(
                self.store.as_ref(),
                cut,
                item_offset,
                path,
                (offset, max_bytes),
            )
            .map(Some);
        }
        if super::super::projection_history::selection(domain).is_some() {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::super::projection_history::value_read(
                self.store.as_ref(),
                cut,
                domain,
                item_offset,
                path,
                (offset, max_bytes),
            )
            .map(Some);
        }
        if super::super::delivery::selection(domain).is_some() {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::super::delivery::value_read(
                self.store.as_ref(),
                cut,
                domain,
                item_offset,
                path,
                offset,
                max_bytes,
            )
            .map(Some);
        }
        if super::super::attempt_identity::selection(domain).is_some() {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::super::attempt_identity::value_read(
                self.store.as_ref(),
                cut,
                domain,
                item_offset,
                path,
                offset,
                max_bytes,
            )
            .map(Some);
        }
        if super::super::occupancy::selection(domain).is_some() {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::super::occupancy::value_read(
                self.store.as_ref(),
                cut,
                domain,
                item_offset,
                path,
                offset,
                max_bytes,
            )
            .map(Some);
        }
        Ok(None)
    }

    pub(super) fn selected_value_read(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        range: (u64, u32),
    ) -> Result<super::super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        let (offset, max_bytes) = range;
        if let Some(chunk) =
            self.immutable_source_value_read(cut, domain, item_offset, path, range)?
        {
            return Ok(chunk);
        }
        if domain
            .split('.')
            .next()
            .is_some_and(|owner| owner == "jobs" || owner == "agents")
        {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return self.collection_value_read(cut, domain, item_offset, path, (offset, max_bytes));
        }
        if domain == "pending_interactions"
            || super::super::interaction_observation::selection(domain).is_some()
        {
            return self.pending_value_read(cut, domain, item_offset, path, (offset, max_bytes));
        }
        if super::super::state_observation::selection(domain).is_some()
            && self.state_source_available(cut, domain)?
        {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::super::state_observation::read(
                self.store.as_ref(),
                cut,
                domain,
                item_offset,
                path,
                (offset, max_bytes),
            );
        }
        if let Some((sequence, selected)) = super::super::trace_observation::selection(domain) {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return if selected.is_empty() {
                super::super::trace_observation::read(
                    self.store.as_ref(),
                    cut,
                    domain,
                    sequence,
                    (item_offset, path),
                    (offset, max_bytes),
                )
            } else {
                super::super::trace_observation::field_read(
                    self.store.as_ref(),
                    cut,
                    domain,
                    sequence,
                    &selected,
                    (item_offset, path),
                    (offset, max_bytes),
                )
            };
        }
        if let Some(field) = transcript_native_field(domain) {
            return self.transcript_field_read(
                cut,
                domain,
                field,
                item_offset,
                path,
                (offset, max_bytes),
            );
        }
        if domain == "inbound.pending" {
            return self.inbound_value_read(cut, domain, item_offset, path, (offset, max_bytes));
        }
        let source = {
            let state = self.lock_published_state();
            state.projection.validate_checkpoint_cut(cut)?;
            state.projection.validate_domain_version(cut, domain)?;
            state
                .projection
                .domain_source(domain, item_offset)
                .ok_or(RuntimeClientError::ReadCutExpired)?
        };
        let mut selected = path.to_vec();
        if let Some((mut prefix, singleton)) = source_domain_path(domain) {
            if !singleton {
                prefix.push(format!("@{item_offset}"));
            }
            selected.splice(0..0, prefix);
        } else if domain == "todos.tasks" {
            selected.splice(0..0, ["tasks".into(), format!("@{item_offset}")]);
        }
        let mut chunk = source.read(cut, domain, item_offset, &selected, offset, max_bytes)?;
        // Source layout paths are internal selectors. The public range identity
        // retains the caller's path within its exact domain and item.
        chunk.path = path.to_vec();
        Ok(chunk)
    }

    pub(super) fn inbound_value_read(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        range: (u64, u32),
    ) -> Result<super::super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        use base64::Engine as _;
        let (offset, max_bytes) = range;
        if !path.is_empty() {
            return Err(RuntimeClientError::InvalidRequest {
                message: "pending read path must name the whole authoritative UserMessage".into(),
            });
        }
        self.lock_published_state()
            .projection
            .validate_checkpoint_cut(cut)?;
        let source_cut = cut
            .source_cut
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let chunk = self
            .store
            .pending_value_read(&source_cut.read, item_offset, offset, max_bytes)
            .map_err(RuntimeClientError::observation_store_error)?;
        Ok(super::super::checkpoint::CheckpointValueChunk {
            representation:
                crate::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
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

    pub(super) fn transcript_field_read(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        field: (u64, &str),
        item_offset: u64,
        path: &[String],
        range: (u64, u32),
    ) -> Result<super::super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        use base64::Engine as _;
        let (offset, max_bytes) = range;
        if !path.is_empty() {
            return Err(RuntimeClientError::InvalidRequest {
                message: "native transcript field read expects an empty path".into(),
            });
        }
        self.lock_published_state()
            .projection
            .validate_checkpoint_cut(cut)?;
        let source = self.transcript_field_source(cut, field.0)?;
        let native = cut
            .source_cut
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let chunk = match (source, field.1) {
            (TranscriptFieldSource::Live(sequence), "completed_response.models") => self
                .store
                .observation_model_read_at(native, sequence, item_offset, offset, max_bytes),
            (TranscriptFieldSource::Inherited(ordinal), "completed_response.models") => self
                .store
                .observation_inherited_model_read(ordinal, item_offset, offset, max_bytes),
            (TranscriptFieldSource::Live(sequence), "turn_process.failure") if item_offset == 0 => {
                self.store
                    .observation_failure_read_at(native, sequence, offset, max_bytes)
            }
            _ => return Err(RuntimeClientError::ReadCutExpired),
        }
        .map_err(RuntimeClientError::observation_store_error)?;
        Ok(super::super::checkpoint::CheckpointValueChunk {
            representation:
                crate::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
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

    pub(super) fn domain_source_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<Option<super::super::checkpoint::CheckpointPage>, RuntimeClientError> {
        if let Some((path, singleton)) = source_domain_path(domain) {
            let source = {
                let state = self.lock_published_state();
                state.projection.validate_domain_version(cut, domain)?;
                state
                    .projection
                    .domain_source(domain, 0)
                    .ok_or(RuntimeClientError::ReadCutExpired)?
            };
            return source
                .page(cut, domain, &path, offset, limit, singleton)
                .map(Some);
        }
        Ok(None)
    }

    /// Resolve only a committed successful Tool delivery in this exact viewed
    /// lineage. A browser supplies canonical coordinates, never a file path.
    pub(crate) fn session_file_reference(
        &self,
        locator: &crate::observation::content::CanonicalContentLocator,
        index: usize,
    ) -> Result<crate::tools::session_files::SessionFileReference, RuntimeClientError> {
        let index = u64::try_from(index).map_err(|_| RuntimeClientError::InvalidRange)?;
        self.store
            .observation_file_delivery(locator, index)
            .map(|selected| selected.file)
            .map_err(RuntimeClientError::observation_store_error)
    }

    /// Read a bounded conversation-owned artifact without exposing its path.
    pub(crate) fn artifact_read(
        &self,
        id: &crate::runtime::identity::ArtifactId,
    ) -> Result<String, RuntimeClientError> {
        use base64::Engine;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| RuntimeClientError::InvalidState {
                message: "artifact runtime unavailable".into(),
            })?;
        let bytes = runtime
            .tool_runtime()
            .artifacts()
            .read_bounded(id)
            .map_err(|_| RuntimeClientError::InvalidState {
                message: "artifact unavailable or exceeds 256 KiB".into(),
            })?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }
}
