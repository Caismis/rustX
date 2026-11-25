//! Creates and reaps the sibling Clients owned by an installed local composition.

use super::{Claim, ExecutionTransport, LocalExecutionEndpoint, sibling, spawn_preparation};
use crate::local_runtime::{
    configuration::{AdmittedSessionConfig, SessionConfigInput, application::ApplicationState},
    session::{SessionError, SessionId},
    session_runtime_manager::SessionRuntimeManager,
};
use crate::runtime::CancellationSignal;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

const MAX_LOCAL_SESSION_CLIENTS: usize = 8;

pub(in crate::local_runtime::session_runtime_manager) struct LocalSessions {
    roots: OnceLock<BTreeSet<PathBuf>>,
    slots: Arc<Semaphore>,
    tasks: TaskTracker,
    stop: CancellationToken,
    reap_failure: Mutex<Option<String>>,
}

impl Default for LocalSessions {
    fn default() -> Self {
        Self {
            roots: OnceLock::new(),
            slots: Arc::new(Semaphore::new(MAX_LOCAL_SESSION_CLIENTS)),
            tasks: TaskTracker::new(),
            stop: CancellationToken::new(),
            reap_failure: Mutex::default(),
        }
    }
}

pub(in crate::local_runtime::session_runtime_manager) struct PendingClient {
    pub session: SessionId,
    lane: rustx_execution_server::ControlHandle,
    client: sibling::LocalClient,
    grant: CancellationSignal,
    slot: OwnedSemaphorePermit,
}

impl PendingClient {
    pub fn admit(&self) -> Result<(), SessionError> {
        if self.grant.is_cancelled()
            || !self
                .client
                .endpoint
                .with_live_control_lane(&self.lane, || ())
                .is_some()
        {
            return Err(creation_error(
                "the local Client or its Workspace grant is unavailable",
            ));
        }
        Ok(())
    }
}

impl SessionRuntimeManager {
    pub(crate) fn configure_local_sessions(&self, roots: Vec<PathBuf>) -> Result<(), String> {
        if roots.is_empty() {
            return Ok(());
        }
        let roots = roots
            .into_iter()
            .map(|root| {
                if !rustx_execution_protocol::is_normalized_absolute_path(&root) {
                    return Err("a local Session root must be a normalized absolute path".into());
                }
                Ok(root)
            })
            .collect::<Result<_, String>>()?;
        self.execution
            .local
            .roots
            .set(roots)
            .map_err(|_| "local Session roots were already configured".into())
    }

    fn local_workspace_grant(
        &self,
        cwd: &Path,
    ) -> Result<Option<CancellationSignal>, SessionError> {
        let Some(roots) = self.execution.local.roots.get() else {
            return Ok(None);
        };
        if !rustx_execution_protocol::is_normalized_absolute_path(cwd) {
            return Err(creation_error(
                "the local Session Workspace must be a normalized absolute path",
            ));
        }
        if roots.contains(cwd) {
            return Ok(Some(CancellationSignal::new()));
        }
        roots
            .iter()
            .any(|root| cwd.starts_with(root))
            .then(|| self.environment.retirement(cwd))
            .flatten()
            .map(Some)
            .ok_or_else(|| creation_error("the Workspace is outside the local Session grants"))
    }

