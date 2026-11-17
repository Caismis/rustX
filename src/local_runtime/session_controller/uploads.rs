//! Correlated durable Session upload lifecycle and receipt content validation.

use super::{SessionController, SessionError, SessionId, SessionNodeId};

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
        files: Vec<super::super::session::uploads::UploadFile>,
    ) -> Result<Vec<super::super::session::uploads::UploadedFile>, SessionError> {
        self.upload_correlated(id, node, uuid::Uuid::now_v7().simple().to_string(), files)
            .await
    }

    /// Read the exact native operation; a missing reply never allocates again.
    /// # Errors
    /// Unknown Sessions and malformed operation identities are rejected.
    pub async fn upload_status(
        &self,
        id: &SessionId,
        operation: &str,
    ) -> Result<super::super::session::uploads::UploadOutcome, SessionError> {
        self.catalog
            .lock()
            .await
            .upload_registry(id)?
            .outcome(id, operation)
            .map_err(|e| SessionError::Catalog {
                detail: e.to_string(),
            })
    }

    /// Correlation is durably claimed before any filesystem side effect.
    /// # Errors
    /// Repeated operations, invalid metadata and storage failures are rejected.
    /// # Panics
    /// Only if an internal claimed allocation or test gate invariant is broken.
    pub async fn upload_correlated(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        operation: String,
        files: Vec<super::super::session::uploads::UploadFile>,
    ) -> Result<Vec<super::super::session::uploads::UploadedFile>, SessionError> {
        use super::super::session::uploads::{UploadMetadata, UploadOutcome, validate_metadata};
        validate_metadata(
            &files
                .iter()
                .map(|f| UploadMetadata {
                    name: f.name.clone(),
                    size: f.data.size(),
                })
                .collect::<Vec<_>>(),
        )
        .map_err(|e| SessionError::Catalog {
            detail: e.to_string(),
        })?;
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
        if registry.outcome(id, &operation).map_err(fail)? != UploadOutcome::Absent {
            return Err(SessionError::Catalog {
                detail: "upload operation already claimed; read its status".into(),
            });
        }
        let batch = registry.claim(workspace, &files).map_err(fail)?;
        registry
            .allocations
            .get_mut(&batch)
            .expect("claimed batch")
            .operation_id = Some(operation);
        // Durable ownership first. Even failed materialization is deletion work.
        self.catalog
            .lock()
            .await
            .commit_uploads(id, registry.clone())?;
        let materialization = {
            let registry = registry.clone();
            let id = id.clone();
            let batch = batch.clone();
            #[cfg(test)]
            let checkpoint = super::super::session::uploads::tests::capture_checkpoint();
            tokio::task::spawn_blocking(move || {
                #[cfg(test)]
                {
                    super::super::session::uploads::tests::with_checkpoint(checkpoint, || {
                        registry.materialize(&id, &batch, &files)
                    })
                }
                #[cfg(not(test))]
                {
                    registry.materialize(&id, &batch, &files)
                }
            })
            .await
            .unwrap_or_else(|error| Err(std::io::Error::other(error)))
        };
        #[cfg(test)]
        {
            let gate = self.upload_commit_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                tokio::task::spawn_blocking(move || gate.enter())
                    .await
                    .unwrap();
            }
        }
        if let Err(error) = materialization.and_then(|()| registry.verify_materialized(id, &batch))
        {
            self.settle_upload_failed(id, registry, &batch).await?;
            return Err(fail(error));
        }

        registry
            .allocations
            .get_mut(&batch)
            .expect("claimed batch")
            .ready = true;
        // Semantic commit point: synced complete files plus durable ready registry.
        let ready_commit = self
            .catalog
            .lock()
            .await
            .commit_uploads(id, registry.clone());
        if let Err(error) = ready_commit {
            if !error.committed() {
                self.settle_upload_failed(id, registry, &batch).await?;
            }
            return Err(error);
        }
        registry.receipts(id, &batch).map_err(fail)
    }
    async fn settle_upload_failed(
        &self,
        id: &SessionId,
        mut registry: super::super::session::uploads::UploadRegistry,
        batch: &str,
    ) -> Result<(), SessionError> {
        let allocation = registry.allocations.get_mut(batch).expect("claimed batch");
        allocation.ready = false;
        allocation.failed = true;
        self.catalog.lock().await.commit_uploads(id, registry)
    }

    /// Validate server receipts and author typed canonical content.
    /// # Errors
    /// Unknown, incomplete, cross-Session and over-budget receipt collections are rejected.
    pub async fn uploaded_content(
        &self,
        id: &SessionId,
        receipts: &[super::super::session::uploads::UploadReceipt],
    ) -> Result<Vec<crate::message::types::UserContentBlock>, SessionError> {
        let registry = self.catalog.lock().await.upload_registry(id)?;
        registry
            .receipt_content(id, receipts)
            .map_err(|e| SessionError::Catalog {
                detail: e.to_string(),
            })
    }
}
