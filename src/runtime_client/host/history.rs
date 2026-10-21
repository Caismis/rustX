//! Trace, transcript, conversation-window and request-history reads over
//! native durable authorities.
use super::{
    Arc, ClientInner, ModelRequest, RequestHistory, RequestHistoryError, RequestIdentity,
    RuntimeClientError, RuntimeClientResult, RuntimeClientTranscriptCursor,
    RuntimeClientTranscriptPage, TRANSCRIPT_PAGE_LIMIT_MAX,
};

pub(super) fn validate_transcript_page_limit(limit: usize) -> Result<(), RuntimeClientError> {
    if limit == 0 || limit > TRANSCRIPT_PAGE_LIMIT_MAX {
        return Err(RuntimeClientError::InvalidRequest {
            message: format!(
                "transcript page limit must be between 1 and {TRANSCRIPT_PAGE_LIMIT_MAX}"
            ),
        });
    }
    Ok(())
}
/// Shared durable transcript projection for root and exact owned children.
pub(super) fn read_transcript_page(
    store: &dyn crate::durable::ConversationStore,
    before: Option<RuntimeClientTranscriptCursor>,
    limit: usize,
    frontier: impl FnOnce()
        -> Result<crate::durable::observation::ObservationReadCut, RuntimeClientError>,
) -> Result<RuntimeClientTranscriptPage, RuntimeClientError> {
    let cut = frontier()?;
    let headers = store
        .transcript_header_page(
            &cut,
            before.map(Into::into),
            limit.min(crate::observation::bounded::CHECKPOINT_PAGE_ITEMS),
        )
        .map_err(|error| RuntimeClientError::RuntimeFailure {
            message: error.to_string(),
        })?;
    let mut page = crate::observation::transcript::transcript_header_view(headers);
    let through = cut.read.journal;
    super::super::response::decorate_through(store, &mut page, through).map_err(|error| {
        RuntimeClientError::RuntimeFailure {
            message: error.to_string(),
        }
    })?;
    Ok(page)
}
/// Both parent and child navigation read from their own canonical store.
pub(super) fn read_conversation_window(
    store: &dyn crate::durable::ConversationStore,
    at: &crate::durable::reading::ConversationWindowAt,
    limit: usize,
) -> Result<crate::runtime_client::snapshot::ConversationWindow, RuntimeClientError> {
    let failed = RuntimeClientError::observation_store_error;
    let read = store
        .read_window_headers(
            at,
            limit.min(crate::observation::bounded::CHECKPOINT_PAGE_ITEMS),
        )
        .map_err(failed)?;
    let mut page = crate::observation::transcript::transcript_header_view(read.page);
    super::super::response::decorate_window(store, &mut page, read.cut.read.journal)
        .map_err(failed)?;
    if !read
        .cut
        .read
        .reconstructible_from(&store.conversation_read_cut().map_err(failed)?)
    {
        return Err(RuntimeClientError::InvalidState {
            message: "conversation history was edited during the read; read it again".into(),
        });
    }
    Ok(crate::runtime_client::snapshot::ConversationWindow {
        cut: read.cut.read,
        page,
        newer_cursor: read.newer_cursor.map(Into::into),
        target: read.target,
        target_cursor: read.target_cursor.map(Into::into),
    })
}

impl ClientInner {
    /// Read Trace through its independent presentation owner. No cursor mutation.
    #[allow(clippy::needless_pass_by_value)] // Attachment dispatch owns request parameters.
    pub(crate) fn trace_page(
        &self,
        before: Option<crate::observation::trace::TraceCursor>,
        limit: usize,
        records: Vec<crate::observation::trace::TraceCursor>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if limit == 0 || limit > super::super::trace::TRACE_PAGE_LIMIT {
            return Err(RuntimeClientError::InvalidRequest {
                message: "Trace limit must be 1..=32".into(),
            });
        }
        if records.len() > super::super::trace::TRACE_RECORD_LIMIT {
            return Err(RuntimeClientError::InvalidRequest {
                message: "Trace refresh limit is 8".into(),
            });
        }
        let seed = self.lock_published_state().projection.checkpoint_seed();
        let checkpoint = seed.checkpoint_candidate()?;
        let current = checkpoint.snapshot;
        let projection = super::super::trace::TraceProjection::at_cut(
            self.store.as_ref(),
            checkpoint
                .read_cut
                .source_cut
                .as_ref()
                .ok_or(RuntimeClientError::ReadCutExpired)?,
        )
        .map_err(RuntimeClientError::observation_store_error)?;
        let mut page = projection
            .page(before.as_ref(), limit)
            .map_err(RuntimeClientError::observation_store_error)?;
        page.read_cut = Some(checkpoint.read_cut.clone());
        if self.runtime.is_some() {
            super::super::trace::repair_records(&mut page.records, &current);
        }
        page.updates = projection
            .refresh(&records, self.runtime.as_ref().map(|_| &current))
            .map_err(RuntimeClientError::observation_store_error)?;
        super::super::trace::ensure_page_bound(&page)
            .map_err(RuntimeClientError::observation_store_error)?;
        Ok(RuntimeClientResult::TracePage { page })
    }

