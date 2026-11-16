//! User-root durable Session controller. No runtime, resolver or client focus is owned here.
use super::session::{
    DisplayPreviewSubject, SessionCatalog, SessionError, SessionId, SessionListPage, SessionNode,
    SessionNodeId, SessionPersistentState, SessionSnapshot, SessionSummary,
};
use crate::runtime::local_storage::{ConversationAccess, ProductController};
use std::sync::Arc;

/// Durable transition, including the editor payload of a fork whose visibility
/// committed even if the final durability barrier failed. No runtime routing.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTransitionResult {
    pub session: SessionSnapshot,
    pub editor_content: Option<Vec<super::session::uploads::UserInputBlock>>,
    pub durability_diagnostic: Option<String>,
}

/// One native cold statistics cut, independent of runtime admission.
#[derive(Debug)]
pub(crate) struct SessionStatisticsReading {
    pub(crate) conversation_id: crate::runtime::identity::ConversationId,
    pub(crate) statistics: crate::observation::statistics::ConversationStatistics,
    pub(crate) occupancy: Option<crate::observation::occupancy::RuntimeClientContextOccupancy>,
    pub(crate) read_cut: crate::observation::cut::CheckpointReadCut,
}

/// Retained allocation admission. Different Conversations may be held together.
/// Delete either wins before acquisition (explicit failure), or cannot pass
/// preflight until this access and all storage/runtime clones release it.
#[derive(Debug)]
pub struct SessionAccess {
    pub session: SessionSnapshot,
    pub node: SessionNode,
    pub settings: SessionPersistentState,
    pub settings_revision: u64,
    /// Identity-derived database path, valid while allocation access is retained.
    pub database_path: std::path::PathBuf,
    pub allocation: Arc<ConversationAccess>,
}

#[derive(Debug)]
pub(crate) struct ScheduleReadPlan {
    pub(crate) catalog: Arc<tokio::sync::Mutex<SessionCatalog>>,
    pub(crate) session: SessionId,
    pub(crate) node: SessionNodeId,
    pub(crate) conversation_id: crate::runtime::identity::ConversationId,
    pub(crate) database_path: std::path::PathBuf,
    pub(crate) controller: Arc<ProductController>,
}
impl ScheduleReadPlan {
    pub(crate) fn open(self) -> Result<SessionReadAccess, SessionError> {
        let allocation = ConversationAccess::existing_for_catalog_read(
            self.controller,
            self.database_path.parent().expect("allocation"),
            || {
                self.catalog
                    .blocking_lock()
                    .validate_schedule_read(&self.session, &self.node, &self.conversation_id)
                    .map_err(std::io::Error::other)
            },
        )
        .map_err(|e| SessionError::Catalog {
            detail: e.to_string(),
        })?;
        Ok(SessionReadAccess {
            conversation_id: self.conversation_id,
            database_path: self.database_path,
            allocation: Arc::new(allocation),
        })
    }
}

/// Allocation retention for display reads. It never clones native Node origin
/// strings or authored Session model/settings bodies.
#[derive(Debug)]
pub(crate) struct SessionReadAccess {
    pub(crate) conversation_id: crate::runtime::identity::ConversationId,
    pub(crate) database_path: std::path::PathBuf,
    pub(crate) allocation: Arc<ConversationAccess>,
}
#[derive(Debug, Clone)]
pub(crate) struct SessionReadOrigin {
    pub(crate) source_session: SessionId,
    pub(crate) source_node: SessionNodeId,
    pub(crate) source_surface_revision: crate::conversation::SurfaceRevision,
    pub(crate) source_locator: crate::observation::content::CanonicalContentLocator,
    pub(crate) side: super::session::LineageSide,
}

/// The outcome of the explicit display-preview repair seam (Issue #386).
///
/// Repair is idempotent by construction: it never rewrites a settled
/// projection, never shifts the subject to a later message, and never
/// resurrects a deleted Session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayPreviewRepair {
    /// The persisted projection was already present (or a racing publisher
    /// won); nothing was written.
    AlreadyPresent,
    /// The repair read derived a line and the once-only catalog mutation
    /// committed it.
    Published,
    /// The root lineage legitimately has no projection: either it has no
    /// ordinary user boundary yet, or its first boundary has no renderable
    /// text. No write happened, and none may happen later for the second
    /// case.
    EmptySubject,
    /// The Session vanished between the repair read and publication
    /// (deletion race). This is not an error.
    SessionGone,
}