    pub(in crate::local_runtime::session_runtime_manager) async fn prepare_local_session(
        &self,
        input: &SessionConfigInput,
    ) -> Result<Option<(PendingClient, AdmittedSessionConfig)>, SessionError> {
        let Some(grant) = self.local_workspace_grant(&input.cwd)? else {
            return Ok(None);
        };
        let slot = self
            .execution
            .local
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| creation_error("the local Session Client limit is reached"))?;
        let session = SessionId::generate();
        let claim = {
            let registry = self.registry.0.lock().expect("registry mutex");
            if registry.draining || grant.is_cancelled() {
                return Err(creation_error("local Session admission closed"));
            }
            self.execution
                .claim(&session, input.cwd.clone(), ExecutionTransport::Unix)
        }
        .map_err(creation_error)?;
        let Claim::Create(claim) = claim else {
            unreachable!("fresh local Session identity already claimed");
        };
        let runtime_root = self.configuration.runtime_root().to_path_buf();
        let id = session.clone();
        let reserved = spawn_preparation(claim, move |workspace| {
            LocalExecutionEndpoint::new(&id, workspace)?.with_recovery_fence(&runtime_root)
        })
        .await
        .map_err(creation_error)
        .and_then(|result| result.map_err(creation_error));
        let endpoint = match reserved {
            Ok(endpoint) => endpoint,
            Err(error) => {
                self.execution.close_session_admission(&session);
                self.execution.retire(&session);
                return Err(error);
            }
        };
        let client = match sibling::LocalClient::launch(session.clone(), endpoint.clone()).await {
            Ok(client) => client,
            Err(error) => {
                self.execution.close_session_admission(&session);
                self.execution.retire(&session);
                return Err(creation_error(error));
            }
        };
        let Some(lane) = endpoint.control() else {
            self.execution.close_session_admission(&session);
            self.reap_local_client(client, &session).await;
            self.execution.retire(&session);
            return Err(creation_error("the admitted Client disconnected"));
        };
        let pending = PendingClient {
            session,
            lane,
            client,
            grant,
            slot,
        };
        let configuration = self.configuration.clone();
        let credentials = self.credentials.clone();
        let input = input.clone();
        let endpoint = pending.client.endpoint.clone();
        let captured = tokio::task::spawn_blocking(move || {
            ApplicationState::initial_bound_session(
                &configuration,
                &input,
                &credentials,
                &endpoint,
                false,
            )
        })
        .await
        .map_err(creation_error)
        .and_then(|result| result.map_err(creation_error));
        match captured {
            Ok(capture) => Ok(Some((pending, capture))),
            Err(error) => {
                self.discard_local_session(pending).await;
                Err(error)
            }
        }
    }

    pub(in crate::local_runtime::session_runtime_manager) async fn discard_local_session(
        &self,
        pending: PendingClient,
    ) {
        self.execution.close_session_admission(&pending.session);
        self.reap_local_client(pending.client, &pending.session)
            .await;
        self.execution.retire(&pending.session);
    }

    pub(in crate::local_runtime::session_runtime_manager) fn keep_local_session(
        &self,
        pending: PendingClient,
    ) {
        let owner = self.clone();
        let stop = self.execution.local.stop.child_token();
        self.execution.local.tasks.spawn(async move {
            let PendingClient {
                session,
                client,
                grant,
                slot,
                ..
            } = pending;
            let _slot = slot;
            let wait = wait_for_child(owner.clone(), client, stop.clone());
            tokio::pin!(wait);
            let child = tokio::select! {
                result = &mut wait => result,
                () = grant.cancelled() => {
                    owner.execution.close_session_admission(&session);
                    stop.cancel();
                    wait.await
                },
            };
            if let Some(child) = child {
                owner.execution.close_session_admission(&session);
                owner.reap_local_client(child, &session).await;
            }
        });
    }

    pub(in crate::local_runtime::session_runtime_manager) fn stop_local_sessions(&self) {
        self.execution.local.stop.cancel();
    }

    async fn reap_local_client(&self, client: sibling::LocalClient, session: &SessionId) {
        if let Err(error) = settle_child(client, session).await {
            tracing::warn!(%session, %error, "local Client reap failed");
            if self.execution.local.stop.is_cancelled() {
                self.execution
                    .local
                    .reap_failure
                    .lock()
                    .expect("local Client reap result")
                    .get_or_insert_with(|| format!("local Client {session}: {error}"));
            }
        }
    }

    pub(crate) async fn join_local_sessions(&self) -> Result<(), String> {
        self.execution.local.tasks.close();
        self.execution.local.tasks.wait().await;
        self.execution
            .local
            .reap_failure
            .lock()
            .expect("local Client reap result")
            .take()
            .map_or(Ok(()), Err)
    }
}

fn creation_error(error: impl std::fmt::Display) -> SessionError {
    SessionError::Catalog {
        detail: format!("local Session execution: {error}"),
    }
}

async fn settle_child(mut client: sibling::LocalClient, session: &SessionId) -> Result<(), String> {
    if let Err(error) = client.endpoint.clone().settle().await {
        tracing::warn!(%session, %error, "local Client settlement is unconfirmed");
    }
    let budget = rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT;
    match tokio::time::timeout(budget, client.child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(format!("exited {status}")),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => {
            let status = client
                .child
                .terminate()
                .await
                .map_err(|error| error.to_string())?;
            Err(format!("needed forced termination ({status})"))
        }
    }
}

#[cfg(target_os = "linux")]
async fn wait_for_child(
    owner: SessionRuntimeManager,
    client: sibling::LocalClient,
    stop: CancellationToken,
) -> Option<sibling::LocalClient> {
    sibling::maintain(owner, client, stop).await
}

#[cfg(not(target_os = "linux"))]
async fn wait_for_child(
    _owner: SessionRuntimeManager,
    mut client: sibling::LocalClient,
    stop: CancellationToken,
) -> Option<sibling::LocalClient> {
    tokio::select! {
        biased;
        () = stop.cancelled() => Some(client),
        result = client.child.wait() => {
            if !result.as_ref().is_ok_and(std::process::ExitStatus::success) {
                tracing::warn!(?result, "local Client exited without confirmed settlement");
            }
            None
        }
    }
}