    /// Locate an exact Tool occurrence through the complete native history.
    pub(crate) async fn trace_locate_tool(
        self: &Arc<Self>,
        locator: super::super::trace::TraceToolLocator,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let owner = Arc::clone(self);
        // The complete-history scan cannot occupy an async transport worker.
        tokio::task::spawn_blocking(move || {
            let (current, cut) = {
                let state = owner
                    .state
                    .lock()
                    .expect("runtime client host lock poisoned");
                (
                    state.projection.checkpoint_seed(),
                    state.projection.checkpoint_read_cut(),
                )
            };
            let current = current.checkpoint_candidate()?.snapshot;
            #[cfg(test)]
            crate::local_runtime::live_inspection::trace_read_gate(owner.store.conversation_id());
            let source_cut = cut
                .source_cut
                .as_ref()
                .ok_or(RuntimeClientError::ReadCutExpired)?;
            let projection =
                super::super::trace::TraceProjection::at_cut(owner.store.as_ref(), source_cut)
                    .map_err(RuntimeClientError::observation_store_error)?;
            let mut location = projection
                .locate_tool_at(&locator, source_cut)
                .map_err(RuntimeClientError::observation_store_error)?;
            if let Some(location) = &mut location {
                location.page.read_cut = Some(cut.clone());
            }
            if owner.runtime.is_some()
                && let Some(location) = &mut location
            {
                super::super::trace::repair_records(&mut location.page.records, &current);
            }
            Ok(RuntimeClientResult::TraceToolLocation { location })
        })
        .await
        .map_err(|_| RuntimeClientError::RuntimeFailure {
            message: "Trace tool locator failed".into(),
        })?
    }

    /// Reads the heavy detail of one exact Trace record identity.
    ///
    /// Detail is a pure historical read at the same kind of cut a page uses:
    /// it drains no observation, advances no cursor, consumes no pending
    /// work and settles nothing. Its lifecycle-bearing summary counterpart
    /// stays the authority for current state, so no live repair applies here
    /// — detail carries immutable historical facts only.
    #[allow(clippy::needless_pass_by_value)] // Attachment dispatch owns request parameters.
    pub(crate) fn projection_page(
        &self,
        after: crate::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let through = {
            let state = self.lock_state();
            state.projection.validate_published_source()?;
            state.projection.checkpoint_read_cut().through
        };
        super::super::projection_history::page(self.store.as_ref(), through, after, limit)
            .map(|page| RuntimeClientResult::ProjectionPage { page })
            .map_err(|e| RuntimeClientError::RuntimeFailure {
                message: e.to_string(),
            })
    }

    #[allow(clippy::needless_pass_by_value)] // Attachment dispatch owns request parameters.
    pub(crate) fn projection_detail(
        &self,
        id: crate::runtime::identity::CheckpointId,
        offset: u32,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let cut = {
            let state = self.lock_state();
            state.projection.validate_published_source()?;
            state.projection.checkpoint_read_cut()
        };
        super::super::projection_history::detail(self.store.as_ref(), &cut, &id, offset, limit)
            .map(|detail| RuntimeClientResult::ProjectionDetail {
                detail: Box::new(detail),
            })
            .map_err(|e| RuntimeClientError::RuntimeFailure {
                message: e.to_string(),
            })
    }