/// The repair outcome plus the subject the repair read observed, so a caller
/// arming the one-shot publisher can distinguish "no boundary yet" (arm) from
/// "boundary without renderable text" (settled `None`, never arm).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DisplayPreviewRepairReport {
    pub(crate) repair: DisplayPreviewRepair,
    /// The subject read, when a repair read happened (`None` when the
    /// projection was already present or the Session was already gone).
    pub(crate) subject: Option<DisplayPreviewSubject>,
}

/// Durable native metadata/allocation access. Deletion execution and frozen
/// cleanup authority remain crate-private; App Server exposes bounded DTOs only.
///
/// ```compile_fail
/// use rustx::local_runtime::session::deletion::DeletionRecord;
/// ```
/// ```compile_fail
/// use rustx::local_runtime::{session_controller::SessionController, SessionId};
/// fn preview(controller: &SessionController, id: &SessionId) {
///     let _ = controller.delete_preview(id);
/// }
/// ```
/// ```compile_fail
/// use rustx::local_runtime::{session_controller::SessionController, SessionId};
/// fn delete(controller: &SessionController, id: &SessionId) {
///     let _ = controller.delete_session(id, "revision");
/// }
/// ```
/// ```compile_fail
/// use rustx::local_runtime::{session_controller::SessionController, SessionId};
/// fn recover(controller: &SessionController, id: &SessionId) {
///     let _ = controller.recover_deletion(id);
/// }
/// ```
///
/// ```rust
/// use rustx::local_runtime::{
///     SessionId, SessionTransitionResult,
///     session::{SessionListPage, SessionSnapshot},
///     session_controller::SessionController,
/// };
///
/// let root = tempfile::tempdir().expect("tempdir");
/// let controller = SessionController::open(root.path()).expect("open controller");
/// let runtime = tokio::runtime::Builder::new_current_thread()
///     .enable_all()
///     .build()
///     .expect("tokio runtime");
/// let page: SessionListPage = runtime
///     .block_on(controller.list_sessions(None, 0, 10))
///     .expect("list sessions");
/// assert!(page.sessions.is_empty());
/// let _ = (
///     SessionController::read_session,
///     SessionController::recover_session_storage,
///     Option::<(SessionId, SessionSnapshot, SessionTransitionResult)>::None,
/// );
/// ```
#[derive(Clone, Debug)]
pub struct SessionController {
    #[cfg(test)]
    pub(crate) upload_commit_gate:
        Arc<std::sync::Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>>,
    #[cfg(test)]
    pub(crate) copy_upload_gate:
        Arc<std::sync::Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>>,
    #[cfg(test)]
    pub(crate) copy_publication_gate:
        Arc<std::sync::Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>>,
    pub(crate) catalog: Arc<tokio::sync::Mutex<SessionCatalog>>,
    /// The catalog's post-commit summary invalidation log, held directly so
    /// observers can subscribe without taking the catalog mutex.
    summary_invalidations: Arc<super::session::SessionSummaryInvalidations>,
    /// Process-local adopted descriptors belong to Session identity, not residency.
    pub(crate) configuration_bindings: Arc<
        std::sync::Mutex<
            std::collections::BTreeMap<SessionId, super::configuration::AdmittedSessionConfig>,
        >,
    >,
    // Allocation of the one process runtime owner, not ownership of its registry.
    pub(crate) runtime_owner: Arc<std::sync::OnceLock<()>>,
    pub(crate) preparation: Arc<tokio::sync::Mutex<()>>,
    /// Test-only wait observer for the preparation owner. It is notified when a
    /// task's preparation-lock future is polled and returns `Pending`, i.e. the
    /// task is provably registered as a waiter on the mutex. It carries no
    /// production state and cannot change scheduling.
    #[cfg(test)]
    pub(crate) create_preparation_wait: Arc<tokio::sync::Notify>,
    #[cfg(test)]
    pub(crate) create_gate:
        Arc<std::sync::Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>>,
    #[cfg(test)]
    cleanup_gate: Arc<std::sync::Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>>,
    #[cfg(test)]
    copy_gate: Arc<std::sync::Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>>,
    #[cfg(test)]
    trace_observation_read_gate:
        Arc<std::sync::Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>>,
}

