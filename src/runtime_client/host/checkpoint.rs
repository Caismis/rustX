//! Checkpoint pages and checkpoint materialization from the off-lock seed.
use super::read::model_name_page_item;
use super::read::transcript_native_field;
use super::{
    ClientInner, ConversationObservation, Ordering, RuntimeClientCursor, RuntimeClientError,
    TranscriptFieldSource,
};
use crate::observation::cut::CollectionFrontier;
use crate::runtime_client::checkpoint::RuntimeClientCheckpoint;

impl ClientInner {
    pub(crate) fn checkpoint_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        if cut.conversation_id != self.conversation_id {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        self.lock_published_state()
            .projection
            .validate_checkpoint_cut(cut)?;
        if super::super::delivery::selection(domain).is_some() {
            return super::super::delivery::page(self.store.as_ref(), cut, domain, offset, limit);
        }
        if super::super::occupancy::selection(domain).is_some() {
            return super::super::occupancy::page(self.store.as_ref(), cut, domain, offset, limit);
        }
        if domain == crate::runtime_client::auxiliary::DOMAIN {
            return crate::runtime_client::auxiliary::page(self.store.as_ref(), cut, offset, limit);
        }
        if let Some(page) = self.domain_source_page(cut, domain, offset, limit)? {
            return Ok(page);
        }

        if (domain == "attempt.in_flight.blocks" || domain == "attempt.foreground")
            && cut.live_publication.is_some()
        {
            return self.live_page(cut, domain, offset, limit);
        }
        if domain == "attempt.foreground" && cut.canonical_foreground.is_some() {
            return self.canonical_foreground_page(cut, offset, limit);
        }
        if super::super::journal_observation::selection(domain).is_some() {
            return super::super::journal_observation::page(
                self.store.as_ref(),
                cut,
                domain,
                offset,
                limit,
            );
        }
        if let Some(cursor) = super::super::response::identity_selection(domain) {
            return super::super::response::identity_page(
                self.store.as_ref(),
                cut,
                domain,
                cursor,
                offset,
                limit,
            );
        }
        if domain == "trace.records" {
            return super::super::trace::checkpoint_page(self.store.as_ref(), cut, offset, limit);
        }
        if super::super::audit_observation::selection(domain).is_some() {
            return super::super::audit_observation::page(
                self.store.as_ref(),
                cut,
                domain,
                offset,
                limit,
            );
        }
        if domain == "pending_interactions" {
            return self.pending_page(cut, offset, limit);
        }
        if domain.starts_with("jobs.") || domain.starts_with("agents.") {
            return self.collection_identity_page(cut, domain, offset, limit);
        }
        if domain == "jobs" || domain == "agents" {
            return self.collection_page(cut, domain, offset, limit);
        }
        if let Some((ordinal, selected)) = super::super::interaction_observation::selection(domain)
        {
            return self.interaction_checkpoint_page(cut, domain, ordinal, selected, offset, limit);
        }
        if super::super::state_observation::selection(domain).is_some()
            && self.state_source_available(cut, domain)?
        {
            return super::super::state_observation::page(
                self.store.as_ref(),
                cut,
                domain,
                offset,
                limit,
            );
        }
        if let Some(page) = self.trace_checkpoint_page(cut, domain, offset, limit) {
            return page;
        }
        if let Some(field) = transcript_native_field(domain) {
            return self.transcript_field_page(cut, domain, field.0, field.1, offset, limit);
        }
        if let Some(page) = self.durable_checkpoint_page(cut, domain, offset, limit) {
            return page;
        }
        self.published_checkpoint_page(cut, domain, offset, limit)
    }

    pub(super) fn trace_checkpoint_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Option<Result<super::super::checkpoint::CheckpointPage, RuntimeClientError>> {
        if super::super::trace_observation::context_selection(domain).is_some() {
            return Some(super::super::trace_observation::context_page(
                self.store.as_ref(),
                cut,
                domain,
                offset,
                limit,
            ));
        }
        if super::super::trace_observation::messages_selection(domain).is_some() {
            return Some(super::super::trace_observation::messages_page(
                self.store.as_ref(),
                cut,
                domain,
                offset,
                limit,
            ));
        }
        let (sequence, selected) = super::super::trace_observation::selection(domain)?;
        Some(if selected.is_empty() {
            super::super::trace_observation::page(
                self.store.as_ref(),
                cut,
                domain,
                sequence,
                offset,
            )
        } else {
            super::super::trace_observation::field_page(
                self.store.as_ref(),
                cut,
                domain,
                sequence,
                &selected,
                offset,
                limit,
            )
        })
    }

