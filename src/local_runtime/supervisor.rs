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
//! `ConversationId`. The catalog publication is authoritative throughout.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::conversation::SurfaceRevision;
use crate::message::types::{InboundKind, MessageBlock, UserContentBlock};
use crate::runtime::conversation_runtime::{ConversationRuntime, ShutdownError};
use crate::runtime::identity::MessageId;
use crate::runtime_client::host::{RuntimeClientSessionControl, SessionControlFuture};
use crate::runtime_client::types::{
    RuntimeClientError, RuntimeClientResult, RuntimeClientSessionRequest, SessionNodeOriginView,
    SessionNodeView, SessionSummaryView, SessionUserMessageBoundaryView, SessionView,
};

use super::session::{
    HistoricalConversationSnapshot, SessionCatalog, SessionError, SessionId, SessionNodeId,
    SessionSnapshot, SessionSummary, SessionUserMessageBoundary,
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
}

/// The native `/tree` read projection.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTreeResult {
    /// Current Session graph metadata.
    pub session: SessionSnapshot,
    /// Historical user-message boundaries available for a new node.
    pub branchable_messages: Vec<SessionUserMessageBoundary>,
}

struct SupervisorState {
    catalog: SessionCatalog,
    active_runtime: Option<ConversationRuntime>,
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
    pub fn new(catalog: SessionCatalog) -> Self {
        Self {
            state: Arc::new(tokio::sync::Mutex::new(SupervisorState {
                catalog,
                active_runtime: None,
            })),
        }
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
        if state.active_runtime.is_some() {
            return Err(SessionSupervisorError::RuntimeAlreadyInstalled);
        }
        let (_, node, _) = state.catalog.active_lineage()?;
        if node.conversation_id != *runtime.conversation_id() {
            return Err(SessionSupervisorError::ConversationMismatch {
                selected: node.conversation_id,
                runtime: runtime.conversation_id().clone(),
            });
        }
        state.active_runtime = Some(runtime);
        Ok(())
    }

    /// Returns the bounded `/resume` metadata projection.
    pub async fn list(&self) -> Vec<SessionSummary> {
        self.state.lock().await.catalog.list()
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
    pub async fn tree(&self) -> Result<SessionTreeResult, SessionSupervisorError> {
        let state = self.state.lock().await;
        let session = state.catalog.active_snapshot()?;
        let source = current_head(&state)?;
        let branchable_messages = branchable_messages(&source, state.active_runtime.as_ref())?;
        Ok(SessionTreeResult {
            session,
            branchable_messages,
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
        let session_id = state.catalog.active_snapshot()?.id;
        Ok(state.catalog.rename(&session_id, &name)?)
    }

    /// Creates a new empty Session and switches to it.
    ///
    /// # Errors
    ///
    /// Returns [`SessionSupervisorError`] when seed preparation, runtime
    /// quiescence, or catalog publication fails.
    pub async fn new_session(&self) -> Result<SessionSwitchResult, SessionSupervisorError> {
        let mut state = self.state.lock().await;
        let (_, _, template) = state.catalog.active_lineage()?;
        let prepared = state.catalog.prepare_session(&template, &[])?;
        self.quiesce_old(&mut state).await?;
        let session_id = prepared.session_id.clone();
        let snapshot = state.catalog.publish_session(
            prepared,
            "New session",
            super::session::SessionNodeOrigin::New,
        )?;
        debug_assert_eq!(snapshot.id, session_id);
        Ok(SessionSwitchResult {
            session: snapshot,
            editor_content: None,
            restart_required: true,
        })
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
            });
        }
        state
            .catalog
            .validate_storage(&session_id, Some(&requested_node))?;
        self.quiesce_old(&mut state).await?;
        let snapshot = state.catalog.select(&session_id, Some(&requested_node))?;
        Ok(SessionSwitchResult {
            session: snapshot,
            editor_content: None,
            restart_required: true,
        })
    }

    /// Clones the exact committed current Surface head into a new Session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionSupervisorError`] when historical materialization,
    /// quiescence, or destination publication fails.
    pub async fn clone_active(&self) -> Result<SessionSwitchResult, SessionSupervisorError> {
        let mut state = self.state.lock().await;
        let (source_session, source_node, template) = state.catalog.active_lineage()?;
        let source = current_head(&state)?;
        let prepared = state.catalog.prepare_clone_session(&template, &source)?;
        self.quiesce_old(&mut state).await?;
        let snapshot = state.catalog.publish_session(
            prepared,
            &format!("Clone of {source_session}"),
            super::session::SessionNodeOrigin::Clone {
                source_session: source_session.clone(),
                source_node: source_node.id,
                source_surface_revision: source.surface_revision,
            },
        )?;
        Ok(SessionSwitchResult {
            session: snapshot,
            editor_content: None,
            restart_required: true,
        })
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
        let (source_session, source_node, template) = state.catalog.active_lineage()?;
        let source = historical_snapshot(&state, surface_revision)?;
        let (prepared, editor_content) =
            state
                .catalog
                .prepare_fork_session(&template, &source, &message_id)?;
        self.quiesce_old(&mut state).await?;
        let snapshot = state.catalog.publish_session(
            prepared,
            &format!("Fork of {source_session}"),
            super::session::SessionNodeOrigin::Fork {
                source_session: source_session.clone(),
                source_node: source_node.id,
                source_surface_revision: surface_revision,
                source_user_message: message_id,
            },
        )?;
        Ok(SessionSwitchResult {
            session: snapshot,
            editor_content: Some(editor_content),
            restart_required: true,
        })
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
        let (source_session, source_node, template) = state.catalog.active_lineage()?;
        let source = historical_snapshot(&state, surface_revision)?;
        let (prepared, editor_content) = state.catalog.prepare_tree_node_at_user_message(
            &source_session,
            &template,
            &source,
            &message_id,
        )?;
        self.quiesce_old(&mut state).await?;
        let snapshot = state.catalog.publish_node(
            &source_session,
            prepared,
            source_node.id.clone(),
            super::session::SessionNodeOrigin::Fork {
                source_session: source_session.clone(),
                source_node: source_node.id.clone(),
                source_surface_revision: surface_revision,
                source_user_message: message_id,
            },
        )?;
        Ok(SessionSwitchResult {
            session: snapshot,
            editor_content: Some(editor_content),
            restart_required: true,
        })
    }

