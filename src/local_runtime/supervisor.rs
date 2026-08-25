//! Native local Session supervisor (Issue #88).
//!
//! [`LocalSessionSupervisor`] is the product owner above one active
//! `ConversationRuntime`. It serializes session control, chooses durable
//! historical boundaries, waits for the old runtime's semantic quiescence,
//! and only then publishes a new active selection. It does not duplicate
//! attempt, cancellation, recovery, or tool lifecycle state.
//!
//! v1 deliberately uses a typed process-boundary switch. A switch leaves the
//! old runtime quiescent and returns `restart_required`; the client then
//! reconnects, and ordinary composition/recovery opens the newly selected
//! `ConversationId`. A visibility commit followed by an uncertain directory
//! barrier returns a typed committed-transition result, including any
//! transient fork/tree editor payload. The catalog publication is authoritative
//! throughout.

use std::sync::Arc;

use crate::conversation::SurfaceRevision;
use crate::message::types::UserContentBlock;
use crate::model::session::SessionModelConfig;
use crate::runtime::conversation_runtime::{ConversationRuntime, ShutdownError};
use crate::runtime::identity::MessageId;
use crate::runtime_client::host::{RuntimeClientSessionControl, SessionControlFuture};
use crate::runtime_client::types::{
    RuntimeClientError, RuntimeClientResult, RuntimeClientSessionRequest, SessionNodeOriginView,
    SessionNodeView, SessionSummaryView, SessionUserMessageBoundaryView, SessionView,
};

use super::session::{
    HistoricalConversationSnapshot, SessionCatalog, SessionError, SessionId, SessionListPage,
    SessionNodeId, SessionSnapshot, SessionSummary, SessionUserMessageBoundary,
};

/// The result of a product transition that changes the active lineage.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSwitchResult {
    /// Newly published active Session metadata.
    pub session: SessionSnapshot,
    /// A forked prompt that belongs in the new editor, but is not canonical.
    pub editor_content: Option<Vec<UserContentBlock>>,
    /// v1 switches replace the one Runtime Client process attachment.
    pub restart_required: bool,
    /// A post-visibility durability failure crossed the catalog commit point.
    /// The transition is authoritative and this diagnostic is carried in the
    /// typed committed-transition result, never collapsed into a generic
    /// pre-commit failure.
    pub committed_restart_diagnostic: Option<String>,
}

/// The native `/tree` read projection.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTreeResult {
    /// Current Session graph metadata.
    pub session: SessionSnapshot,
    /// Bounded graph-node page.
    pub nodes: Vec<super::session::SessionNode>,
    /// Offset for the next graph-node page.
    pub next_node_offset: Option<usize>,
    /// Historical user-message boundaries available for a new node.
    pub branchable_messages: Vec<SessionUserMessageBoundary>,
    /// Offset for the next historical-boundary page.
    pub next_history_offset: Option<usize>,
}

/// The explicit runtime attachment state owned by the Session supervisor.
///
/// `NotInstalled` exists only during native composition, before the one
/// recovered runtime is handed to the supervisor. Once `Live` quiesces for a
/// replacement, the state is absorbing for this process attachment: it can
/// never silently become live again.
enum RuntimeAttachmentState {
    NotInstalled,
    Live(ConversationRuntime),
    ReplacementRequired { detail: String },
}

struct SupervisorState {
    catalog: SessionCatalog,
    /// Current runtime default used only when creating a new Session.
    default_model: SessionModelConfig,
    runtime: RuntimeAttachmentState,
}

/// The single local product owner of session metadata, graph state, active
/// selection, and one live `ConversationRuntime`.
#[derive(Clone)]
pub struct LocalSessionSupervisor {
    state: Arc<tokio::sync::Mutex<SupervisorState>>,
}

impl std::fmt::Debug for LocalSessionSupervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalSessionSupervisor")
            .finish_non_exhaustive()
    }
}