    pub(super) fn published_checkpoint_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        let seed = self.lock_published_state().projection.checkpoint_seed();
        let mut page = seed.checkpoint_page(cut, domain, offset, limit)?;
        for item in &mut page.items {
            self.materialize_content_bindings(&mut item.content)?;
        }
        Ok(page)
    }

    pub(super) fn durable_checkpoint_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Option<Result<super::super::checkpoint::CheckpointPage, RuntimeClientError>> {
        let source_cut = cut.source_cut.as_ref()?;
        if domain == "inbound.pending" {
            return Some(self.pending_checkpoint_page(cut, source_cut, offset, limit));
        }
        if domain == "messages"
            || domain == "transcript.entries"
            || domain.rsplit_once('.').is_some_and(|(prefix, family)| {
                prefix.starts_with("messages.") && matches!(family, "blocks" | "tools")
            })
        {
            return Some(super::super::history_observation::page(
                self.store.as_ref(),
                cut,
                domain,
                offset,
                limit,
            ));
        }
        None
    }

    pub(super) fn interaction_checkpoint_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        ordinal: u64,
        mut selected: Vec<String>,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        selected.splice(0..0, ["kind".to_owned(), "arguments".to_owned()]);
        let header = self.pending_read_header(cut, ordinal)?;
        let source = self.primary_pending_source(&header.interaction)?;
        let page = source
            .page(&selected, offset, limit)
            .map_err(RuntimeClientError::observation_read_error)?;
        let items = page
            .fields
            .into_iter()
            .map(|field| crate::observation::cut::CheckpointPageItem {
                offset: field.ordinal,
                identity: field.ordinal.to_string(),
                value: field.value.clone(),
                deferred: field.value.is_none(),
                key: None,
                body_complete: None,
                content: Vec::new(),
            })
            .collect();
        Ok(super::super::checkpoint::CheckpointPage {
            read_cut: cut.clone(),
            cursor: RuntimeClientCursor::new(cut.revision),
            domain: domain.into(),
            total: page.total,
            items,
            next_offset: page.next_offset,
            collections: Vec::new(),
        })
    }

    pub(super) fn pending_checkpoint_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        source_cut: &crate::durable::observation::ObservationReadCut,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        #[derive(serde::Serialize, schemars::JsonSchema)]
        struct PendingSummary {
            sequence: crate::runtime::inbound::InboundSequence,
            revision: u64,
            message: Option<crate::message::types::UserMessageBlock>,
        }
        let source = self
            .store
            .pending_header_page(&source_cut.read, (offset > 0).then_some(offset), limit)
            .map_err(RuntimeClientError::observation_store_error)?;
        let items = source
            .headers
            .into_iter()
            .map(|header| {
                let message = match header.message.display {
                    Some(crate::message::types::MessageBlock::User(message)) => Some(message),
                    _ => None,
                };
                let value = crate::observation::bounded::structural_value(&PendingSummary {
                    sequence: crate::runtime::inbound::InboundSequence::new(header.sequence),
                    revision: header.revision,
                    message,
                });
                crate::observation::cut::CheckpointPageItem {
                    body_complete: None,
                    key: None,
                    offset: header.sequence,
                    identity: header.message.identity.as_ref().map_or_else(
                        || format!("pending:{}", header.sequence),
                        ToString::to_string,
                    ),
                    deferred: header.message.deferred,
                    value,
                    content: Vec::new(),
                }
            })
            .collect();
        Ok(super::super::checkpoint::CheckpointPage {
            collections: Vec::new(),
            cursor: RuntimeClientCursor::new(cut.revision),
            read_cut: cut.clone(),
            domain: "inbound.pending".into(),
            total: source.total,
            items,
            next_offset: source.next_sequence,
        })
    }

    pub(super) fn transcript_field_source(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        cursor: u64,
    ) -> Result<TranscriptFieldSource, RuntimeClientError> {
        let source_cut = cut
            .source_cut
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        if cursor > source_cut.read.transcript {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        self.store
            .response_identity_source(source_cut, cursor)
            .map_err(RuntimeClientError::observation_store_error)?
            .map(|source| match source {
                crate::durable::observation::ResponseIdentitySource::Attempt { sequence } => {
                    TranscriptFieldSource::Live(sequence)
                }
                crate::durable::observation::ResponseIdentitySource::Inherited { ordinal } => {
                    TranscriptFieldSource::Inherited(ordinal)
                }
            })
            .ok_or(RuntimeClientError::ContentSourceUnavailable)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn transcript_field_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        cursor: u64,
        field: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        if !(1..=crate::observation::bounded::CHECKPOINT_PAGE_ITEMS).contains(&limit) {
            return Err(RuntimeClientError::InvalidRequest {
                message: "native field page limit must be in 1..=32".into(),
            });
        }
        let source = self.transcript_field_source(cut, cursor)?;
        let native = cut
            .source_cut
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let (total, items, next_offset) = match (source, field) {
            (TranscriptFieldSource::Live(sequence), "completed_response.models") => {
                let page = self
                    .store
                    .observation_models_at(native, sequence, offset, limit)
                    .map_err(RuntimeClientError::observation_store_error)?;
                let items = page
                    .names
                    .into_iter()
                    .map(|name| {
                        model_name_page_item(
                            name.ordinal,
                            name.request_id.as_ref().map_or_else(
                                || format!("trace:{}", name.source_sequence),
                                ToString::to_string,
                            ),
                            &super::super::snapshot::RuntimeClientModelName {
                                request_id: name.request_id,
                                request_id_source: name.request_id_source,
                                source_sequence: Some(name.source_sequence),
                                encoded_bytes: name.encoded_bytes,
                                value: name.value,
                            },
                        )
                    })
                    .collect();
                (page.total, items, page.next_offset)
            }
            (TranscriptFieldSource::Inherited(ordinal), "completed_response.models") => {
                let page = self
                    .store
                    .observation_inherited_models(ordinal, offset, limit)
                    .map_err(RuntimeClientError::observation_store_error)?;
                let items = page
                    .names
                    .into_iter()
                    .map(|name| {
                        model_name_page_item(
                            name.ordinal,
                            name.ordinal.to_string(),
                            &super::super::snapshot::RuntimeClientModelName {
                                request_id: None,
                                request_id_source: None,
                                source_sequence: None,
                                encoded_bytes: name.encoded_bytes,
                                value: name.value,
                            },
                        )
                    })
                    .collect();
                (page.total, items, page.next_offset)
            }
            (TranscriptFieldSource::Live(sequence), "turn_process.failure") => {
                let summary = self
                    .store
                    .observation_execution_at(native, sequence)
                    .map_err(RuntimeClientError::observation_store_error)?;
                let items = if offset == 0 {
                    summary
                        .failure
                        .map(|failure| crate::observation::cut::CheckpointPageItem {
                            body_complete: None,
                            key: None,
                            offset: 0,
                            identity: format!("attempt:{sequence}"),
                            deferred: summary.failure_deferred,
                            value: crate::observation::bounded::structural_value(&failure),
                            content: Vec::new(),
                        })
                        .into_iter()
                        .collect()
                } else {
                    Vec::new()
                };
                (u64::from(summary.failure_readable_end > 0), items, None)
            }
            _ => return Err(RuntimeClientError::ReadCutExpired),
        };
        Ok(super::super::checkpoint::CheckpointPage {
            collections: Vec::new(),
            cursor: RuntimeClientCursor::new(cut.revision),
            read_cut: cut.clone(),
            domain: domain.into(),
            total,
            items,
            next_offset,
        })
    }

    pub(super) fn materialize_content_bindings(
        &self,
        bindings: &mut [crate::observation::cut::ContentBinding],
    ) -> Result<(), RuntimeClientError> {
        for binding in bindings {
            if matches!(
                binding.content_ref.source,
                crate::observation::content::ContentSource::LivePublicationPrefix { .. }
            ) {
                continue;
            }
            let raw = if matches!(
                binding.content_ref.source,
                crate::observation::content::ContentSource::CanonicalMessageBlock { .. }
            ) && binding
                .path
                .last()
                .is_some_and(|field| field == "arguments")
            {
                self.store
                    .content_descriptor(
                        &binding.content_ref.source,
                        crate::observation::content::ContentRepresentation::RawJsonUtf8,
                    )
                    .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?
            } else {
                None
            };
            binding.content_ref = match raw {
                Some(reference) => reference,
                None => self
                    .store
                    .content_descriptor(
                        &binding.content_ref.source,
                        binding.content_ref.representation,
                    )
                    .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?
                    .ok_or(RuntimeClientError::ContentSourceUnavailable)?,
            };
        }
        Ok(())
    }

    /// Read a bounded checkpoint at one captured native cut. Trace enrichment
    /// happens outside the Projection lock and never substitutes a moving cursor.
    pub(crate) fn checkpoint_with_trace(
        &self,
        records: &[crate::observation::trace::TraceCursor],
    ) -> Result<super::super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        if records.len() > super::super::trace::TRACE_RECORD_LIMIT {
            return Err(RuntimeClientError::InvalidRequest {
                message: "Trace refresh limit is 8".into(),
            });
        }
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(
                super::super::checkpoint::CHECKPOINT_MATERIALIZATION_SECONDS,
            );
        let mut last = RuntimeClientError::ReadCutExpired;
        for attempt in 0..super::super::checkpoint::CHECKPOINT_ATTEMPTS {
            let progress = self.publication_gate.generation();
            if std::time::Instant::now() >= deadline {
                break;
            }
            let result = self
                .repair_failed_source_for_checkpoint(deadline)
                .and_then(|()| self.materialize_checkpoint(records, deadline));
            match result {
                Ok(checkpoint) => return Ok(checkpoint),
                Err(RuntimeClientError::ReadCutExpired) => {
                    last = RuntimeClientError::ReadCutExpired;
                    if attempt + 1 < super::super::checkpoint::CHECKPOINT_ATTEMPTS {
                        self.publication_gate.wait_after(progress, deadline);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(last)
    }

    /// Request one read on the existing worker and await its exact fence.
    /// The worker retains/cancels/joins physical Source work during shutdown.
    pub(super) fn repair_failed_source_for_checkpoint(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(), RuntimeClientError> {
        if self.read_only || !self.worker_started.load(Ordering::Acquire) {
            return Ok(());
        }
        let cut = {
            let mut state = self.lock_published_state();
            if self.pending.is_closed()
                || !self.pending.can_publish_prepared_reads()
                || state.projection.snapshot_ref().shutting_down
            {
                return Ok(());
            }
            state.projection.request_failed_read_repair()
        };
        let Some((cut, newly_requested)) = cut else {
            return Ok(());
        };
        if newly_requested {
            self.pending
                .push(ConversationObservation::JournalSourceReady);
        }
        #[cfg(test)]
        if let Some(hook) = self
            .publication_gate
            .repair_request_hook
            .lock()
            .unwrap()
            .take()
        {
            hook();
        }
        loop {
            let progress = self.publication_gate.generation();
            {
                let state = self.lock_published_state();
                if state.projection.read_domain_fence() != cut {
                    return Err(RuntimeClientError::ReadCutExpired);
                }
                if state.projection.read_domain_cut().is_none() {
                    drop(state);
                    // The read is folded once the domains are clean, but its
                    // publication is encoded outside the state lock and
                    // installed afterwards, before its transition completes.
                    // The checkpoint this repair serves must include that
                    // install, within the checkpoint's own deadline.
                    if self.publication_gate.wait_installed(deadline) {
                        return Ok(());
                    }
                    return Err(RuntimeClientError::ObservationUnavailable {
                        message: "checkpoint Source repair deadline exhausted before its publication installed".into(),
                    });
                }
                if self.pending.is_closed()
                    || !self.pending.can_publish_prepared_reads()
                    || state.projection.snapshot_ref().shutting_down
                {
                    return Err(RuntimeClientError::ObservationUnavailable {
                        message: "native Source repair awaits its observation owner".into(),
                    });
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(RuntimeClientError::ObservationUnavailable {
                    message: "checkpoint Source repair deadline exhausted".into(),
                });
            }
            self.publication_gate.wait_after(progress, deadline);
        }
    }

    pub(super) fn materialize_checkpoint(
        &self,
        records: &[crate::observation::trace::TraceCursor],
        deadline: std::time::Instant,
    ) -> Result<super::super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        let seed = {
            let state = self.state.lock().expect("published checkpoint state");
            #[cfg(test)]
            state.projection.probe_snapshot_enter();
            state.projection.validate_published_source()?;
            state.projection.checkpoint_seed()
        };
        let mut checkpoint = seed.checkpoint_candidate()?;
        self.materialize_content_bindings(&mut checkpoint.content)?;
        self.materialize_trace(&mut checkpoint.snapshot, &checkpoint.read_cut, records)?;
        self.materialize_auxiliary_audit_frontier(&mut checkpoint, deadline)?;
        if !records.is_empty() {
            checkpoint
                .collections
                .retain(|frontier| frontier.domain != "trace_updates");
            checkpoint
                .collections
                .push(crate::observation::cut::CollectionFrontier {
                    domain: "trace_updates".into(),
                    revision: checkpoint.read_cut.through,
                    total: checkpoint.snapshot.trace_updates.len() as u64,
                    loaded: checkpoint.snapshot.trace_updates.len() as u64,
                    next_offset: None,
                });
        }
        if std::time::Instant::now() >= deadline {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "checkpoint materialization deadline exhausted".into(),
            });
        }
        super::super::checkpoint::fit_checkpoint(&mut checkpoint)?;
        self.state
            .lock()
            .expect("published checkpoint fence")
            .projection
            .validate_checkpoint_cut(&checkpoint.read_cut)?;
        Ok(checkpoint)
    }

    fn materialize_auxiliary_audit_frontier(
        &self,
        checkpoint: &mut super::checkpoint::RuntimeClientCheckpoint,
        deadline: std::time::Instant,
    ) -> Result<(), RuntimeClientError> {
        if std::time::Instant::now() >= deadline {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "checkpoint auxiliary audit deadline exhausted".into(),
            });
        }
        let total = self
            .store
            .auxiliary_audit_total(crate::model::auxiliary::AuxiliaryAuditCut {
                journal: checkpoint.read_cut.through,
            })
            .map_err(RuntimeClientError::observation_store_error)?;
        if total > 9_007_199_254_740_991 {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "auxiliary audit total exceeds the protocol integer domain".into(),
            });
        }
        if std::time::Instant::now() >= deadline {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "checkpoint auxiliary audit deadline exhausted".into(),
            });
        }
        checkpoint
            .collections
            .retain(|frontier| frontier.domain != crate::runtime_client::auxiliary::DOMAIN);
        checkpoint
            .collections
            .push(super::checkpoint::CollectionFrontier {
                domain: crate::runtime_client::auxiliary::DOMAIN.into(),
                revision: checkpoint
                    .read_cut
                    .domain_revisions
                    .get("auxiliary")
                    .copied()
                    .unwrap_or(0),
                total,
                loaded: 0,
                next_offset: (total > 0).then_some(0),
            });
        Ok(())
    }

    pub(crate) fn checkpoint(
        &self,
    ) -> Result<super::super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        self.checkpoint_with_trace(&[])
    }

    pub(super) fn materialize_trace(
        &self,
        snapshot: &mut super::super::snapshot::RuntimeClientSnapshot,
        cut: &crate::observation::cut::CheckpointReadCut,
        records: &[crate::observation::trace::TraceCursor],
    ) -> Result<(), RuntimeClientError> {
        #[cfg(test)]
        if let Some(hook) = self.trace_cut_hook.lock().unwrap().take() {
            hook();
        }
        // The default head is published by the same bounded read-domain owner
        // as event metadata. Only an explicitly requested refresh is read here.
        snapshot.trace_updates = super::super::trace::TraceProjection::at_cut(
            self.store.as_ref(),
            cut.source_cut
                .as_ref()
                .ok_or(RuntimeClientError::ReadCutExpired)?,
        )
        .and_then(|projection| {
            projection.refresh(records, self.runtime.as_ref().map(|_| &*snapshot))
        })
        .map_err(RuntimeClientError::observation_store_error)?;
        Ok(())
    }
}