    #[allow(clippy::needless_pass_by_value)] // Attachment dispatch owns request parameters.
    pub(crate) fn trace_detail(
        &self,
        record_id: String,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if record_id.len() > 256 {
            return Err(RuntimeClientError::InvalidRequest {
                message: "Trace record identity is too long".into(),
            });
        }
        let sequence = record_id
            .strip_prefix("trace:")
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| RuntimeClientError::InvalidRequest {
                message: "invalid native Trace record identity".into(),
            })?;
        let domain = format!("trace.records.{sequence}.detail");
        self.domain_page(&domain, 0, 32)
    }

    /// Reads one bounded durable transcript page. The transcript cursor is
    /// intentionally distinct from the Runtime Client observation cursor.
    pub(crate) fn transcript_page(
        &self,
        before: Option<RuntimeClientTranscriptCursor>,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        validate_transcript_page_limit(limit)?;
        let page = read_transcript_page(self.store.as_ref(), before, limit, || {
            self.lock_published_state()
                .projection
                .checkpoint_read_cut()
                .source_cut
                .ok_or(RuntimeClientError::ReadCutExpired)
        })?;
        Ok(RuntimeClientResult::TranscriptPage { page })
    }

    pub(crate) fn conversation_turns(
        &self,
        offset: Option<usize>,
        limit: usize,
    ) -> Result<crate::observation::turn::ObservedTurnPage, RuntimeClientError> {
        self.store
            .observation_turns(offset.map(|value| value as u64), limit)
            .map_err(RuntimeClientError::observation_store_error)
    }

    pub(crate) fn conversation_window(
        &self,
        at: &crate::durable::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<crate::runtime_client::snapshot::ConversationWindow, RuntimeClientError> {
        read_conversation_window(self.store.as_ref(), at, limit)
    }

    /// Reads one bounded page of historical user-message boundaries, selected
    /// against the current committed Surface head.
    ///
    /// The revision travels with the page because it is part of what a `/fork`
    /// or `/tree` selection *means*: a later append must not silently change
    /// which conversation cut the reader chose.
    pub(crate) fn user_message_boundaries(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<
        (
            crate::conversation::SurfaceRevision,
            Vec<crate::local_runtime::session::observation::SessionUserMessageBoundaryObservation>,
            Option<usize>,
        ),
        RuntimeClientError,
    > {
        if limit == 0 || limit > 32 {
            return Err(RuntimeClientError::InvalidRequest {
                message: "boundary pages require 1..32 rows".into(),
            });
        }
        let cut = {
            let state = self.state.lock().expect("native projection lock");
            state.projection.validate_published_source()?;
            state.projection.checkpoint_read_cut()
        };
        let source = cut
            .source_cut
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let page = self
            .store
            .surface_boundary_header_page(source.surface_revision, offset as u64, limit)
            .map_err(RuntimeClientError::observation_store_error)?;
        let boundaries=page.boundaries.into_iter().map(|boundary|{
            let header=boundary.header;
            let locator=header.locator.ok_or(RuntimeClientError::ReadCutExpired)?;
            let message=match header.display{Some(crate::message::types::MessageBlock::User(user))=>Some(user),None=>None,_=>return Err(RuntimeClientError::ReadCutExpired)};
            Ok(crate::local_runtime::session::observation::SessionUserMessageBoundaryObservation{surface_revision:boundary.surface_revision,locator,message,block_count:header.block_count,content:header.content,collections:header.collections})
        }).collect::<Result<Vec<_>,RuntimeClientError>>()?;
        self.state
            .lock()
            .expect("native projection lock")
            .projection
            .validate_checkpoint_cut(&cut)?;
        Ok((
            page.through,
            boundaries,
            page.next_offset
                .map(usize::try_from)
                .transpose()
                .map_err(|_| RuntimeClientError::ReadCutExpired)?,
        ))
    }

    /// Returns a durable request-history read handle owned by the
    /// conversation runtime.
    ///
    /// The durable `ConversationStore` owns these snapshots. The returned
    /// value is a read-only handle; each historical read is resolved through
    /// the runtime authority and does not create another conversation or
    /// transcript authority.
    #[must_use]
    pub(crate) fn request_history(&self) -> RequestHistory {
        RequestHistory::new(self.store.clone())
    }

    /// Reconstructs one retained provider-neutral request from durable facts.
    ///
    /// # Errors
    ///
    /// Returns a lookup or historical reconstruction error for an unknown or
    /// invalid request.
    pub(crate) fn reconstruct_request(
        &self,
        identity: &RequestIdentity,
    ) -> Result<ModelRequest, RequestHistoryError> {
        self.request_history().reconstruct(identity)
    }
}
