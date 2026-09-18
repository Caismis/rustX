//! User-root durable Session controller. No runtime, resolver or client focus is owned here.
use super::session::{
    SessionCatalog, SessionError, SessionId, SessionListPage, SessionNode, SessionNodeId,
    SessionPersistentState, SessionSnapshot, SessionSummary,
};
use crate::durable::ConversationStore;
use crate::message::types::{MessageBlock, UserContentBlock};
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
    // Allocation of the one process runtime owner, not ownership of its registry.
    pub(crate) runtime_owner: Arc<std::sync::OnceLock<()>>,
    pub(crate) preparation: Arc<tokio::sync::Mutex<()>>,
    #[cfg(test)]
    create_gate: Arc<std::sync::Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>>,
    #[cfg(test)]
    cleanup_gate: Arc<std::sync::Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>>,
    #[cfg(test)]
    copy_gate: Arc<std::sync::Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>>,
}
impl SessionController {
    /// Commit a transport-independent batch to the addressed Session workspace.
    /// # Errors
    /// Invalid names, stale Session authority and durability failures are explicit.
    /// # Panics
    /// Panics only if internal allocation invariants or test gates are corrupted.
    pub async fn upload(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        files: Vec<super::session::uploads::UploadFile>,
    ) -> Result<Vec<super::session::uploads::UploadedFile>, SessionError> {
        let _preparation = self.preparation.lock().await;
        let access = self.acquire_session(id, node).await?;
        let fail = |e: std::io::Error| SessionError::Catalog {
            detail: e.to_string(),
        };
        let workspace = access.settings.cwd.canonicalize().map_err(fail)?;
        if workspace.to_str().is_none() {
            return Err(SessionError::Catalog {
                detail: "upload workspace must be UTF-8".into(),
            });
        }
        let mut registry = self.catalog.lock().await.upload_registry(id)?;
        let batch = registry.claim(workspace, &files).map_err(fail)?;
        // Durable ownership first. Even failed materialization is deletion work.
        self.catalog
            .lock()
            .await
            .commit_uploads(id, registry.clone())?;
        registry.materialize(id, &batch, &files).map_err(fail)?;
        #[cfg(test)]
        {
            let gate = self.upload_commit_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                tokio::task::spawn_blocking(move || gate.enter())
                    .await
                    .unwrap();
            }
        }
        registry.verify_materialized(id, &batch).map_err(fail)?;
        registry
            .allocations
            .get_mut(&batch)
            .expect("claimed batch")
            .ready = true;
        // Semantic commit point: synced complete files plus durable ready registry.
        self.catalog
            .lock()
            .await
            .commit_uploads(id, registry.clone())?;
        registry.receipts(id, &batch).map_err(fail)
    }

    /// Validate server receipts and author typed canonical content.
    /// # Errors
    /// Unknown, incomplete and cross-Session receipts are rejected.
    pub async fn uploaded_content(
        &self,
        id: &SessionId,
        receipts: &[super::session::uploads::UploadReceipt],
    ) -> Result<Vec<crate::message::types::UserContentBlock>, SessionError> {
        let registry = self.catalog.lock().await.upload_registry(id)?;
        receipts
            .iter()
            .map(|receipt| {
                registry
                    .receipt_ref(id, receipt)
                    .map(crate::message::types::UserContentBlock::UploadedFile)
                    .map_err(|e| SessionError::Catalog {
                        detail: e.to_string(),
                    })
            })
            .collect()
    }

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
        Ok(Self::new(catalog))
    }
    pub(crate) fn new(catalog: SessionCatalog) -> Self {
        Self {
            #[cfg(test)]
            upload_commit_gate: Arc::default(),
            #[cfg(test)]
            copy_upload_gate: Arc::default(),
            #[cfg(test)]
            copy_publication_gate: Arc::default(),
            catalog: Arc::new(tokio::sync::Mutex::new(catalog)),
            runtime_owner: Arc::default(),
            preparation: Arc::new(tokio::sync::Mutex::new(())),
            #[cfg(test)]
            create_gate: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            cleanup_gate: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            copy_gate: Arc::new(std::sync::Mutex::new(None)),
        }
    }
    #[cfg(test)]
    pub(crate) fn install_delete_cleanup_gate(
        &self,
        gate: Arc<crate::runtime::conversation_runtime::Gate>,
    ) {
        *self.cleanup_gate.lock().unwrap() = Some(gate);
    }
    /// # Errors
    /// Unknown identities are rejected without configuration resolution.
    pub async fn read_session(&self, id: &SessionId) -> Result<SessionSnapshot, SessionError> {
        self.catalog.lock().await.snapshot(id)
    }
    /// Exact durable display metadata; does not resolve configuration or compose a runtime.
    /// # Errors
    /// Unknown/deleting identities and conversation storage failures are returned.
    pub async fn read_session_summary(
        &self,
        id: &SessionId,
    ) -> Result<SessionSummary, SessionError> {
        let snapshot = self.catalog.lock().await.clone();
        snapshot.summary(id)
    }
    /// # Errors
    /// Invalid pagination or storage errors are returned.
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
        if !settings.cwd.is_absolute() {
            return Err(SessionError::Catalog {
                detail: "Session cwd must be absolute".into(),
            });
        }
        // Blocking preparation outlives a cancelled async caller. Transfer the
        // allocator guard into that work and retain it through publication.
        let preparation = self.preparation.clone().lock_owned().await;
        let snapshot = self.catalog.lock().await.clone();
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
        let prepared = prepared?;
        let mut catalog = self.catalog.lock().await;
        let (session, durability_diagnostic) =
            match catalog.publish_session(&prepared, super::session::SessionNodeOrigin::New) {
                Ok(session) => (session, None),
                Err(error) if error.committed() => (
                    catalog.snapshot(&prepared.session_id)?,
                    Some(error.to_string()),
                ),
                Err(error) => return Err(error),
            };
        Ok(SessionTransitionResult {
            session,
            editor_content: None,
            durability_diagnostic,
        })
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
    /// Resolve durable identity without taking allocation authority. The runtime
    /// manager must register its flight before acquiring the selected allocation.
    pub(crate) async fn resolve_session_target(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
    ) -> Result<SessionNode, SessionError> {
        self.catalog
            .lock()
            .await
            .lineage(id, node)
            .map(|(node, _)| node)
    }
    /// Explicit cold storage recovery for one Session's graph and owned children.
    /// Never performed by opening, listing or reading the catalog.
    /// # Errors
    /// Missing identity or allocation/deletion conflicts fail closed.
    pub async fn recover_session_storage(&self, id: &SessionId) -> Result<(), SessionError> {
        let snapshot = self.catalog.lock().await.clone();
        let controller = snapshot.controller()?;
        let id = id.clone();
        tokio::task::spawn_blocking(move || snapshot.recover_session_storage(&id, &controller))
            .await
            .map_err(|error| SessionError::Catalog {
                detail: error.to_string(),
            })?
    }
    /// Read a bounded graph page without loading a runtime.
    /// # Errors
    /// Unknown Session or invalid page bounds fail explicitly.
    pub async fn tree(
        &self,
        id: &SessionId,
        offset: usize,
        limit: usize,
    ) -> Result<super::session::SessionNodePage, SessionError> {
        self.catalog.lock().await.node_page(id, offset, limit)
    }
    /// Select the current node inside an explicitly addressed Session graph.
    /// # Errors
    /// Unknown Session/node and catalog visibility failures remain explicit.
    pub async fn set_current_node(
        &self,
        id: &SessionId,
        node: &SessionNodeId,
    ) -> Result<SessionSnapshot, SessionError> {
        self.catalog.lock().await.set_current_node(id, Some(node))
    }
    /// Clone an exact committed revision into an independent Session.
    /// # Errors
    /// Identity, allocation and storage failures are returned without fallback.
    pub async fn clone_session(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        revision: crate::conversation::SurfaceRevision,
    ) -> Result<SessionTransitionResult, SessionError> {
        self.copy_lineage(id, node, revision, None, false).await
    }
    /// Branch an explicitly addressed Session graph at a user-message boundary.
    /// # Errors
    /// Unknown identities/boundaries and allocation conflicts fail closed.
    pub async fn branch_session_node(
        &self,
        id: &SessionId,
        node: &SessionNodeId,
        revision: crate::conversation::SurfaceRevision,
        boundary: &crate::runtime::identity::MessageId,
    ) -> Result<SessionTransitionResult, SessionError> {
        self.copy_lineage(id, Some(node), revision, Some(boundary), true)
            .await
    }
    /// Copy an exact immutable Surface boundary while retaining source allocation
    /// access. Source deletion cannot commit before destination publication.
    /// Concurrent source appends cannot alter the supplied revision's lineage cut.
    /// # Errors
    /// Unknown identities/boundaries, deletion conflicts and storage failures fail closed.
    pub async fn fork_session(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        revision: crate::conversation::SurfaceRevision,
        boundary: Option<&crate::runtime::identity::MessageId>,
    ) -> Result<SessionTransitionResult, SessionError> {
        self.copy_lineage(id, node, revision, boundary, false).await
    }
    #[allow(clippy::too_many_lines)] // One prepare/copy/publication transaction.
    pub(crate) async fn copy_lineage(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        revision: crate::conversation::SurfaceRevision,
        boundary: Option<&crate::runtime::identity::MessageId>,
        tree: bool,
    ) -> Result<SessionTransitionResult, SessionError> {
        let _preparation = self.preparation.lock().await;
        let access = self.acquire_session(id, node).await?;
        let snapshot = self.catalog.lock().await.clone();
        let path = snapshot.database_path(id, &access.node.conversation_id);
        let store = crate::durable::SqliteConversationStore::open_existing(
            access.node.conversation_id.clone(),
            &path,
        )
        .map_err(SessionError::Store)?;
        let source = super::session::HistoricalConversationSnapshot {
            conversation_id: access.node.conversation_id.clone(),
            surface_revision: revision,
            messages: store
                .load_surface_snapshot(revision)
                .map_err(SessionError::Store)?,
            canonical: store.load_canonical().map_err(SessionError::Store)?,
            surface_history: store
                .load_surface_history(revision)
                .map_err(SessionError::Store)?,
        };
        self.copy_admitted_lineage(id, &access.node, &access.settings, &source, boundary, tree)
            .await
    }

    /// Both durable and already-attached callers retain their allocation access
    /// and the preparation mutex before entering this one lifecycle owner.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn copy_admitted_lineage(
        &self,
        id: &SessionId,
        node: &SessionNode,
        settings: &SessionPersistentState,
        source: &super::session::HistoricalConversationSnapshot,
        boundary: Option<&crate::runtime::identity::MessageId>,
        tree: bool,
    ) -> Result<SessionTransitionResult, SessionError> {
        let snapshot = self.catalog.lock().await.clone();
        let revision = source.surface_revision;
        #[cfg(test)]
        {
            let gate = self.copy_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                tokio::task::spawn_blocking(move || gate.enter())
                    .await
                    .unwrap();
            }
        }
        let (mut prepared, editor_content, origin) = if let Some(message) = boundary {
            let (prepared, editor) = if tree {
                snapshot.prepare_tree_node_at_user_message(id, settings, source, message)?
            } else {
                snapshot.prepare_fork_session(settings, source, message)?
            };
            (
                prepared,
                Some(editor),
                super::session::SessionNodeOrigin::Fork {
                    source_session: id.clone(),
                    source_node: node.id.clone(),
                    source_surface_revision: revision,
                    source_user_message: message.clone(),
                },
            )
        } else {
            (
                snapshot.prepare_clone_session(settings, source)?,
                None,
                super::session::SessionNodeOrigin::Clone {
                    source_session: id.clone(),
                    source_node: node.id.clone(),
                    source_surface_revision: revision,
                },
            )
        };
        if !tree {
            let claim = self
                .catalog
                .lock()
                .await
                .claim_upload_preparation(&prepared.session_id, &[]);
            if let Err(error) = claim {
                // A visible claim is retained for recovery; a failed claim owns
                // no uploads yet and the inert native seed can be discarded.
                if !error.committed() {
                    snapshot.discard_prepared_session(&prepared)?;
                }
                return Err(error);
            }
        }
        let editor_result = async {
            let registry =
                if tree {
                    snapshot.upload_registry(id)?
                } else {
                    let destination = crate::durable::SqliteConversationStore::open_existing(
                        prepared.conversation_id.clone(),
                        &prepared.database_path,
                    )
                    .map_err(SessionError::Store)?;
                    let messages = destination.load_canonical().map_err(SessionError::Store)?;
                    let references = messages
                        .iter()
                        .filter_map(|m| match m {
                            MessageBlock::User(u) => Some(u.content.as_slice()),
                            _ => None,
                        })
                        .chain(editor_content.as_deref())
                        .flatten()
                        .filter_map(|b| match b {
                            UserContentBlock::UploadedFile(f) => Some(f.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    if !references.is_empty() {
                        let workspace = prepared.state.cwd.canonicalize().map_err(|e| {
                            SessionError::Catalog {
                                detail: e.to_string(),
                            }
                        })?;
                        self.catalog
                            .lock()
                            .await
                            .claim_upload_workspace(&prepared.session_id, &workspace)?;
                        prepared.uploads = snapshot
                            .upload_registry(id)?
                            .copy_required(id, &prepared.session_id, &workspace, &references)
                            .map_err(|e| SessionError::Catalog {
                                detail: e.to_string(),
                            })?;
                        #[cfg(test)]
                        {
                            let gate = self.copy_upload_gate.lock().unwrap().clone();
                            if let Some(gate) = gate {
                                tokio::task::spawn_blocking(move || gate.enter())
                                    .await
                                    .unwrap();
                            }
                        }
                        prepared
                            .uploads
                            .verify_all_materialized(&prepared.session_id)
                            .map_err(|e| SessionError::Catalog {
                                detail: e.to_string(),
                            })?;
                    }
                    prepared.uploads.clone()
                };
            editor_content
                .as_ref()
                .map(|content| {
                    registry
                        .editor_input(&prepared.session_id, content)
                        .map_err(|e| SessionError::Catalog {
                            detail: e.to_string(),
                        })
                })
                .transpose()
        }
        .await;
        let editor_content = match editor_result {
            Ok(editor) => editor,
            Err(error) => return Err(self.discard_failed_copy(&prepared, tree, error).await),
        };
        #[cfg(test)]
        {
            let gate = self.copy_publication_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                tokio::task::spawn_blocking(move || gate.enter())
                    .await
                    .unwrap();
            }
        }
        let mut catalog = self.catalog.lock().await;
        let result = if tree {
            catalog.publish_node(id, &prepared, node.id.clone(), origin)
        } else {
            catalog.publish_session(&prepared, origin)
        };
        let (session, diagnostic) = match result {
            Ok(session) => (session, None),
            Err(error) if error.committed() => (
                catalog.snapshot(if tree { id } else { &prepared.session_id })?,
                Some(error.to_string()),
            ),
            Err(error) => {
                drop(catalog);
                return Err(self.discard_failed_copy(&prepared, tree, error).await);
            }
        };
        Ok(SessionTransitionResult {
            session,
            editor_content,
            durability_diagnostic: diagnostic,
        })
    }
    /// One lifecycle owner consumes a claim only after the entire frozen private
    /// workset is gone. Node cleanup never acquires Session deletion authority.
    async fn discard_failed_copy(
        &self,
        prepared: &super::session::PreparedLineage,
        tree: bool,
        operation: SessionError,
    ) -> SessionError {
        let snapshot = self.catalog.lock().await.clone();
        let cleanup = if tree {
            snapshot.discard_prepared_node(prepared)
        } else {
            match snapshot.cleanup_upload_preparation(&prepared.session_id) {
                Ok(()) => self
                    .catalog
                    .lock()
                    .await
                    .finish_upload_preparation(&prepared.session_id),
                Err(error) => Err(error),
            }
        };
        match cleanup {
            Ok(()) => operation,
            Err(cleanup) => SessionError::PreparationCleanupPending {
                operation: Box::new(operation),
                cleanup: Box::new(cleanup),
            },
        }
    }
    /// Read explicit selections and their CAS revision without resolving them.
    /// # Errors
    /// Unknown Session identities are rejected.
    pub async fn read_settings(
        &self,
        id: &SessionId,
    ) -> Result<(u64, SessionPersistentState), SessionError> {
        let catalog = self.catalog.lock().await;
        Ok((catalog.settings_revision(id)?, catalog.lineage(id, None)?.1))
    }
    /// # Errors
    /// Concurrent settings edits fail with `StaleSettings`; visibility errors retain
    /// their pre/post commit distinction.
    pub async fn replace_settings(
        &self,
        id: &SessionId,
        expected: u64,
        settings: SessionPersistentState,
    ) -> Result<u64, SessionError> {
        self.catalog
            .lock()
            .await
            .replace_settings(id, expected, settings)
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
    /// Retry only the existing frozen cleanup record, outside the metadata mutex.
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
        let result = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            if let Some(gate) = gate {
                gate.enter();
            }
            work.run()
        })
        .await
        .unwrap_or_else(|error| Err(std::io::Error::other(error)));
        self.catalog.lock().await.finish_delete(&record, result)
    }
}

