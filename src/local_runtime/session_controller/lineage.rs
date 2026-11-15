//! Coherent Session graph/node queries, boundary resolution, clone/fork/branch, and lineage copy.

use super::{
    SessionController, SessionError, SessionId, SessionNode, SessionNodeId, SessionPersistentState,
    SessionSnapshot, SessionTransitionResult,
};
use crate::durable::{CanonicalStore, ObservationStore, ProjectionStore};
use crate::message::types::{MessageBlock, UserContentBlock};

impl SessionController {
    /// Read the exact immutable node under the catalog lock. The existing native
    /// node map owns identity and provenance; no runtime or store is opened.
    /// # Errors
    /// Unknown/deleting Session and unknown Node fail explicitly.
    pub async fn read_node(
        &self,
        id: &SessionId,
        node: &SessionNodeId,
    ) -> Result<SessionNode, SessionError> {
        self.catalog
            .lock()
            .await
            .lineage(id, Some(node))
            .map(|(node, _)| node)
    }
    /// Read a bounded graph page without loading a runtime.
    /// # Errors
    /// Unknown Session or invalid page bounds fail explicitly.
    pub async fn tree(
        &self,
        id: &SessionId,
        offset: usize,
        limit: usize,
    ) -> Result<super::super::session::SessionNodePage, SessionError> {
        self.catalog.lock().await.node_page(id, offset, limit)
    }
    /// Read finite immutable catalog metadata without opening a Runtime or Store.
    /// # Errors
    /// Unknown or deleting Sessions and unknown Nodes are rejected.
    pub async fn observe_node(
        &self,
        id: &SessionId,
        node: &SessionNodeId,
    ) -> Result<super::super::session::observation::SessionNodeObservation, SessionError> {
        self.catalog.lock().await.observe_node(id, node)
    }
    /// Read a finite native tree page.
    /// # Errors
    /// Unknown Sessions and invalid page limits are rejected.
    pub async fn observe_tree(
        &self,
        id: &SessionId,
        offset: usize,
        limit: usize,
    ) -> Result<
        (
            Vec<super::super::session::observation::SessionNodeObservation>,
            Option<usize>,
        ),
        SessionError,
    > {
        self.catalog.lock().await.observe_tree(id, offset, limit)
    }
    /// Slice at most 64 KiB from a retained immutable catalog value.
    /// # Errors
    /// Unknown identities, changed immutable evidence and invalid ranges are rejected.
    pub async fn node_value_read(
        &self,
        reference: &super::super::session::observation::SessionNodeValueRef,
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::super::session::observation::SessionNodeValueChunk, SessionError> {
        self.catalog
            .lock()
            .await
            .node_value_read(reference, offset, max_bytes)
    }
    /// Resolve a finite historical boundary at the existing native Surface owner.
    /// # Errors
    /// Unknown allocations and mismatched immutable boundary evidence are rejected.
    pub(crate) async fn resolve_boundary(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        revision: crate::conversation::SurfaceRevision,
        locator: &crate::observation::content::CanonicalContentLocator,
        side: super::super::session::LineageSide,
    ) -> Result<crate::runtime::identity::MessageId, SessionError> {
        let access = self.acquire_session(id, node).await?;
        let locator = locator.clone();
        tokio::task::spawn_blocking(move || {
            let store = crate::durable::SqliteConversationStore::open_existing(
                access.node.conversation_id.clone(),
                &access.database_path,
            )
            .map_err(SessionError::Store)?
            .with_lifecycle(access.allocation);
            match side {
                super::super::session::LineageSide::Before => {
                    store.surface_boundary_message_id(revision, &locator)
                }
                super::super::session::LineageSide::After => {
                    store.canonical_boundary_message_id(revision, &locator)
                }
            }
            .map_err(SessionError::Store)
        })
        .await
        .map_err(|error| SessionError::Catalog {
            detail: error.to_string(),
        })?
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
    /// Clone structure through Surface revision R into an independent Session.
    /// `read_lineage_cut(R)` captures invocation-time native cut C for temporal
    /// evidence. Repeating R at a newer C may inherit a newer historical outcome;
    /// later source activity cannot alter the captured (R, C).
    /// # Errors
    /// Identity, allocation and storage failures are returned without fallback.
    pub async fn clone_session(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        revision: crate::conversation::SurfaceRevision,
    ) -> Result<SessionTransitionResult, SessionError> {
        self.copy_lineage(
            id,
            node,
            revision,
            None,
            false,
            super::super::session::LineageSide::Before,
        )
        .await
    }
    /// Branch an explicitly addressed Session graph at a user-message boundary.
    /// The boundary narrows structure through R; temporal facts remain bounded
    /// by invocation-time C captured by the native lineage read.
    /// # Errors
    /// Unknown identities/boundaries and allocation conflicts fail closed.
    pub async fn branch_session_node(
        &self,
        id: &SessionId,
        node: &SessionNodeId,
        revision: crate::conversation::SurfaceRevision,
        boundary: &crate::runtime::identity::MessageId,
    ) -> Result<SessionTransitionResult, SessionError> {
        self.copy_lineage(
            id,
            Some(node),
            revision,
            Some(boundary),
            true,
            super::super::session::LineageSide::Before,
        )
        .await
    }
    /// Copy structure through R and its boundary while retaining source allocation
    /// access. Source deletion cannot commit before destination publication.
    /// Native `read_lineage_cut(R)` captures temporal cut C; concurrent source
    /// activity cannot alter that frozen (R, C). A later invocation at the same
    /// R may observe newer temporal facts through its newer C.
    /// # Errors
    /// Unknown identities/boundaries, deletion conflicts and storage failures fail closed.
    pub async fn fork_session(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        revision: crate::conversation::SurfaceRevision,
        boundary: Option<&crate::runtime::identity::MessageId>,
    ) -> Result<SessionTransitionResult, SessionError> {
        self.copy_lineage(
            id,
            node,
            revision,
            boundary,
            false,
            super::super::session::LineageSide::Before,
        )
        .await
    }
    #[allow(clippy::too_many_lines)] // One prepare/copy/publication transaction.
    /// Fork through a historical activation even when another activation has
    /// the same canonical frontier. Reuses the one native lineage lifecycle.
    /// # Errors
    /// Rejects unavailable ownership or missing/ineligible historical dependencies.
    pub async fn fork_projection(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        ledger: crate::context::resources::LedgerFrontier,
        activation: crate::context::checkpoint::ProjectionActivationId,
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
        let source = store
            .read_projection_lineage_cut(ledger, activation)
            .map_err(SessionError::Store)?;
        self.copy_admitted_lineage(
            id,
            &access.node,
            &access.settings,
            &source,
            None,
            false,
            super::super::session::LineageSide::Before,
        )
        .await
    }

    pub(crate) async fn copy_lineage(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        revision: crate::conversation::SurfaceRevision,
        boundary: Option<&crate::runtime::identity::MessageId>,
        tree: bool,
        side: super::super::session::LineageSide,
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
        if side == super::super::session::LineageSide::After {
            let Some(message) = boundary else {
                return Err(SessionError::Seed {
                    detail: "A post-response cut requires a completed response".into(),
                });
            };
            if store
                .message_append_revision(message)
                .map_err(SessionError::Store)?
                != Some(revision)
            {
                return Err(SessionError::Seed { detail: "Stale or mismatched response boundary revision; choose an exact historical cut".into() });
            }
        }
        let source = match boundary {
            Some(message) => store.read_message_lineage_cut(
                revision,
                message,
                side == super::super::session::LineageSide::After,
            ),
            None => store.read_lineage_cut(revision),
        }
        .map_err(SessionError::Store)?;
        if side == super::super::session::LineageSide::After
            && !source
                .completed_responses
                .iter()
                .any(|response| Some(&response.closing_message_id) == boundary)
        {
            return Err(SessionError::UnknownBoundary {
                message_id: boundary.expect("After cut checked above").clone(),
            });
        }
        self.copy_admitted_lineage(
            id,
            &access.node,
            &access.settings,
            &source,
            boundary,
            tree,
            side,
        )
        .await
    }

    /// Both durable and already-attached callers retain their allocation access
    /// and the preparation mutex before entering this one lifecycle owner.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One admitted lineage publication transaction.
    pub(crate) async fn copy_admitted_lineage(
        &self,
        id: &SessionId,
        node: &SessionNode,
        settings: &SessionPersistentState,
        source: &super::super::session::LineageReadCut,
        boundary: Option<&crate::runtime::identity::MessageId>,
        tree: bool,
        side: super::super::session::LineageSide,
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
            // Resolve the immutable Ledger position once at the existing lineage
            // mutation boundary. Display reads never bind a giant original ID.
            let store = crate::durable::SqliteConversationStore::open_existing(
                node.conversation_id.clone(),
                &snapshot.database_path(id, &node.conversation_id),
            )
            .map_err(SessionError::Store)?;
            let position = store
                .message_ledger_position(message)
                .map_err(SessionError::Store)?
                .filter(|position| *position <= source.read_cut.ledger_through)
                .ok_or_else(|| SessionError::Seed {
                    detail: "Exact canonical lineage boundary is unavailable".into(),
                })?;
            let source_locator =
                crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position };
            let (prepared, editor) = if tree {
                snapshot.prepare_tree_node(id, settings, source, message, side)?
            } else {
                snapshot.prepare_fork_session(settings, source, message, side)?
            };
            (
                prepared,
                Some(editor),
                super::super::session::SessionNodeOrigin::Fork {
                    source_session: id.clone(),
                    source_node: node.id.clone(),
                    source_surface_revision: revision,
                    source_message: message.clone(),
                    source_locator,
                    side,
                },
            )
        } else {
            (
                snapshot.prepare_clone_session(settings, source)?,
                None,
                super::super::session::SessionNodeOrigin::Clone {
                    source_session: id.clone(),
                    source_node: node.id.clone(),
                    source_surface_revision: revision,
                },
            )
        };
        prepared.dynamic_tools.clone_from(&node.dynamic_tools);
        if !tree {
            snapshot.copy_saved_label(id, &mut prepared)?;
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
        prepared: &super::super::session::PreparedLineage,
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
}