impl LocalSessionSupervisor {
    /// Creates a supervisor over a loaded native catalog. The active runtime
    /// is installed by the local product composition after ordinary recovery.
    #[must_use]
    pub fn new(catalog: SessionCatalog, default_model: SessionModelConfig) -> Self {
        Self {
            state: Arc::new(tokio::sync::Mutex::new(SupervisorState {
                catalog,
                default_model,
                runtime: RuntimeAttachmentState::NotInstalled,
            })),
        }
    }

    /// Arms a deterministic pre-visibility catalog fault for unit tests.
    #[cfg(test)]
    pub(crate) async fn arm_catalog_write_fault_before_rename(&self) {
        self.state
            .lock()
            .await
            .catalog
            .arm_write_fault_before_rename();
    }

    /// Arms a deterministic post-visibility durability fault for unit tests.
    #[cfg(test)]
    pub(crate) async fn arm_catalog_write_fault_after_rename(&self) {
        self.state
            .lock()
            .await
            .catalog
            .arm_write_fault_after_rename();
    }

    /// Installs the one active runtime after it has been composed and
    /// activated. Replacing a live runtime is rejected by construction.
    ///
    /// # Errors
    ///
    /// Returns [`SessionSupervisorError`] if a runtime is already installed
    /// or its conversation identity does not match the active node.
    pub async fn install_runtime(
        &self,
        runtime: ConversationRuntime,
    ) -> Result<(), SessionSupervisorError> {
        let mut state = self.state.lock().await;
        if !matches!(&state.runtime, RuntimeAttachmentState::NotInstalled) {
            return match &state.runtime {
                RuntimeAttachmentState::Live(_) => {
                    Err(SessionSupervisorError::RuntimeAlreadyInstalled)
                }
                RuntimeAttachmentState::ReplacementRequired { detail } => {
                    Err(SessionSupervisorError::RestartRequired {
                        detail: detail.clone(),
                    })
                }
                RuntimeAttachmentState::NotInstalled => unreachable!(),
            };
        }
        let (_, node, _) = state.catalog.active_lineage()?;
        if node.conversation_id != *runtime.conversation_id() {
            return Err(SessionSupervisorError::ConversationMismatch {
                selected: node.conversation_id,
                runtime: runtime.conversation_id().clone(),
            });
        }
        state.runtime = RuntimeAttachmentState::Live(runtime);
        Ok(())
    }

    /// Returns one bounded `/resume` metadata page.
    ///
    /// # Errors
    ///
    /// Returns [`SessionSupervisorError`] when the native page bound is
    /// invalid or the catalog cannot be read.
    pub async fn list(
        &self,
        query: Option<String>,
        offset: usize,
        limit: usize,
    ) -> Result<SessionListPage, SessionSupervisorError> {
        Ok(self
            .state
            .lock()
            .await
            .catalog
            .list_page(query.as_deref(), offset, limit)?)
    }

    /// Returns the authoritative `/session` metadata projection.
    ///
    /// # Errors
    ///
    /// Returns [`SessionSupervisorError`] when the catalog has no coherent
    /// active Session.
    pub async fn current(&self) -> Result<SessionSnapshot, SessionSupervisorError> {
        Ok(self.state.lock().await.catalog.active_snapshot()?)
    }

    /// Returns the current graph plus exact branchable user boundaries.
    ///
    /// # Errors
    ///
    /// Returns [`SessionSupervisorError`] when the active runtime or durable
    /// historical surface cannot be read.
    pub async fn tree(
        &self,
        node_offset: usize,
        history_offset: usize,
        limit: usize,
    ) -> Result<SessionTreeResult, SessionSupervisorError> {
        let state = self.state.lock().await;
        let session = state.catalog.active_snapshot()?;
        let source = current_head(&state)?;
        let node_page = state.catalog.node_page(&session.id, node_offset, limit)?;
        let history_page = branchable_messages(&source, &state.runtime, history_offset, limit)?;
        Ok(SessionTreeResult {
            session,
            nodes: node_page.nodes,
            next_node_offset: node_page.next_offset,
            branchable_messages: history_page.boundaries,
            next_history_offset: history_page.next_offset,
        })
    }

