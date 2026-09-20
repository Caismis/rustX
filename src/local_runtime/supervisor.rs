//! Narrow single-runtime local client attachment. Its identity is routing state,
//! never catalog authority. Durable operations belong to `SessionController`.
use super::session::{
    HistoricalConversationSnapshot, SessionCatalog, SessionError, SessionId, SessionListPage,
    SessionNodeId, SessionPersistentState, SessionSnapshot, SessionSummary,
    SessionUserMessageBoundary,
};
use super::session_controller::SessionController;
use crate::conversation::SurfaceRevision;
use crate::model::session::SessionModelConfig;
use crate::runtime::conversation_runtime::ConversationRuntime;
use crate::runtime::identity::MessageId;
use crate::runtime_client::host::{RuntimeClientSessionControl, SessionControlFuture};
use crate::runtime_client::types::{
    RuntimeClientError, RuntimeClientResult, RuntimeClientSessionRequest, SessionNodeOriginView,
    SessionNodeView, SessionSummaryView, SessionUserMessageBoundaryView, SessionView,
};
use std::sync::Arc;

pub use super::session_controller::SessionTransitionResult;
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTreeResult {
    pub session: SessionSnapshot,
    pub nodes: Vec<super::session::SessionNode>,
    pub next_node_offset: Option<usize>,
    pub branchable_messages: Vec<SessionUserMessageBoundary>,
    pub next_history_offset: Option<usize>,
}
/// Client routing projection. The durable Session snapshot keeps its graph default.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRoute {
    pub session: SessionSnapshot,
    pub node: super::session::SessionNode,
}
/// Call-site-local attachment for the existing Runtime Client protocol (#288).
/// Opening another Session returns its identity; it never shuts down this runtime.
#[derive(Clone, Debug)]
pub struct LocalSessionAttachment {
    controller: SessionController,
    session_id: SessionId,
    template: SessionPersistentState,
    settings_revision: Arc<std::sync::atomic::AtomicU64>,
    model_publication_uncertain: Arc<std::sync::atomic::AtomicBool>,
    runtime: Arc<std::sync::OnceLock<ConversationRuntime>>,
}
impl LocalSessionAttachment {
    pub(crate) fn new(
        catalog: SessionCatalog,
        session_id: SessionId,
        template: SessionPersistentState,
        revision: u64,
    ) -> Self {
        Self {
            controller: SessionController::new(catalog),
            session_id,
            template,
            settings_revision: Arc::new(std::sync::atomic::AtomicU64::new(revision)),
            model_publication_uncertain: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            runtime: Arc::new(std::sync::OnceLock::new()),
        }
    }
    /// # Errors
    /// Identity, storage and attachment failures are returned explicitly.
    pub async fn inspect_deletion(
        &self,
        id: &SessionId,
    ) -> std::io::Result<super::session_deletion::DeletionTargetSnapshot> {
        self.controller.catalog.lock().await.inspect_deletion(id)
    }
    pub(crate) async fn delete_preview(
        &self,
        id: &SessionId,
    ) -> super::session::deletion::SessionDeleteResult {
        self.controller.delete_preview(id).await
    }
    pub(crate) async fn commit_startup(
        &self,
        planned: super::session::PlannedCatalog,
    ) -> Result<(), SessionError> {
        self.controller.catalog.lock().await.commit_planned(planned)
    }
    #[cfg(test)]
    pub(crate) fn install_copy_publication_gate(
        &self,
        gate: Arc<crate::runtime::conversation_runtime::Gate>,
    ) {
        *self.controller.copy_publication_gate.lock().unwrap() = Some(gate);
    }
    #[cfg(test)]
    pub(crate) async fn arm_catalog_write_fault_before_rename(&self) {
        self.controller
            .catalog
            .lock()
            .await
            .arm_write_fault_before_rename();
    }
    #[cfg(test)]
    pub(crate) async fn arm_catalog_write_fault_after_rename(&self) {
        self.controller
            .catalog
            .lock()
            .await
            .arm_write_fault_after_rename();
    }
    /// # Errors
    /// Identity, storage and attachment failures are returned explicitly.
    pub async fn install_runtime(
        &self,
        runtime: ConversationRuntime,
    ) -> Result<(), SessionAttachmentError> {
        self.controller
            .catalog
            .lock()
            .await
            .conversation_lineage(&self.session_id, runtime.conversation_id())?;
        self.runtime.set(runtime).map_err(|_| {
            SessionAttachmentError::Session(SessionError::Catalog {
                detail: "attachment already installed".into(),
            })
        })
    }
    /// # Errors
    /// Identity, storage and attachment failures are returned explicitly.
    pub async fn list(
        &self,
        query: Option<String>,
        offset: usize,
        limit: usize,
    ) -> Result<SessionListPage, SessionAttachmentError> {
        Ok(self
            .controller
            .list_sessions(query.as_deref(), offset, limit)
            .await?)
    }
    /// # Errors
    /// Identity, storage and attachment failures are returned explicitly.
    pub async fn current(&self) -> Result<SessionSnapshot, SessionAttachmentError> {
        Ok(self.controller.read_session(&self.session_id).await?)
    }
    /// # Errors
    /// Identity, storage and attachment failures are returned explicitly.
    pub async fn rename(&self, name: String) -> Result<SessionSnapshot, SessionAttachmentError> {
        Ok(self
            .controller
            .rename_session(&self.session_id, &name)
            .await?)
    }
    /// # Errors
    /// Identity, storage and attachment failures are returned explicitly.
    pub async fn new_session(&self) -> Result<SessionTransitionResult, SessionAttachmentError> {
        Ok(self
            .controller
            .create_session(self.template.clone())
            .await?)
    }
    /// # Errors
    /// Identity, storage and attachment failures are returned explicitly.
    pub async fn select(
        &self,
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
    ) -> Result<SessionRoute, SessionAttachmentError> {
        let catalog = self.controller.catalog.lock().await;
        let (node, _) = catalog.lineage(&session_id, node_id.as_ref())?;
        Ok(SessionRoute {
            session: catalog.snapshot(&session_id)?,
            node,
        })
    }
    // The existing wire spells a route as active_node. Keep that translation
    // at the client adapter, never by rewriting a durable SessionSnapshot.
    async fn attachment_view(
        &self,
        session: SessionSnapshot,
    ) -> Result<SessionView, SessionAttachmentError> {
        let runtime = self.runtime.get().ok_or_else(|| SessionError::Catalog {
            detail: "client attachment is not installed".into(),
        })?;
        let (node, _) = self
            .controller
            .catalog
            .lock()
            .await
            .conversation_lineage(&self.session_id, runtime.conversation_id())?;
        Ok(route_view(SessionRoute { session, node }))
    }
    fn requires_reattach(&self, target: &SessionView) -> bool {
        self.session_id != target.id
            || self
                .runtime
                .get()
                .is_none_or(|runtime| runtime.conversation_id() != &target.active_conversation_id)
    }