mod lineage;
mod reads;
mod settings;
#[cfg(test)]
mod tests;
mod uploads;

impl SessionController {
    /// Open one root without selecting, resolving, or composing a Session.
    /// # Errors
    /// Competing controllers, invalid schemas and storage failures are explicit.
    pub fn open(root: &std::path::Path) -> Result<Self, SessionError> {
        let controller =
            Arc::new(
                ProductController::acquire(root).map_err(|e| SessionError::Catalog {
                    detail: e.to_string(),
                })?,
            );
        let mut catalog = match SessionCatalog::open_existing(controller.root())? {
            Some(catalog) => catalog,
            None => SessionCatalog::empty(&controller)?,
        };
        catalog.retain_lifecycle(controller);
        catalog.recover_upload_preparations()?;
        // Recover derived activity after a process exit, before serving catalog pages.
        // This inspects durable messages without composing or selecting runtimes.
        for id in catalog.persisted_session_ids() {
            match catalog.activity_subject(&id).and_then(|time| match time {
                Some(time) => catalog.publish_activity(&id, time).map(|_| ()),
                None => Ok(()),
            }) {
                Ok(()) => {}
                Err(error) => tracing::warn!(%id, %error, "Session activity repair failed"),
            }
        }
        Ok(Self::new(catalog))
    }
    pub(crate) fn new(catalog: SessionCatalog) -> Self {
        let summary_invalidations = catalog.summary_invalidations();
        Self {
            summary_invalidations,
            #[cfg(test)]
            upload_commit_gate: Arc::default(),
            #[cfg(test)]
            copy_upload_gate: Arc::default(),
            #[cfg(test)]
            copy_publication_gate: Arc::default(),
            catalog: Arc::new(tokio::sync::Mutex::new(catalog)),
            configuration_bindings: Arc::default(),
            runtime_owner: Arc::default(),
            preparation: Arc::new(tokio::sync::Mutex::new(())),
            #[cfg(test)]
            create_preparation_wait: Arc::new(tokio::sync::Notify::new()),
            #[cfg(test)]
            create_gate: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            cleanup_gate: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            copy_gate: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            trace_observation_read_gate: Arc::new(std::sync::Mutex::new(None)),
        }
    }
    #[cfg(test)]
    pub(crate) fn install_delete_cleanup_gate(
        &self,
        gate: Arc<crate::runtime::conversation_runtime::Gate>,
    ) {
        *self.cleanup_gate.lock().unwrap() = Some(gate);
    }
    #[cfg(test)]
    pub(crate) fn install_trace_observation_read_gate(
        &self,
        gate: Arc<crate::runtime::conversation_runtime::Gate>,
    ) {
        *self.trace_observation_read_gate.lock().unwrap() = Some(gate);
    }

    /// The shared catalog, downgraded for the one-shot display-projection
    /// publisher (Issue #386). The publisher's weak handle never extends
    /// native product ownership past the Session's own lifetime: when the
    /// product drops, the catalog is released immediately, and a publisher
    /// still parked on its observation queue cannot keep the storage locked.
    pub(crate) fn downgrade_catalog(&self) -> std::sync::Weak<tokio::sync::Mutex<SessionCatalog>> {
        std::sync::Arc::downgrade(&self.catalog)
    }
    /// The post-commit summary invalidation log of this product root
    /// (Issue #386), for observers that translate native metadata
    /// invalidation onto a client transport.
    pub(crate) fn summary_invalidations(&self) -> Arc<super::session::SessionSummaryInvalidations> {
        self.summary_invalidations.clone()
    }

    /// Prepare valid private storage outside the metadata lock, then atomically
    /// publish. There is no reuse of another Session, even an unused one.
    /// # Errors
    /// Invalid context, preparation or catalog commit failures are returned.
    /// Full visibility/durability outcome for callers publishing editor/routing projections.
    /// # Errors
    /// Preparation and pre-visibility failures are ordinary errors.
    pub async fn create_session(
        &self,
        settings: SessionPersistentState,
    ) -> Result<SessionTransitionResult, SessionError> {
        self.create_session_with_binding(settings, Vec::new(), None, |_| {})
            .await
    }