    /// Renames metadata only. No runtime or canonical conversation operation
    /// is involved.
    ///
    /// # Errors
    ///
    /// Returns [`SessionSupervisorError`] when the name or catalog update is
    /// invalid.
    pub async fn rename(&self, name: String) -> Result<SessionSnapshot, SessionSupervisorError> {
        let mut state = self.state.lock().await;
        ensure_live(&state.runtime)?;
        let session_id = state.catalog.active_snapshot()?.id;
        match state.catalog.rename(&session_id, &name) {
            Ok(snapshot) => Ok(snapshot),
            Err(error) if error.committed() => {
                let detail = error.to_string();
                mark_replacement_required(&mut state, detail.clone());
                Err(SessionSupervisorError::RestartRequired { detail })
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Creates a new empty Session and switches to it.
    ///
    /// # Errors
    ///
    /// Returns [`SessionSupervisorError`] when seed preparation, runtime
    /// quiescence, or catalog publication fails.
    pub async fn new_session(&self) -> Result<SessionSwitchResult, SessionSupervisorError> {
        let mut state = self.state.lock().await;
        ensure_live(&state.runtime)?;
        let template = super::session::SessionPersistentState {
            model: state.default_model.clone(),
        };
        let prepared = state.catalog.prepare_session(&template, &[])?;
        let origin = super::session::SessionNodeOrigin::New;
        state
            .catalog
            .preflight_publish_session(&prepared, origin.clone())?;
        self.quiesce_old(&mut state).await?;
        let session_id = prepared.session_id.clone();
        match state.catalog.publish_session(&prepared, origin) {
            Ok(snapshot) => {
                debug_assert_eq!(snapshot.id, session_id);
                Ok(SessionSwitchResult {
                    session: snapshot,
                    editor_content: None,
                    restart_required: true,
                    committed_restart_diagnostic: None,
                })
            }
            Err(error) if error.committed() => committed_switch(&state, &session_id, None, &error),
            Err(error) => Err(publication_failure(&error)),
        }
    }

    /// Switches to an existing persisted Session/node after validating its
    /// durable conversation first.
    ///
    /// # Errors
    ///
    /// Returns [`SessionSupervisorError`] when the selected lineage is
    /// unknown, invalid, or the old runtime cannot quiesce.
    pub async fn select(
        &self,
        session_id: SessionId,
        node_id: Option<SessionNodeId>,
    ) -> Result<SessionSwitchResult, SessionSupervisorError> {
        let mut state = self.state.lock().await;
        ensure_live(&state.runtime)?;
        let current = state.catalog.active_snapshot()?;
        let requested_node = match node_id.clone() {
            Some(node_id) => node_id,
            None => state.catalog.snapshot(&session_id)?.active_node,
        };
        if current.id == session_id && current.active_node == requested_node {
            return Ok(SessionSwitchResult {
                session: current,
                editor_content: None,
                restart_required: false,
                committed_restart_diagnostic: None,
            });
        }
        state
            .catalog
            .validate_storage(&session_id, Some(&requested_node))?;
        state
            .catalog
            .preflight_select(&session_id, Some(&requested_node))?;
        self.quiesce_old(&mut state).await?;
        match state.catalog.select(&session_id, Some(&requested_node)) {
            Ok(snapshot) => Ok(SessionSwitchResult {
                session: snapshot,
                editor_content: None,
                restart_required: true,
                committed_restart_diagnostic: None,
            }),
            Err(error) if error.committed() => committed_switch(&state, &session_id, None, &error),
            Err(error) => Err(publication_failure(&error)),
        }
    }

    /// Clones the exact committed current Surface head into a new Session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionSupervisorError`] when historical materialization,
    /// quiescence, or destination publication fails.
    pub async fn clone_active(&self) -> Result<SessionSwitchResult, SessionSupervisorError> {
        let mut state = self.state.lock().await;
        ensure_live(&state.runtime)?;
        let (source_session, source_node, template) = state.catalog.active_lineage()?;
        let source = current_head(&state)?;
        let prepared = state.catalog.prepare_clone_session(&template, &source)?;
        let origin = super::session::SessionNodeOrigin::Clone {
            source_session: source_session.clone(),
            source_node: source_node.id.clone(),
            source_surface_revision: source.surface_revision,
        };
        state
            .catalog
            .preflight_publish_session(&prepared, origin.clone())?;
        self.quiesce_old(&mut state).await?;
        let session_id = prepared.session_id.clone();
        match state.catalog.publish_session(&prepared, origin) {
            Ok(snapshot) => Ok(SessionSwitchResult {
                session: snapshot,
                editor_content: None,
                restart_required: true,
                committed_restart_diagnostic: None,
            }),
            Err(error) if error.committed() => committed_switch(&state, &session_id, None, &error),
            Err(error) => Err(publication_failure(&error)),
        }
    }

    /// Forks at an exact historical user-message boundary into a new Session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionSupervisorError`] when the revision/boundary is not
    /// durable, the old runtime cannot quiesce, or publication fails.
    pub async fn fork_active(
        &self,
        surface_revision: SurfaceRevision,
        message_id: MessageId,
    ) -> Result<SessionSwitchResult, SessionSupervisorError> {
        let mut state = self.state.lock().await;
        ensure_live(&state.runtime)?;
        let (source_session, source_node, template) = state.catalog.active_lineage()?;
        let source = historical_snapshot(&state, surface_revision)?;
        let (prepared, editor_content) =
            state
                .catalog
                .prepare_fork_session(&template, &source, &message_id)?;
        let origin = super::session::SessionNodeOrigin::Fork {
            source_session: source_session.clone(),
            source_node: source_node.id.clone(),
            source_surface_revision: surface_revision,
            source_user_message: message_id.clone(),
        };
        state
            .catalog
            .preflight_publish_session(&prepared, origin.clone())?;
        self.quiesce_old(&mut state).await?;
        let session_id = prepared.session_id.clone();
        match state.catalog.publish_session(&prepared, origin) {
            Ok(snapshot) => Ok(SessionSwitchResult {
                session: snapshot,
                editor_content: Some(editor_content),
                restart_required: true,
                committed_restart_diagnostic: None,
            }),
            Err(error) if error.committed() => {
                committed_switch(&state, &session_id, Some(editor_content), &error)
            }
            Err(error) => Err(publication_failure(&error)),
        }
    }

    /// Creates a new independent node under the active Session from an exact
    /// historical user-message boundary.
    ///
    /// # Errors
    ///
    /// Returns [`SessionSupervisorError`] when the revision/boundary is not
    /// durable, the old runtime cannot quiesce, or publication fails.
    pub async fn tree_branch(
        &self,
        surface_revision: SurfaceRevision,
        message_id: MessageId,
    ) -> Result<SessionSwitchResult, SessionSupervisorError> {
        let mut state = self.state.lock().await;
        ensure_live(&state.runtime)?;
        let (source_session, source_node, template) = state.catalog.active_lineage()?;
        let source = historical_snapshot(&state, surface_revision)?;
        let (prepared, editor_content) = state.catalog.prepare_tree_node_at_user_message(
            &source_session,
            &template,
            &source,
            &message_id,
        )?;
        let origin = super::session::SessionNodeOrigin::Fork {
            source_session: source_session.clone(),
            source_node: source_node.id.clone(),
            source_surface_revision: surface_revision,
            source_user_message: message_id.clone(),
        };
        state.catalog.preflight_publish_node(
            &source_session,
            &prepared,
            source_node.id.clone(),
            origin.clone(),
        )?;
        self.quiesce_old(&mut state).await?;
        match state
            .catalog
            .publish_node(&source_session, &prepared, source_node.id.clone(), origin)
        {
            Ok(snapshot) => Ok(SessionSwitchResult {
                session: snapshot,
                editor_content: Some(editor_content),
                restart_required: true,
                committed_restart_diagnostic: None,
            }),
            Err(error) if error.committed() => {
                committed_switch(&state, &source_session, Some(editor_content), &error)
            }
            Err(error) => Err(publication_failure(&error)),
        }
    }

    async fn quiesce_old(&self, state: &mut SupervisorState) -> Result<(), SessionSupervisorError> {
        let runtime = match &state.runtime {
            RuntimeAttachmentState::Live(runtime) => runtime.clone(),
            RuntimeAttachmentState::NotInstalled => {
                return Err(SessionSupervisorError::RestartRequired {
                    detail: "the local Session attachment has no installed runtime".to_owned(),
                });
            }
            RuntimeAttachmentState::ReplacementRequired { detail } => {
                return Err(SessionSupervisorError::RestartRequired {
                    detail: detail.clone(),
                });
            }
        };
        // `ConversationRuntime::shutdown` is the linearization point for
        // replacement: success means no attempt, foreground tool,
        // conversation background, or admission worker remains owned by the
        // old runtime. Catalog active selection is committed only after this
        // await returns.
        runtime
            .shutdown()
            .await
            .map_err(SessionSupervisorError::Shutdown)?;
        mark_replacement_required(
            state,
            "the old ConversationRuntime reached quiescence; this attachment must be replaced"
                .to_owned(),
        );
        Ok(())
    }
}

fn current_head(
    state: &SupervisorState,
) -> Result<HistoricalConversationSnapshot, SessionSupervisorError> {
    let runtime = live_runtime(&state.runtime)?;
    let (surface_revision, messages) = runtime
        .historical_head_snapshot()
        .map_err(SessionSupervisorError::Store)?;
    Ok(HistoricalConversationSnapshot {
        conversation_id: runtime.conversation_id().clone(),
        surface_revision,
        messages,
    })
}

fn historical_snapshot(
    state: &SupervisorState,
    surface_revision: SurfaceRevision,
) -> Result<HistoricalConversationSnapshot, SessionSupervisorError> {
    let runtime = live_runtime(&state.runtime)?;
    let messages = runtime
        .historical_surface_snapshot(surface_revision)
        .map_err(SessionSupervisorError::Store)?;
    Ok(HistoricalConversationSnapshot {
        conversation_id: runtime.conversation_id().clone(),
        surface_revision,
        messages,
    })
}

fn branchable_messages(
    head: &HistoricalConversationSnapshot,
    state: &RuntimeAttachmentState,
    offset: usize,
    limit: usize,
) -> Result<super::session::SessionUserMessageBoundaryPage, SessionSupervisorError> {
    let runtime = live_runtime(state)?;
    runtime
        .historical_user_message_boundaries_page(head.surface_revision, offset, limit)
        .map_err(SessionSupervisorError::Store)
        .map(|page| super::session::SessionUserMessageBoundaryPage {
            boundaries: page
                .boundaries
                .into_iter()
                .map(|boundary| SessionUserMessageBoundary {
                    surface_revision: boundary.surface_revision,
                    message: boundary.message,
                })
                .collect(),
            next_offset: page.next_offset,
        })
}

fn live_runtime(
    state: &RuntimeAttachmentState,
) -> Result<&ConversationRuntime, SessionSupervisorError> {
    match state {
        RuntimeAttachmentState::Live(runtime) => Ok(runtime),
        RuntimeAttachmentState::NotInstalled => Err(SessionSupervisorError::RestartRequired {
            detail: "the local Session attachment has no installed runtime".to_owned(),
        }),
        RuntimeAttachmentState::ReplacementRequired { detail } => {
            Err(SessionSupervisorError::RestartRequired {
                detail: detail.clone(),
            })
        }
    }
}

fn ensure_live(state: &RuntimeAttachmentState) -> Result<(), SessionSupervisorError> {
    live_runtime(state).map(|_| ())
}

fn mark_replacement_required(state: &mut SupervisorState, detail: String) {
    state.runtime = RuntimeAttachmentState::ReplacementRequired { detail };
}

/// A native Session-supervisor failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSupervisorError {
    /// A catalog/graph/materialization operation failed.
    Session(SessionError),
    /// The source/destination durable read failed.
    Store(crate::durable::ConversationStoreError),
    /// The old runtime did not reach quiescence.
    Shutdown(ShutdownError),
    /// The process attachment has no usable live runtime and must be
    /// replaced. This covers both a completed quiescent switch and a catalog
    /// mutation whose visibility commit crossed but whose durability barrier
    /// was uncertain.
    RestartRequired { detail: String },
    /// A second runtime was offered to one product instance.
    RuntimeAlreadyInstalled,
    /// The selected node and composed runtime disagree.
    ConversationMismatch {
        selected: crate::runtime::identity::ConversationId,
        runtime: crate::runtime::identity::ConversationId,
    },
}

impl From<SessionError> for SessionSupervisorError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl core::fmt::Display for SessionSupervisorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Session(error) => error.fmt(f),
            Self::Store(error) => write!(f, "session history: {error}"),
            Self::Shutdown(error) => write!(f, "runtime did not reach quiescence: {error:?}"),
            Self::RestartRequired { detail } => write!(
                f,
                "this Session attachment requires process replacement: {detail}"
            ),
            Self::RuntimeAlreadyInstalled => {
                f.write_str("the local product already owns an active ConversationRuntime")
            }
            Self::ConversationMismatch { selected, runtime } => write!(
                f,
                "selected node maps to conversation {selected}, but runtime is {runtime}"
            ),
        }
    }
}