    fn source(
        &self,
        revision: Option<SurfaceRevision>,
    ) -> Result<HistoricalConversationSnapshot, SessionAttachmentError> {
        let runtime = self.runtime.get().ok_or_else(|| {
            SessionAttachmentError::Session(SessionError::Catalog {
                detail: "client attachment is not installed".into(),
            })
        })?;
        let (surface_revision, messages) = match revision {
            Some(revision) => (
                revision,
                runtime
                    .historical_surface_snapshot(revision)
                    .map_err(SessionAttachmentError::Store)?,
            ),
            None => runtime
                .historical_head_snapshot()
                .map_err(SessionAttachmentError::Store)?,
        };
        let canonical = runtime
            .historical_canonical_history()
            .map_err(SessionAttachmentError::Store)?;
        Ok(HistoricalConversationSnapshot {
            completed_responses: crate::runtime_client::response::lineage_provenance(
                runtime.tool_runtime().durable_store().as_ref(),
                &canonical,
            )
            .map_err(SessionAttachmentError::Store)?,
            conversation_id: runtime.conversation_id().clone(),
            surface_revision,
            messages,
            canonical,
            surface_history: runtime
                .historical_surface_history(surface_revision)
                .map_err(SessionAttachmentError::Store)?,
        })
    }
    /// # Errors
    /// Identity, storage and attachment failures are returned explicitly.
    pub async fn tree(
        &self,
        node_offset: usize,
        history_offset: usize,
        limit: usize,
    ) -> Result<SessionTreeResult, SessionAttachmentError> {
        let source = self.source(None)?;
        let runtime = self.runtime.get().ok_or_else(|| {
            SessionAttachmentError::Session(SessionError::Catalog {
                detail: "attachment not installed".into(),
            })
        })?;
        let history = runtime
            .historical_user_message_boundaries_page(source.surface_revision, history_offset, limit)
            .map_err(SessionAttachmentError::Store)?;
        let catalog = self.controller.catalog.lock().await;
        let nodes = catalog.node_page(&self.session_id, node_offset, limit)?;
        Ok(SessionTreeResult {
            session: catalog.snapshot(&self.session_id)?,
            nodes: nodes.nodes,
            next_node_offset: nodes.next_offset,
            branchable_messages: history
                .boundaries
                .into_iter()
                .map(|b| SessionUserMessageBoundary {
                    surface_revision: b.surface_revision,
                    message: b.message,
                })
                .collect(),
            next_history_offset: history.next_offset,
        })
    }
    /// # Errors
    /// Identity, storage and attachment failures are returned explicitly.
    pub async fn clone_attached(&self) -> Result<SessionTransitionResult, SessionAttachmentError> {
        self.branch(None, None, false).await
    }
    /// # Errors
    /// Identity, storage and attachment failures are returned explicitly.
    pub async fn fork_attached(
        &self,
        revision: SurfaceRevision,
        message: MessageId,
    ) -> Result<SessionTransitionResult, SessionAttachmentError> {
        self.branch(Some(revision), Some(message), false).await
    }
    /// # Errors
    /// Identity, storage and attachment failures are returned explicitly.
    pub async fn tree_branch(
        &self,
        revision: SurfaceRevision,
        message: MessageId,
    ) -> Result<SessionTransitionResult, SessionAttachmentError> {
        self.branch(Some(revision), Some(message), true).await
    }
    async fn branch(
        &self,
        revision: Option<SurfaceRevision>,
        message: Option<MessageId>,
        tree: bool,
    ) -> Result<SessionTransitionResult, SessionAttachmentError> {
        let _preparation = self.controller.preparation.lock().await;
        let source = self.source(revision)?;
        let (node, settings) = self
            .controller
            .catalog
            .lock()
            .await
            .conversation_lineage(&self.session_id, &source.conversation_id)?;
        Ok(self
            .controller
            .copy_admitted_lineage(
                &self.session_id,
                &node,
                &settings,
                &source,
                message.as_ref(),
                tree,
                super::session::LineageSide::Before,
            )
            .await?)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAttachmentError {
    Session(SessionError),
    Store(crate::durable::ConversationStoreError),
}
impl From<SessionError> for SessionAttachmentError {
    fn from(e: SessionError) -> Self {
        Self::Session(e)
    }
}
impl std::fmt::Display for SessionAttachmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Session(e) => e.fmt(f),
            Self::Store(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for SessionAttachmentError {}

impl RuntimeClientSessionControl for LocalSessionAttachment {
    #[allow(clippy::too_many_lines)]
    fn handle(&self, request: RuntimeClientSessionRequest) -> SessionControlFuture {
        let supervisor = self.clone();
        Box::pin(async move {
            let mut result = match request {
                RuntimeClientSessionRequest::DeletePreview { session_id } => {
                    RuntimeClientResult::SessionDeletion {
                        result: project_session_deletion(
                            supervisor.delete_preview(&session_id).await,
                        ),
                    }
                }
                RuntimeClientSessionRequest::List {
                    query,
                    offset,
                    limit,
                } => {
                    let page = supervisor
                        .list(query, offset, limit)
                        .await
                        .map_err(|error| session_error(&error))?;
                    RuntimeClientResult::SessionList {
                        sessions: page
                            .sessions
                            .into_iter()
                            .map(session_summary_view)
                            .collect(),
                        next_offset: page.next_offset,
                    }
                }
                RuntimeClientSessionRequest::Get => RuntimeClientResult::Session {
                    session: supervisor
                        .attachment_view(
                            supervisor
                                .current()
                                .await
                                .map_err(|error| session_error(&error))?,
                        )
                        .await
                        .map_err(|error| session_error(&error))?,
                },
                RuntimeClientSessionRequest::Tree {
                    node_offset,
                    history_offset,
                    limit,
                } => {
                    let tree = supervisor
                        .tree(node_offset, history_offset, limit)
                        .await
                        .map_err(|error| session_error(&error))?;
                    RuntimeClientResult::SessionTree {
                        session: supervisor
                            .attachment_view(tree.session)
                            .await
                            .map_err(|error| session_error(&error))?,
                        nodes: tree.nodes.into_iter().map(session_node_view).collect(),
                        next_node_offset: tree.next_node_offset,
                        branchable_messages: tree
                            .branchable_messages
                            .into_iter()
                            .map(|boundary| SessionUserMessageBoundaryView {
                                surface_revision: boundary.surface_revision,
                                message: boundary.message,
                            })
                            .collect(),
                        next_history_offset: tree.next_history_offset,
                    }
                }
                RuntimeClientSessionRequest::Name(name) => RuntimeClientResult::SessionChanged {
                    session: supervisor
                        .attachment_view(
                            supervisor
                                .rename(name)
                                .await
                                .map_err(|error| session_error(&error))?,
                        )
                        .await
                        .map_err(|error| session_error(&error))?,
                    editor_content: None,
                    restart_required: false,
                },
                RuntimeClientSessionRequest::New => changed_view(
                    supervisor
                        .new_session()
                        .await
                        .map_err(|error| session_error(&error))?,
                ),
                RuntimeClientSessionRequest::Select {
                    session_id,
                    node_id,
                } => {
                    let route = supervisor
                        .select(session_id, node_id)
                        .await
                        .map_err(|error| session_error(&error))?;
                    RuntimeClientResult::SessionChanged {
                        session: route_view(route),
                        editor_content: None,
                        restart_required: false,
                    }
                }
                RuntimeClientSessionRequest::Clone => changed_view(
                    supervisor
                        .clone_attached()
                        .await
                        .map_err(|error| session_error(&error))?,
                ),
                RuntimeClientSessionRequest::Fork {
                    surface_revision,
                    message_id,
                } => changed_view(
                    supervisor
                        .fork_attached(surface_revision, message_id)
                        .await
                        .map_err(|error| session_error(&error))?,
                ),
                RuntimeClientSessionRequest::TreeBranch {
                    surface_revision,
                    message_id,
                } => changed_view(
                    supervisor
                        .tree_branch(surface_revision, message_id)
                        .await
                        .map_err(|error| session_error(&error))?,
                ),
            };
            // Durable publication does not rebind this single-runtime attachment.
            // Compare every returned route, including metadata projections, here.
            if let RuntimeClientResult::SessionChanged {
                session,
                restart_required,
                ..
            } = &mut result
            {
                *restart_required = supervisor.requires_reattach(session);
            }
            Ok(result)
        })
    }

    fn persist_model(&self, config: SessionModelConfig) -> Result<(), RuntimeClientError> {
        use std::sync::atomic::Ordering;
        let mut catalog =
            self.controller
                .catalog
                .try_lock()
                .map_err(|_| RuntimeClientError::SessionFailure {
                    message: "Session metadata is busy".into(),
                })?;
        let expected = self.settings_revision.load(Ordering::Acquire);
        let (_, mut settings) = catalog.lineage(&self.session_id, None).map_err(|e| {
            RuntimeClientError::SessionFailure {
                message: e.to_string(),
            }
        })?;
        settings.model = Some(config.clone());
        match catalog.replace_settings(&self.session_id, expected, settings) {
            Ok(revision) => {
                self.settings_revision.store(revision, Ordering::Release);
                if let Some(binding) = self
                    .controller
                    .configuration_bindings
                    .lock()
                    .expect("Session configuration bindings")
                    .get_mut(&self.session_id)
                {
                    *binding = binding.with_model(config);
                }
                Ok(())
            }
            Err(error) if error.committed() => {
                self.model_publication_uncertain
                    .store(true, Ordering::Release);
                Err(RuntimeClientError::SessionRestartRequired {
                    message: error.to_string(),
                })
            }
            Err(error) => Err(RuntimeClientError::SessionFailure {
                message: error.to_string(),
            }),
        }
    }
    fn ensure_live(&self) -> Result<(), RuntimeClientError> {
        if self
            .model_publication_uncertain
            .load(std::sync::atomic::Ordering::Acquire)
        {
            Err(RuntimeClientError::SessionRestartRequired {
                message: "this attachment's model publication has uncertain durability".into(),
            })
        } else {
            Ok(())
        }
    }
}

fn changed_view(change: SessionTransitionResult) -> RuntimeClientResult {
    if let Some(diagnostic) = change.durability_diagnostic {
        RuntimeClientResult::SessionCommittedRestartRequired {
            session: session_view(change.session),
            editor_content: change.editor_content,
            diagnostic,
        }
    } else {
        RuntimeClientResult::SessionChanged {
            session: session_view(change.session),
            editor_content: change.editor_content,
            restart_required: false,
        }
    }
}

fn session_summary_view(summary: SessionSummary) -> SessionSummaryView {
    SessionSummaryView {
        id: summary.id,
        name: summary.name,
        preview: summary.preview,
        updated_at: summary.updated_at,
        active_node: summary.active_node,
    }
}

fn route_view(route: SessionRoute) -> SessionView {
    let mut view = session_view(route.session);
    view.active_node = route.node.id;
    view.active_conversation_id = route.node.conversation_id;
    view
}

fn session_view(snapshot: SessionSnapshot) -> SessionView {
    SessionView {
        id: snapshot.id,
        name: snapshot.name,
        created_at: snapshot.created_at,
        updated_at: snapshot.updated_at,
        active_node: snapshot.active_node,
        active_conversation_id: snapshot.active_conversation_id,
        node_count: snapshot.node_count,
    }
}

fn session_node_view(node: super::session::SessionNode) -> SessionNodeView {
    SessionNodeView {
        id: node.id,
        parent: node.parent,
        conversation_id: node.conversation_id,
        origin: match node.origin {
            super::session::SessionNodeOrigin::New => SessionNodeOriginView::New,
            super::session::SessionNodeOrigin::Clone {
                source_session,
                source_node,
                source_surface_revision,
            } => SessionNodeOriginView::Clone {
                source_session,
                source_node,
                source_surface_revision,
            },
            super::session::SessionNodeOrigin::Fork {
                source_session,
                source_node,
                source_surface_revision,
                source_message,
                side,
            } => SessionNodeOriginView::Fork {
                source_session,
                source_node,
                source_surface_revision,
                source_message,
                side,
            },
        },
    }
}

fn session_error(error: &SessionAttachmentError) -> RuntimeClientError {
    RuntimeClientError::SessionFailure {
        message: error.to_string(),
    }
}
#[cfg(test)]
pub(crate) async fn assert_deletion_cleanup_releases_catalog(
    catalog: SessionCatalog,
    mut work: super::session::deletion::CleanupWork,
    _model: SessionModelConfig,
) {
    let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let release = gate.arm_scoped();
    work.cleanup_gate = Some(gate.clone());
    let controller = super::session_controller::SessionController::new(catalog);
    let worker = controller.clone();
    let task = tokio::spawn(async move { worker.clean_deletion(work).await });
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
    assert!(!task.is_finished());
    assert!(controller.catalog.try_lock().is_ok());
    drop(release);
    assert!(matches!(
        task.await.unwrap(),
        super::session::deletion::SessionDeleteResult::Deleted { .. }
    ));
}

/// The Session control owner projects internal outcomes explicitly. Neither
/// recovery capabilities nor storage error strings cross the protocol boundary.
pub(crate) fn project_session_deletion(
    result: super::session::deletion::SessionDeleteResult,
) -> crate::runtime_client::session_deletion::RuntimeClientSessionDeletionResult {
    use super::session::deletion::{
        DeletionBlocker as Blocker, DeletionScope, SessionDeleteResult as Native,
    };
    use crate::runtime_client::session_deletion::{
        RuntimeClientSessionDeletePreview, RuntimeClientSessionDeletionBlocker as Reason,
        RuntimeClientSessionDeletionResult as Wire,
    };
    match result {
        Native::Preview { preview } => {
            let nodes = preview
                .scopes
                .iter()
                .filter(|s| matches!(s, DeletionScope::Node { .. }))
                .count() as u64;
            let total = preview.scopes.len() as u64;
            Wire::Preview {
                preview: RuntimeClientSessionDeletePreview {
                    session_id: preview.session_id,
                    name: preview.name.map(|name| name.chars().take(256).collect()),
                    target_revision: preview.target_revision,
                    owned_node_count: nodes,
                    owned_conversation_count: total,
                    owned_child_count: total - nodes,
                },
            }
        }
        Native::Deleted { session_id } => Wire::Deleted { session_id },
        Native::NotFound { session_id } => Wire::NotFound { session_id },
        // A stale execution invalidates confirmation, never mints its replacement.
        Native::Stale { session_id, .. } => Wire::Stale { session_id },
        Native::Blocked { session_id, reason } => Wire::Blocked {
            session_id,
            reason: match reason {
                Blocker::ResourceConflict => Reason::ResourceConflict,
                Blocker::Workspace { resources } => Reason::Workspace {
                    resource_count: resources.len() as u64,
                },
                Blocker::InvalidOwnership { .. } => Reason::InvalidOwnership,
            },
        },
        Native::CommittedCleanupPending { record, .. } => Wire::CommittedCleanupPending {
            session_id: record.session_id,
        },
        Native::CommittedDurabilityUncertain { session_id, .. } => {
            Wire::CommittedDurabilityUncertain { session_id }
        }
    }
}