    async fn quiesce_old(&self, state: &mut SupervisorState) -> Result<(), SessionSupervisorError> {
        let Some(runtime) = state.active_runtime.clone() else {
            return Err(SessionSupervisorError::NoActiveRuntime);
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
        state.active_runtime = None;
        Ok(())
    }
}

fn current_head(
    state: &SupervisorState,
) -> Result<HistoricalConversationSnapshot, SessionSupervisorError> {
    let Some(runtime) = state.active_runtime.as_ref() else {
        return Err(SessionSupervisorError::NoActiveRuntime);
    };
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
    let Some(runtime) = state.active_runtime.as_ref() else {
        return Err(SessionSupervisorError::NoActiveRuntime);
    };
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
    runtime: Option<&ConversationRuntime>,
) -> Result<Vec<SessionUserMessageBoundary>, SessionSupervisorError> {
    let Some(runtime) = runtime.as_ref() else {
        return Err(SessionSupervisorError::NoActiveRuntime);
    };
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for raw_revision in 0..=head.surface_revision.get() {
        let revision = SurfaceRevision::new(raw_revision);
        let messages = runtime
            .historical_surface_snapshot(revision)
            .map_err(SessionSupervisorError::Store)?;
        for message in messages {
            let MessageBlock::User(user) = message else {
                continue;
            };
            if user.kind != InboundKind::Message || !seen.insert(user.id.clone()) {
                continue;
            }
            result.push(SessionUserMessageBoundary {
                surface_revision: revision,
                message: user,
            });
        }
    }
    Ok(result)
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
    /// No live runtime remains in this process after a switch.
    NoActiveRuntime,
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
            Self::NoActiveRuntime => f.write_str("no active ConversationRuntime remains"),
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

impl RuntimeClientSessionControl for LocalSessionSupervisor {
    fn handle(&self, request: RuntimeClientSessionRequest) -> SessionControlFuture {
        let supervisor = self.clone();
        Box::pin(async move {
            let result = match request {
                RuntimeClientSessionRequest::List => RuntimeClientResult::SessionList {
                    sessions: supervisor
                        .list()
                        .await
                        .into_iter()
                        .map(session_summary_view)
                        .collect(),
                },
                RuntimeClientSessionRequest::Get => RuntimeClientResult::Session {
                    session: session_view(
                        supervisor
                            .current()
                            .await
                            .map_err(|error| session_error(&error))?,
                    ),
                },
                RuntimeClientSessionRequest::Tree => {
                    let tree = supervisor
                        .tree()
                        .await
                        .map_err(|error| session_error(&error))?;
                    RuntimeClientResult::SessionTree {
                        session: session_view(tree.session),
                        branchable_messages: tree
                            .branchable_messages
                            .into_iter()
                            .map(|boundary| SessionUserMessageBoundaryView {
                                surface_revision: boundary.surface_revision,
                                message: boundary.message,
                            })
                            .collect(),
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
        if state.active_runtime.is_none() {
            return Err(RuntimeClientError::SessionFailure {
                message: "no active ConversationRuntime owns this Session product".to_owned(),
            });
        }
        state
            .catalog
            .persist_active_model(config)
            .map_err(|error| session_error(&SessionSupervisorError::Session(error)))
    }
}

fn changed_view(change: SessionSwitchResult) -> RuntimeClientResult {
    RuntimeClientResult::SessionChanged {
        session: session_view(change.session),
        editor_content: change.editor_content,
        restart_required: change.restart_required,
    }
}

fn session_summary_view(summary: SessionSummary) -> SessionSummaryView {
    SessionSummaryView {
        id: summary.id.as_str().to_owned(),
        name: summary.name,
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
        nodes: snapshot
            .nodes
            .into_iter()
            .map(|node| SessionNodeView {
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
            })
            .collect(),
    }
}

fn session_error(error: &SessionSupervisorError) -> RuntimeClientError {
    RuntimeClientError::SessionFailure {
        message: error.to_string(),
    }
}