    /// Install the captured process authority before publishing listable identity.
    /// No await separates installation, publication and pre-visibility rollback.
    pub(crate) async fn create_session_with_binding(
        &self,
        settings: SessionPersistentState,
        dynamic_tools: Vec<crate::tools::host::HostToolDefinition>,
        binding: Option<super::configuration::AdmittedSessionConfig>,
        published: impl FnOnce(&SessionTransitionResult),
    ) -> Result<SessionTransitionResult, SessionError> {
        if !settings.cwd.is_absolute() {
            return Err(SessionError::Catalog {
                detail: "Session cwd must be absolute".into(),
            });
        }
        // Blocking preparation outlives a cancelled async caller. Transfer the
        // allocator guard into that work and retain it through publication.
        let preparation = self.acquire_preparation().await;
        // The real controller snapshots (clones) the whole Catalog document
        // before preparation. Instrument that clone so the stage profile does
        // not bypass it and later claim catalog cloning was measured.
        let snapshot = {
            let catalog = self.catalog.lock().await;
            #[cfg(test)]
            {
                crate::local_runtime::session::create_profile::timed(
                    |times: &mut crate::local_runtime::session::create_profile::StageTimes| {
                        &mut times.catalog_snapshot_ns
                    },
                    || (*catalog).clone(),
                )
            }
            #[cfg(not(test))]
            {
                (*catalog).clone()
            }
        };
        #[cfg(test)]
        let gate = self
            .create_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let (_preparation, prepared) = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            if let Some(gate) = gate {
                gate.enter();
            }
            let prepared = snapshot.prepare_session(&settings, &[]);
            (preparation, prepared)
        })
        .await
        .map_err(|e| SessionError::Catalog {
            detail: e.to_string(),
        })?;
        let mut prepared = prepared?;
        prepared.dynamic_tools = dynamic_tools;
        let mut catalog = self.catalog.lock().await;
        // Keep provisional bindings private from configuration coordination as
        // well as attachment until the catalog visibility decision is complete.
        let mut bindings = self
            .configuration_bindings
            .lock()
            .expect("Session configuration bindings");
        if let Some(binding) = binding {
            bindings.insert(prepared.session_id.clone(), binding);
        }
        let (session, durability_diagnostic) =
            match catalog.publish_session(&prepared, super::session::SessionNodeOrigin::New) {
                Ok(session) => (session, None),
                Err(error) if error.committed() => (
                    catalog.snapshot(&prepared.session_id)?,
                    Some(error.to_string()),
                ),
                Err(error) => {
                    bindings.remove(&prepared.session_id);
                    return Err(error);
                }
            };
        let result = SessionTransitionResult {
            session,
            editor_content: None,
            durability_diagnostic,
        };
        // Synchronous ownership registration shares the catalog visibility cut.
        // Observers cannot resolve the new identity until this lock is released.
        published(&result);
        Ok(result)
    }
    /// Acquire the preparation owner. In test builds, notify the wait observer
    /// exactly when this task's lock future is polled and returns `Pending`,
    /// which is the deterministic "this task is registered as a waiter on the
    /// preparation mutex" boundary. The production path is unchanged.
    async fn acquire_preparation(&self) -> tokio::sync::OwnedMutexGuard<()> {
        #[cfg(test)]
        {
            let mut lock = std::pin::pin!(self.preparation.clone().lock_owned());
            std::future::poll_fn(|context| match lock.as_mut().poll(context) {
                std::task::Poll::Ready(guard) => std::task::Poll::Ready(guard),
                std::task::Poll::Pending => {
                    self.create_preparation_wait.notify_one();
                    std::task::Poll::Pending
                }
            })
            .await
        }
        #[cfg(not(test))]
        {
            self.preparation.clone().lock_owned().await
        }
    }

    /// Resolve identity and retain existing native destructive exclusion.
    /// No runtime is composed. #287 owns residency and single-writer admission.
    /// # Errors
    /// Unknown/deleted identities and conflicting deletion preflight fail closed.
    pub async fn acquire_session(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
    ) -> Result<SessionAccess, SessionError> {
        self.catalog.lock().await.acquire_session(id, node)
    }
    pub(crate) async fn acquire_read(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
    ) -> Result<SessionReadAccess, SessionError> {
        self.catalog.lock().await.acquire_read(id, node)
    }
    pub(crate) async fn acquire_read_origin(
        &self,
        id: &SessionId,
        node: &SessionNodeId,
    ) -> Result<(SessionReadAccess, SessionReadOrigin), SessionError> {
        let catalog = self.catalog.lock().await;
        let origin = catalog.read_origin(id, node)?;
        let access = catalog.acquire_read(id, Some(node))?;
        Ok((access, origin))
    }
    /// Resolve durable identity without taking allocation authority. The runtime
    /// manager must register its flight before acquiring the selected allocation.
    pub(crate) async fn resolve_session_target(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
    ) -> Result<SessionNode, SessionError> {
        let node = self
            .catalog
            .lock()
            .await
            .lineage(id, node)
            .map(|(node, _)| node)?;
        if node.archived {
            return Err(SessionError::Catalog {
                detail: "archived Threads cannot execute".into(),
            });
        }
        Ok(node)
    }
    /// Explicit cold storage recovery for one Session's graph and owned children.
    /// Never performed by opening, listing or reading the catalog.
    ///
    /// Recovery ends with the same display-preview repair every other
    /// reopen/compose seam runs: a Session whose durable history already
    /// holds its first ordinary user message leaves recovery with its
    /// projection backfilled, idempotently.
    /// # Errors
    /// Missing identity or allocation/deletion conflicts fail closed.
    pub async fn recover_session_storage(&self, id: &SessionId) -> Result<(), SessionError> {
        let snapshot = self.catalog.lock().await.clone();
        let controller = snapshot.controller()?;
        let target = id.clone();
        tokio::task::spawn_blocking(move || snapshot.recover_session_storage(&target, &controller))
            .await
            .map_err(|error| SessionError::Catalog {
                detail: error.to_string(),
            })??;
        self.repair_activity(id).await?;
        self.repair_display_preview(id).await.map(|_| ())
    }

    /// Finite destructive preview. Every guard is released before returning.
    pub(crate) async fn delete_preview(
        &self,
        id: &SessionId,
    ) -> super::session::deletion::SessionDeleteResult {
        let snapshot = self.catalog.lock().await.clone();
        snapshot.delete_preview(id)
    }
    /// # Errors
    /// A pre-visibility catalog error leaves the Session live.
    // Durable primitive: production callers must be SessionRuntimeManager after
    // writer-absence proof. Direct callers in tests isolate catalog durability.
    pub(crate) async fn delete_session(
        &self,
        id: &SessionId,
        revision: &str,
    ) -> Result<super::session::deletion::SessionDeleteResult, SessionError> {
        let work = match self.catalog.lock().await.commit_delete(id, revision)? {
            Ok(work) => work,
            Err(result) => return Ok(result),
        };
        Ok(self.clean_deletion(work).await)
    }
    /// Confirm an already observed absent identity after a possibly uncertain final rename.
    /// Session identities are never reused; this grants no cleanup authority.
    pub(crate) async fn confirm_deletion_absence(
        &self,
        id: &SessionId,
    ) -> super::session::deletion::SessionDeleteResult {
        use super::session::deletion::SessionDeleteResult;
        match self.catalog.lock().await.confirm_catalog_durability() {
            Ok(()) => SessionDeleteResult::NotFound {
                session_id: id.clone(),
            },
            Err(error) => SessionDeleteResult::CommittedDurabilityUncertain {
                session_id: id.clone(),
                detail: error.to_string(),
            },
        }
    }
    /// Retry only an observed frozen cleanup record, outside the metadata mutex.
    /// Public callers must first reconcile through `SessionRuntimeManager`. A racing
    /// cleanup may already have finalized that record; recovery confirms absence.
    pub(crate) async fn recover_deletion(
        &self,
        id: &SessionId,
    ) -> super::session::deletion::SessionDeleteResult {
        let work = match self.catalog.lock().await.recover_delete(id) {
            Ok(work) => work,
            Err(result) => return result,
        };
        self.clean_deletion(work).await
    }
    pub(crate) async fn clean_deletion(
        &self,
        work: super::session::deletion::CleanupWork,
    ) -> super::session::deletion::SessionDeleteResult {
        let record = work.record.clone();
        #[cfg(test)]
        let gate = self.cleanup_gate.lock().unwrap().clone();
        #[cfg(test)]
        if let Some(gate) = gate {
            let _ = tokio::task::spawn_blocking(move || gate.enter()).await;
        }
        let result = work.settle().await;
        self.catalog.lock().await.finish_delete(&record, result)
    }
}
