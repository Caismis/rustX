//! Creates and reaps the sibling Clients owned by an installed local composition.

use super::{Claim, ExecutionTransport, LocalExecutionEndpoint, Slot, sibling, spawn_preparation};
use crate::local_runtime::{
    configuration::{AdmittedSessionConfig, SessionConfigInput, application::ApplicationState},
    session::{SessionError, SessionId},
    session_runtime_manager::{RuntimeManagerError, SessionRuntimeManager, error},
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

impl std::fmt::Debug for PendingClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingClient")
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl PendingClient {
    pub fn endpoint(&self) -> &Arc<LocalExecutionEndpoint> {
        &self.client.endpoint
    }

    pub fn admit(&self) -> Result<(), SessionError> {
        if self.grant.is_cancelled()
            || self
                .client
                .endpoint
                .with_live_control_lane(&self.lane, || ())
                .is_none()
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
        let slot = self.local_client_slot()?;
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
        let pending = self
            .start_claimed_local_client(session, claim, grant, slot, None)
            .await
            .map_err(creation_error)?;
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

    /// The residency owner gets the newly launched Client until runtime
    /// admission. A previously admitted Client stays with its existing owner.
    pub(in crate::local_runtime::session_runtime_manager) async fn prepare_resumed_local_session(
        &self,
        flight: &super::super::Flight,
        session: &SessionId,
        cwd: &Path,
    ) -> Result<(), RuntimeManagerError> {
        if !self
            .execution
            .local
            .roots
            .get()
            .is_some_and(|roots| roots.iter().any(|root| cwd.starts_with(root)))
            || flight
                .with_admission(self, || self.execution.configuration_endpoint(session))?
                .is_some()
        {
            return Ok(());
        }
        flight
            .local_admission
            .starting
            .store(true, std::sync::atomic::Ordering::Release);
        let Some(grant) = self.local_workspace_grant(cwd).map_err(error)? else {
            return Ok(());
        };
        let slot = self.local_client_slot().map_err(error)?;
        let claim = self
            .claim_persisted_execution(session, ExecutionTransport::Unix, None, Some(cwd))
            .await?;
        let Claim::Create(claim) = claim else {
            return Ok(());
        };
        #[cfg(test)]
        {
            let hooks = self.configuration.test_hooks.clone();
            tokio::task::block_in_place(move || hooks.reach("before_resumed_client_preparation"));
        }
        let pending = self
            .start_claimed_local_client(session.clone(), claim, grant, slot, Some(flight))
            .await?;
        let mut owned = flight
            .local_admission
            .pending
            .lock()
            .expect("local runtime admission");
        *owned = Some(Box::new(super::super::lifecycle::PendingLocalRuntime {
            client: pending,
            configuration: None,
        }));
        Ok(())
    }

    pub(in crate::local_runtime::session_runtime_manager) fn local_sessions_configured(
        &self,
    ) -> bool {
        self.execution.local.roots.get().is_some()
    }

    #[cfg(test)]
    pub(in crate::local_runtime::session_runtime_manager) fn available_local_client_slots(
        &self,
    ) -> usize {
        self.execution.local.slots.available_permits()
    }

    fn local_client_slot(&self) -> Result<OwnedSemaphorePermit, SessionError> {
        self.execution
            .local
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| creation_error("the local Session Client limit is reached"))
    }

    async fn start_claimed_local_client(
        &self,
        session: SessionId,
        claim: Arc<Slot>,
        grant: CancellationSignal,
        slot: OwnedSemaphorePermit,
        flight: Option<&super::super::Flight>,
    ) -> Result<PendingClient, RuntimeManagerError> {
        let runtime_root = self.configuration.runtime_root().to_path_buf();
        let id = session.clone();
        let reserved = spawn_preparation(claim, move |workspace| {
            rustx_execution_server::RecoveryFence::require_session_settled(
                &runtime_root,
                &rustx_execution_protocol::SessionId::new(id.as_str()),
            )?;
            LocalExecutionEndpoint::new(&id, workspace)?.with_recovery_fence(&runtime_root)
        })
        .await
        .map_err(error)
        .and_then(std::convert::identity);
        let endpoint = match reserved {
            Ok(endpoint) => endpoint,
            Err(error) => {
                self.execution.close_session_admission(&session);
                self.execution.retire(&session);
                return Err(error);
            }
        };
        #[cfg(test)]
        endpoint.install_startup_hooks(self.configuration.test_hooks.clone());
        let client = match sibling::LocalClient::launch(session.clone(), endpoint.clone()).await {
            Ok(client) => client,
            Err(error) => {
                self.execution.close_session_admission(&session);
                self.execution.retire(&session);
                return Err(error);
            }
        };
        let admitted_lane = || {
            endpoint
                .control()
                .ok_or_else(|| error("the admitted Client disconnected"))
        };
        let lane = match flight.map_or_else(admitted_lane, |flight| {
            flight.with_admission(self, admitted_lane)
        }) {
            Ok(lane) => lane,
            Err(failure) => {
                self.execution.close_session_admission(&session);
                self.reap_local_client(client, &session).await;
                self.execution.retire(&session);
                return Err(failure);
            }
        };
        Ok(PendingClient {
            session,
            lane,
            client,
            grant,
            slot,
        })
    }

    pub(crate) fn local_workspace_skill_grant(
        &self,
        workspace: &Path,
    ) -> Option<CancellationSignal> {
        self.execution
            .reserves_workspace(workspace)
            .then(|| self.local_workspace_grant(workspace).ok().flatten())
            .flatten()
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

    /// A terminal residency failure is published only after the new Client
    /// settles. The Host also joins this task if the initiating load vanished.
    pub(in crate::local_runtime::session_runtime_manager) fn settle_failed_local_load(
        &self,
        pending: PendingClient,
        publish: impl FnOnce() + Send + 'static,
    ) {
        // Close admission before handing the physical wait off to another task.
        self.execution.close_session_admission(&pending.session);
        let owner = self.clone();
        self.execution.local.tasks.spawn(async move {
            owner.discard_local_session(pending).await;
            publish();
        });
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
            let endpoint = owner.execution.close_session_admission(&session);
            if let Some(child) = child {
                owner.reap_local_client(child, &session).await;
            } else if let Some(endpoint) = endpoint
                && let Err(failure) = endpoint.settle().await
            {
                tracing::warn!(%session, %failure, "exited local Client settlement is unconfirmed");
            }
            owner.execution.retire(&session);
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
