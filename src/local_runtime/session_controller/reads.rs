//! Coherent cold session, history, statistics, trace, archive, and preview reads.

use super::super::session::DisplayPreviewSubject;
use super::{
    DisplayPreviewRepair, DisplayPreviewRepairReport, SessionController, SessionError, SessionId,
    SessionListPage, SessionNodeId, SessionSnapshot, SessionStatisticsReading, SessionSummary,
};
use crate::durable::ObservationStore;

impl SessionController {
    pub(crate) async fn thread(
        &self,
        id: &crate::runtime::identity::ConversationId,
    ) -> Result<super::super::session::threads::Thread, SessionError> {
        self.catalog.lock().await.thread(id)
    }
    pub(crate) async fn threads(
        &self,
        offset: usize,
        limit: usize,
        archived: bool,
    ) -> Result<
        (
            Vec<super::super::session::threads::ThreadSummary>,
            Option<usize>,
        ),
        SessionError,
    > {
        self.catalog.lock().await.threads(offset, limit, archived)
    }
    pub(crate) async fn update_thread(
        &self,
        id: &crate::runtime::identity::ConversationId,
        update: super::super::session::threads::ThreadUpdate,
    ) -> Result<super::super::session::threads::Thread, SessionError> {
        self.catalog.lock().await.update_thread(id, update)
    }
    /// Prepare historical export through native authorities, without residency.
    /// # Errors
    /// Missing history, ownership conflicts and cancellation fail explicitly.
    pub async fn prepare_archive(
        &self,
        id: SessionId,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<
        crate::session_archive::SessionArchiveCut,
        crate::session_archive::SessionArchivePrepareError,
    > {
        let root = self.catalog.lock().await.controller()?.root().to_path_buf();
        tokio::task::spawn_blocking(move || {
            crate::session_archive::SessionArchiveProducer::prepare(&root, &id, &cancel)
        })
        .await
        .map_err(|_| crate::session_archive::SessionArchivePrepareError::Storage)?
    }

    /// # Errors
    /// Unknown identities are rejected without configuration resolution.
    pub async fn read_session(&self, id: &SessionId) -> Result<SessionSnapshot, SessionError> {
        self.catalog.lock().await.snapshot(id)
    }
    /// Read a bounded durable window without composing a runtime or resolving tools.
    pub(crate) async fn read_history(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        at: crate::durable::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<
        (
            crate::runtime::identity::ConversationId,
            crate::runtime_client::snapshot::ConversationWindow,
        ),
        SessionError,
    > {
        let access = self.acquire_read(id, node).await?;
        tokio::task::spawn_blocking(move || {
            let store = crate::durable::SqliteConversationStore::open_existing(
                access.conversation_id.clone(),
                &access.database_path,
            )
            .map_err(SessionError::Store)?
            .with_lifecycle(access.allocation);
            let read = store
                .read_window_headers(&at, limit.min(32))
                .map_err(SessionError::Store)?;
            let mut page = crate::observation::transcript::transcript_header_view(read.page);
            crate::runtime_client::response::decorate_window(
                &store,
                &mut page,
                read.cut.read.journal,
            )
            .map_err(SessionError::Store)?;
            if !read
                .cut
                .read
                .reconstructible_from(&store.conversation_read_cut().map_err(SessionError::Store)?)
            {
                return Err(SessionError::Catalog {
                    detail: "History changed during read".into(),
                });
            }
            Ok((
                access.conversation_id,
                crate::runtime_client::snapshot::ConversationWindow {
                    cut: read.cut.read,
                    page,
                    newer_cursor: read.newer_cursor.map(Into::into),
                    target: read.target,
                    target_cursor: read.target_cursor.map(Into::into),
                },
            ))
        })
        .await
        .map_err(|error| SessionError::Catalog {
            detail: error.to_string(),
        })?
    }
    pub(crate) async fn read_statistics(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
    ) -> Result<
        Result<SessionStatisticsReading, crate::runtime_client::RuntimeClientError>,
        SessionError,
    > {
        let access = self.acquire_read(id, node).await?;
        tokio::task::spawn_blocking(move || {
            let store = crate::durable::SqliteConversationStore::open_existing(
                access.conversation_id.clone(),
                &access.database_path,
            )
            .map_err(SessionError::Store)?
            .with_lifecycle(access.allocation);
            let reading = || {
                use crate::runtime_client::RuntimeClientError;
                let read_cut = crate::runtime_client::trace_observation::history_cut(&store)?;
                let through = read_cut.through;
                let occupancy = crate::observation::occupancy::read(&store, through)
                    .map_err(RuntimeClientError::observation_store_error)?;
                let statistics = crate::runtime_client::response::statistics(&store, through)
                    .map_err(RuntimeClientError::observation_store_error)?;
                Ok(SessionStatisticsReading {
                    conversation_id: access.conversation_id,
                    statistics,
                    occupancy,
                    read_cut,
                })
            };
            Ok(reading())
        })
        .await
        .map_err(|error| SessionError::Catalog {
            detail: error.to_string(),
        })?
    }
    /// Durable inspection uses the same projection as live trace, without runtime startup.
    pub(crate) async fn read_trace<T: Send + 'static>(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        read: impl FnOnce(
            crate::runtime_client::trace::TraceProjection<'_>,
        ) -> Result<T, crate::durable::ConversationStoreError>
        + Send
        + 'static,
    ) -> Result<(crate::runtime::identity::ConversationId, T), SessionError> {
        let access = self.acquire_read(id, node).await?;
        tokio::task::spawn_blocking(move || {
            let store = crate::durable::SqliteConversationStore::open_existing(
                access.conversation_id.clone(),
                &access.database_path,
            )
            .map_err(SessionError::Store)?
            .with_lifecycle(access.allocation);
            let projection = crate::runtime_client::trace::TraceProjection::new(&store)
                .map_err(SessionError::Store)?;
            let result = read(projection).map_err(SessionError::Store)?;
            Ok((access.conversation_id, result))
        })
        .await
        .map_err(|error| SessionError::Catalog {
            detail: error.to_string(),
        })?
    }
    /// Execute a bounded native trace read under this exact node's storage access.
    pub(crate) async fn read_trace_observation<T: Send + 'static>(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        read: impl FnOnce(
            &dyn crate::durable::ConversationStore,
        ) -> Result<T, crate::runtime_client::RuntimeClientError>
        + Send
        + 'static,
    ) -> Result<
        (
            crate::runtime::identity::ConversationId,
            Result<T, crate::runtime_client::RuntimeClientError>,
        ),
        SessionError,
    > {
        let selected = self.catalog.lock().await.lineage(id, node)?.0;
        let access = self.acquire_read(id, Some(&selected.id)).await?;
        let lineage_request = if selected.parent.is_some() {
            self.lineage_request(id, &selected.id).await?
        } else {
            None
        };
        #[cfg(test)]
        let gate = self.trace_observation_read_gate.lock().unwrap().clone();
        tokio::task::spawn_blocking(move || {
            let store = crate::durable::SqliteConversationStore::open_existing(
                access.conversation_id.clone(),
                &access.database_path,
            )
            .map_err(SessionError::Store)?
            .with_lifecycle(access.allocation)
            .with_lineage_request(lineage_request);
            #[cfg(test)]
            if let Some(gate) = gate {
                gate.enter();
            }
            let result = read(&store);
            Ok((access.conversation_id, result))
        })
        .await
        .map_err(|error| SessionError::Catalog {
            detail: error.to_string(),
        })?
    }
    /// Exact durable display metadata; does not resolve configuration or compose a runtime.
    /// # Errors
    /// Unknown/deleting identities are returned.
    pub async fn read_session_summary(
        &self,
        id: &SessionId,
    ) -> Result<SessionSummary, SessionError> {
        let snapshot = self.catalog.lock().await.clone();
        snapshot.summary(id)
    }
    /// # Errors
    /// Invalid pagination is rejected; the projection opens no conversation store.
    pub async fn list_sessions(
        &self,
        query: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<SessionListPage, SessionError> {
        let snapshot = self.catalog.lock().await.clone();
        snapshot.list_page(query, offset, limit)
    }
    /// # Errors
    /// Unknown identity, invalid name and pre/post visibility failures are distinct.
    pub async fn rename_session(
        &self,
        id: &SessionId,
        name: &str,
    ) -> Result<SessionSnapshot, SessionError> {
        self.catalog.lock().await.rename(id, name)
    }
    /// Publishes the Session's derived display projection through the once-only
    /// catalog mutation. Returns `true` when this call committed the value,
    /// `false` when a projection was already present (no commit, no
    /// generation bump). `updated_at` is deliberately untouched, mirroring
    /// the `commit_uploads` precedent: the projection is derived display
    /// metadata, not a metadata publication.
    /// # Errors
    /// Unknown (e.g. deleted) identities and catalog commit failures are explicit.
    pub async fn publish_display_preview(
        &self,
        id: &SessionId,
        preview: &str,
    ) -> Result<bool, SessionError> {
        self.catalog
            .lock()
            .await
            .publish_display_preview(id, preview)
    }

    /// The one explicit, idempotent display-preview repair algorithm, run by
    /// the reopen/compose/recovery seams — never by listing or summary
    /// projection.
    ///
    /// Two phases keep the catalog mutex out of store I/O: under the lock the
    /// repair only checks the persisted projection (present means
    /// [`DisplayPreviewRepair::AlreadyPresent`]) and clones the catalog
    /// handle; the root conversation store is then opened outside the lock,
    /// and the once-only publish re-checks absence under the lock, so racing
    /// repairs converge instead of double-writing.
    ///
    /// A legitimate `None` — no ordinary user boundary yet, or a first
    /// boundary without renderable text — reports
    /// [`DisplayPreviewRepair::EmptySubject`] and writes nothing: repair must
    /// never manufacture rewrites or move the subject to a later message.
    /// # Errors
    /// Genuine store/catalog failures are explicit; a Session that vanished
    /// mid-repair is [`DisplayPreviewRepair::SessionGone`], not an error.
    pub async fn repair_display_preview(
        &self,
        id: &SessionId,
    ) -> Result<DisplayPreviewRepair, SessionError> {
        Ok(self.repair_display_preview_report(id).await?.repair)
    }
    /// The repair algorithm plus the subject its read observed; see
    /// [`SessionController::repair_display_preview`].
    pub(crate) async fn repair_display_preview_report(
        &self,
        id: &SessionId,
    ) -> Result<DisplayPreviewRepairReport, SessionError> {
        let report = |repair, subject| DisplayPreviewRepairReport { repair, subject };
        let snapshot = {
            let catalog = self.catalog.lock().await;
            match catalog.summary(id) {
                Ok(summary) if summary.preview.is_some() => {
                    return Ok(report(DisplayPreviewRepair::AlreadyPresent, None));
                }
                Ok(_) => {}
                Err(SessionError::UnknownSession { .. } | SessionError::DeletingSession { .. }) => {
                    return Ok(report(DisplayPreviewRepair::SessionGone, None));
                }
                Err(error) => return Err(error),
            }
            catalog.clone()
        };
        // Outside the catalog mutex: the only store I/O of the repair.
        let subject = match snapshot.display_preview_subject(id) {
            Ok(subject) => subject,
            Err(SessionError::UnknownSession { .. }) => {
                return Ok(report(DisplayPreviewRepair::SessionGone, None));
            }
            Err(error) => return Err(error),
        };
        let DisplayPreviewSubject::Derived(preview) = &subject else {
            return Ok(report(DisplayPreviewRepair::EmptySubject, Some(subject)));
        };
        // The one test-only interleaving seam, shared with the one-shot
        // publisher: the subject is derived and the projection is still known
        // absent, and no catalog mutex is held. See
        // `session_display_projection::publication_test_support`.
        #[cfg(test)]
        super::super::session_display_projection::publication_test_support::park(id).await;
        match self.publish_display_preview(id, preview).await {
            Ok(true) => Ok(report(DisplayPreviewRepair::Published, Some(subject))),
            // A racing publisher/repair committed first; the projection is
            // settled all the same.
            Ok(false) => Ok(report(DisplayPreviewRepair::AlreadyPresent, Some(subject))),
            Err(SessionError::UnknownSession { .. } | SessionError::DeletingSession { .. }) => {
                Ok(report(DisplayPreviewRepair::SessionGone, Some(subject)))
            }
            Err(error) => Err(error),
        }
    }
    pub(crate) async fn repair_activity(&self, id: &SessionId) -> Result<(), SessionError> {
        let snapshot = self.catalog.lock().await.clone();
        let target = id.clone();
        let time = tokio::task::spawn_blocking(move || snapshot.activity_subject(&target))
            .await
            .map_err(|error| SessionError::Catalog {
                detail: error.to_string(),
            })??;
        if let Some(time) = time {
            self.catalog.lock().await.publish_activity(id, time)?;
        }
        Ok(())
    }
}