#[cfg(test)]
mod tests {
    use super::super::configuration::SessionConfigInput;
    use super::super::session::deletion::SessionDeleteResult;
    use super::*;
    fn settings(path: &std::path::Path) -> SessionPersistentState {
        SessionPersistentState::from_input(&SessionConfigInput::new(path.to_path_buf()))
    }
    #[tokio::test]
    async fn committed_delete_rejects_load_while_cleanup_releases_catalog_mutex() {
        let root = tempfile::tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let a = controller
            .create_session(settings(root.path()))
            .await
            .unwrap()
            .session;
        let b = controller
            .create_session(settings(root.path()))
            .await
            .unwrap()
            .session;
        let SessionDeleteResult::Preview { preview } = controller.delete_preview(&a.id).await
        else {
            panic!("unloaded Session is deletable")
        };
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        *controller.cleanup_gate.lock().unwrap() = Some(gate.clone());
        let worker = controller.clone();
        let id = a.id.clone();
        let delete =
            tokio::spawn(async move { worker.delete_session(&id, &preview.target_revision).await });
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        assert!(controller.catalog.try_lock().is_ok());
        assert!(matches!(
            controller.acquire_session(&a.id, None).await,
            Err(SessionError::DeletingSession { .. })
        ));
        let access_b = controller.acquire_session(&b.id, None).await.unwrap();
        controller
            .rename_session(&b.id, "during cleanup")
            .await
            .unwrap();
        assert_eq!(
            controller
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .len(),
            1
        );
        assert!(!delete.is_finished());
        drop(release);
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert_eq!(access_b.session.id, b.id);
    }
    #[tokio::test]
    async fn cancelled_create_retains_allocation_exclusion_until_preparation_finishes() {
        let root = tempfile::tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        *controller.create_gate.lock().unwrap() = Some(gate.clone());
        let worker = controller.clone();
        let input = settings(root.path());
        let create = tokio::spawn(async move { worker.create_session(input).await });
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        let reserved: Vec<_> = std::fs::read_dir(root.path().join("sessions"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        create.abort();
        assert!(create.await.unwrap_err().is_cancelled());
        assert!(controller.preparation.try_lock().is_err());
        assert!(
            controller
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .is_empty()
        );
        *controller.create_gate.lock().unwrap() = None;
        drop(release);
        let created = controller
            .create_session(settings(root.path()))
            .await
            .unwrap()
            .session;
        // The cancelled preparation's private storage cannot be reused or published.
        assert!(!reserved.iter().any(|name| name == created.id.as_str()));
        assert_eq!(
            controller
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .len(),
            1
        );
    }
    #[tokio::test]
    async fn independent_sessions_reopen_without_focus_or_runtime_effects() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("rustx.toml"), "malformed = [").unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        assert!(
            controller
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .is_empty()
        );
        let a = controller
            .create_session(settings(root.path()))
            .await
            .unwrap()
            .session;
        let b = controller
            .create_session(settings(root.path()))
            .await
            .unwrap()
            .session;
        controller.rename_session(&a.id, "A renamed").await.unwrap();
        assert_eq!(controller.read_session(&b.id).await.unwrap(), b);
        let (reads, effects) = super::super::static_effects::measure(|| {
            let catalog = controller.catalog.try_lock().unwrap();
            (
                catalog.snapshot(&a.id).unwrap(),
                catalog.list_page(None, 0, 32).unwrap(),
            )
        });
        assert_eq!(effects, [0; 12]);
        assert_eq!(reads.1.sessions.len(), 2);
        let bytes = std::fs::read(root.path().join("sessions/catalog.json")).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            json["schema_version"],
            crate::local_runtime::session::SESSION_CATALOG_SCHEMA_VERSION
        );
        assert!(json.get("active_session").is_none());
        assert!(
            serde_json::to_value(&reads.1.sessions)
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row.get("active").is_none())
        );
        drop(controller);
        let controller = SessionController::open(root.path()).unwrap();
        assert_eq!(controller.read_session(&b.id).await.unwrap(), b);
        assert_eq!(controller.read_session(&a.id).await.unwrap(), reads.0);
        assert_eq!(
            std::fs::read(root.path().join("sessions/catalog.json")).unwrap(),
            bytes
        );
    }
    #[tokio::test]
    async fn bounded_cwd_projection_reads_the_only_durable_owner_without_runtime() {
        let root = tempfile::tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let original = settings(root.path());
        let session = controller
            .create_session(original.clone())
            .await
            .unwrap()
            .session;
        let page = controller.list_sessions(None, 0, 1).await.unwrap();
        assert_eq!(page.sessions[0].cwd, original.cwd);
        assert_eq!(page.next_offset, None);
        controller
            .rename_session(&session.id, "named cold session")
            .await
            .unwrap();
        assert_eq!(
            controller.read_settings(&session.id).await.unwrap(),
            (0, original.clone())
        );
        drop(controller);
        let controller = SessionController::open(root.path()).unwrap();
        let page = controller
            .list_sessions(Some("named cold"), 0, 1)
            .await
            .unwrap();
        assert_eq!(page.sessions[0].cwd, original.cwd);
        let mut changed = original;
        changed.cwd = root.path().join("another");
        controller
            .replace_settings(&session.id, 0, changed.clone())
            .await
            .unwrap();
        assert_eq!(
            controller.list_sessions(None, 0, 1).await.unwrap().sessions[0].cwd,
            changed.cwd
        );
    }

    #[tokio::test]
    async fn explicit_session_model_and_omitted_default_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let omitted = settings(root.path());
        let mut explicit = omitted.clone();
        explicit.model = Some(crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("chosen").unwrap(),
        ));
        let a = controller
            .create_session(omitted.clone())
            .await
            .unwrap()
            .session;
        let b = controller
            .create_session(explicit.clone())
            .await
            .unwrap()
            .session;
        drop(controller);
        let controller = SessionController::open(root.path()).unwrap();
        assert_eq!(controller.read_settings(&a.id).await.unwrap(), (0, omitted));
        assert_eq!(
            controller.read_settings(&b.id).await.unwrap(),
            (0, explicit)
        );
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.path().join("sessions/catalog.json")).unwrap(),
        )
        .unwrap();
        let state = &json["sessions"][a.id.as_str()]["state"];
        for key in ["model", "config", "tools", "exclude_tools"] {
            assert!(state.get(key).is_none(), "{key}");
        }
    }
    #[tokio::test]
    async fn retained_allocations_and_committed_delete_have_one_winner() {
        let root = tempfile::tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let a = controller
            .create_session(settings(root.path()))
            .await
            .unwrap()
            .session;
        let b = controller
            .create_session(settings(root.path()))
            .await
            .unwrap()
            .session;
        let access_a = controller.acquire_session(&a.id, None).await.unwrap();
        let access_b = controller.acquire_session(&b.id, None).await.unwrap();
        assert!(matches!(
            controller.delete_preview(&a.id).await,
            SessionDeleteResult::Preview { .. }
        ));
        drop(access_a);
        let SessionDeleteResult::Preview { preview } = controller.delete_preview(&a.id).await
        else {
            panic!("released allocation")
        };
        assert!(matches!(
            controller
                .delete_session(&a.id, &preview.target_revision)
                .await
                .unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert!(matches!(
            controller.acquire_session(&a.id, None).await,
            Err(SessionError::UnknownSession { .. })
        ));
        assert_eq!(
            controller.read_session(&b.id).await.unwrap(),
            access_b.session
        );
        drop(access_b);
        let SessionDeleteResult::Preview { preview } = controller.delete_preview(&b.id).await
        else {
            panic!("released B")
        };
        controller
            .delete_session(&b.id, &preview.target_revision)
            .await
            .unwrap();
        assert!(
            controller
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .is_empty()
        );
    }
    #[tokio::test]
    async fn settings_cas_has_one_winner_without_touching_other_metadata() {
        let root = tempfile::tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let initial = settings(root.path());
        let a = controller
            .create_session(initial.clone())
            .await
            .unwrap()
            .session;
        let b = controller
            .create_session(initial.clone())
            .await
            .unwrap()
            .session;
        let mut candidate = initial.clone();
        candidate.model = Some(crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("chosen").unwrap(),
        ));
        let (one, two) = tokio::join!(
            controller.replace_settings(&a.id, 0, candidate.clone()),
            controller.replace_settings(&a.id, 0, initial)
        );
        assert_eq!(one.unwrap(), 1);
        assert!(matches!(
            two,
            Err(SessionError::StaleSettings {
                expected: 0,
                actual: 1
            })
        ));
        assert_eq!(
            controller.read_settings(&a.id).await.unwrap(),
            (1, candidate)
        );
        assert_eq!(controller.read_session(&b.id).await.unwrap(), b);
    }
    #[tokio::test]
    async fn create_and_rename_visibility_follow_catalog_rename_not_durability_barrier() {
        let root = tempfile::tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let a = controller
            .create_session(settings(root.path()))
            .await
            .unwrap()
            .session;
        controller
            .catalog
            .lock()
            .await
            .arm_write_fault_before_rename();
        assert!(
            controller
                .create_session(settings(root.path()))
                .await
                .is_err()
        );
        assert_eq!(
            controller
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .len(),
            1
        );
        controller
            .catalog
            .lock()
            .await
            .arm_write_fault_before_rename();
        assert!(
            !controller
                .rename_session(&a.id, "new")
                .await
                .unwrap_err()
                .committed()
        );
        assert_eq!(controller.read_session(&a.id).await.unwrap(), a);
        controller
            .catalog
            .lock()
            .await
            .arm_write_fault_after_rename();
        assert!(
            controller
                .rename_session(&a.id, "new")
                .await
                .unwrap_err()
                .committed()
        );
        let reopened = SessionCatalog::open_existing(root.path()).unwrap().unwrap();
        assert_eq!(
            reopened.snapshot(&a.id).unwrap(),
            controller.read_session(&a.id).await.unwrap()
        );
        assert_eq!(
            reopened.snapshot(&a.id).unwrap().name.as_deref(),
            Some("new")
        );
    }
    fn user(id: &str) -> crate::message::types::MessageBlock {
        crate::message::types::MessageBlock::User(crate::message::types::UserMessageBlock {
            id: crate::runtime::identity::MessageId::new(id),
            content: vec![crate::message::types::UserContentBlock::Text(
                crate::message::content::TextBlock { text: id.into() },
            )],
            source: crate::message::types::UserSource::Human,
            kind: crate::message::types::InboundKind::Message,
            timestamp: None,
        })
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::too_many_lines)]
    async fn exact_fork_boundary_excludes_delete_and_releases_metadata_lock() {
        let root = tempfile::tempdir().unwrap();
        let controller = SessionController::open(root.path()).unwrap();
        let a = controller
            .create_session(settings(root.path()))
            .await
            .unwrap()
            .session;
        let b = controller
            .create_session(settings(root.path()))
            .await
            .unwrap()
            .session;
        let path = controller
            .catalog
            .lock()
            .await
            .database_path(&a.id, &a.active_conversation_id);
        let store =
            crate::durable::SqliteConversationStore::open(a.active_conversation_id.clone(), &path)
                .unwrap();
        store.append_canonical(&user("before")).unwrap();
        store.append_canonical(&user("boundary")).unwrap();
        let revision = store.load_head().unwrap().revision;
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        *controller.copy_gate.lock().unwrap() = Some(gate.clone());
        let worker = controller.clone();
        let id = a.id.clone();
        let fork = tokio::spawn(async move {
            worker
                .fork_session(
                    &id,
                    None,
                    revision,
                    Some(&crate::runtime::identity::MessageId::new("boundary")),
                )
                .await
        });
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        assert!(!fork.is_finished());
        assert!(matches!(
            controller.delete_preview(&a.id).await,
            SessionDeleteResult::Preview { .. }
        ));
        controller
            .rename_session(&b.id, "B during copy")
            .await
            .unwrap();
        store.append_canonical(&user("later")).unwrap();
        drop(release);
        let copied = fork.await.unwrap().unwrap();
        assert_eq!(
            copied.editor_content.unwrap(),
            vec![super::super::session::uploads::UserInputBlock::Text(
                crate::message::content::TextBlock {
                    text: "boundary".into()
                }
            )]
        );
        let path = controller
            .catalog
            .lock()
            .await
            .database_path(&copied.session.id, &copied.session.active_conversation_id);
        let destination = crate::durable::SqliteConversationStore::open_existing(
            copied.session.active_conversation_id.clone(),
            &path,
        )
        .unwrap();
        let history = destination.load_canonical().unwrap();
        assert_eq!(history.len(), 1);
        let crate::message::types::MessageBlock::User(message) = &history[0] else {
            panic!("copied user")
        };
        assert_ne!(message.id.as_str(), "before");
        assert_eq!(
            message.content,
            match user("before") {
                crate::message::types::MessageBlock::User(message) => message.content,
                _ => unreachable!(),
            }
        );
        assert_eq!(store.load_canonical().unwrap().len(), 3);
        drop(store);
        let SessionDeleteResult::Preview { preview } = controller.delete_preview(&a.id).await
        else {
            panic!("fork released source allocation")
        };
        controller
            .delete_session(&a.id, &preview.target_revision)
            .await
            .unwrap();
        assert!(matches!(
            controller.fork_session(&a.id, None, revision, None).await,
            Err(SessionError::UnknownSession { .. })
        ));
        assert_eq!(
            controller.read_session(&copied.session.id).await.unwrap(),
            copied.session
        );
    }
}