impl std::error::Error for SessionSupervisorError {}

fn publication_failure(error: &SessionError) -> SessionSupervisorError {
    SessionSupervisorError::RestartRequired {
        detail: error.to_string(),
    }
}

fn committed_switch(
    state: &SupervisorState,
    session_id: &SessionId,
    editor_content: Option<Vec<UserContentBlock>>,
    error: &SessionError,
) -> Result<SessionSwitchResult, SessionSupervisorError> {
    debug_assert!(error.committed());
    let session = state.catalog.snapshot(session_id).map_err(|snapshot_error| {
        SessionSupervisorError::RestartRequired {
            detail: format!(
                "catalog visibility committed, but the committed Session snapshot could not be read: {snapshot_error}; original durability outcome: {error}"
            ),
        }
    })?;
    Ok(SessionSwitchResult {
        session,
        editor_content,
        restart_required: true,
        committed_restart_diagnostic: Some(error.to_string()),
    })
}

impl RuntimeClientSessionControl for LocalSessionSupervisor {
    #[allow(clippy::too_many_lines)]
    fn handle(&self, request: RuntimeClientSessionRequest) -> SessionControlFuture {
        let supervisor = self.clone();
        Box::pin(async move {
            let result = match request {
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
                    session: session_view(
                        supervisor
                            .current()
                            .await
                            .map_err(|error| session_error(&error))?,
                    ),
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
                        session: session_view(tree.session),
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
                    session: session_view(
                        supervisor
                            .rename(name)
                            .await
                            .map_err(|error| session_error(&error))?,
                    ),
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
                } => changed_view(
                    supervisor
                        .select(SessionId::new(session_id), node_id.map(SessionNodeId::new))
                        .await
                        .map_err(|error| session_error(&error))?,
                ),
                RuntimeClientSessionRequest::Clone => changed_view(
                    supervisor
                        .clone_active()
                        .await
                        .map_err(|error| session_error(&error))?,
                ),
                RuntimeClientSessionRequest::Fork {
                    surface_revision,
                    message_id,
                } => changed_view(
                    supervisor
                        .fork_active(surface_revision, message_id)
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
            Ok(result)
        })
    }

    fn persist_model(
        &self,
        config: crate::model::session::SessionModelConfig,
    ) -> Result<(), RuntimeClientError> {
        // Model updates arrive through the synchronous Runtime Client path.
        // Do not block a runtime worker or call `blocking_lock` inside an
        // async executor: a concurrent Session transition fails explicitly,
        // while the normal idle path acquires this product lock immediately.
        let mut state = self
            .state
            .try_lock()
            .map_err(|_| RuntimeClientError::SessionFailure {
                message: "a Session transition is already in progress".to_owned(),
            })?;
        ensure_live(&state.runtime).map_err(|error| session_error(&error))?;
        match state.catalog.persist_active_model(config) {
            Ok(()) => Ok(()),
            Err(error) if error.committed() => {
                let detail = error.to_string();
                mark_replacement_required(&mut state, detail.clone());
                Err(RuntimeClientError::SessionRestartRequired { message: detail })
            }
            Err(error) => Err(RuntimeClientError::SessionFailure {
                message: error.to_string(),
            }),
        }
    }

    fn ensure_live(&self) -> Result<(), RuntimeClientError> {
        let state = self
            .state
            .try_lock()
            .map_err(|_| RuntimeClientError::SessionFailure {
                message: "a Session transition is already in progress".to_owned(),
            })?;
        ensure_live(&state.runtime).map_err(|error| session_error(&error))
    }
}

fn changed_view(change: SessionSwitchResult) -> RuntimeClientResult {
    if let Some(diagnostic) = change.committed_restart_diagnostic {
        RuntimeClientResult::SessionCommittedRestartRequired {
            session: session_view(change.session),
            editor_content: change.editor_content,
            diagnostic,
        }
    } else {
        RuntimeClientResult::SessionChanged {
            session: session_view(change.session),
            editor_content: change.editor_content,
            restart_required: change.restart_required,
        }
    }
}

fn session_summary_view(summary: SessionSummary) -> SessionSummaryView {
    SessionSummaryView {
        id: summary.id.as_str().to_owned(),
        name: summary.name,
        preview: summary.preview,
        updated_at: summary.updated_at,
        active_node: summary.active_node.as_str().to_owned(),
        active: summary.active,
    }
}

fn session_view(snapshot: SessionSnapshot) -> SessionView {
    SessionView {
        id: snapshot.id.as_str().to_owned(),
        name: snapshot.name,
        created_at: snapshot.created_at,
        updated_at: snapshot.updated_at,
        active_node: snapshot.active_node.as_str().to_owned(),
        active_conversation_id: snapshot.active_conversation_id,
        node_count: snapshot.node_count,
    }
}

fn session_node_view(node: super::session::SessionNode) -> SessionNodeView {
    SessionNodeView {
        id: node.id.as_str().to_owned(),
        parent: node.parent.map(|parent| parent.as_str().to_owned()),
        conversation_id: node.conversation_id,
        origin: match node.origin {
            super::session::SessionNodeOrigin::New => SessionNodeOriginView::New,
            super::session::SessionNodeOrigin::Clone {
                source_session,
                source_node,
                source_surface_revision,
            } => SessionNodeOriginView::Clone {
                source_session: source_session.as_str().to_owned(),
                source_node: source_node.as_str().to_owned(),
                source_surface_revision,
            },
            super::session::SessionNodeOrigin::Fork {
                source_session,
                source_node,
                source_surface_revision,
                source_user_message,
            } => SessionNodeOriginView::Fork {
                source_session: source_session.as_str().to_owned(),
                source_node: source_node.as_str().to_owned(),
                source_surface_revision,
                source_user_message,
            },
        },
    }
}

fn session_error(error: &SessionSupervisorError) -> RuntimeClientError {
    match error {
        SessionSupervisorError::RestartRequired { detail } => {
            RuntimeClientError::SessionRestartRequired {
                message: detail.clone(),
            }
        }
        _ => RuntimeClientError::SessionFailure {
            message: error.to_string(),
        },
    }
}
