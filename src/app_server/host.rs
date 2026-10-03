//! One App Server process owner. Admission and diagnostics aggregate downward.
use super::transport::resources::{ConnectionLease, TransportResources};
use crate::local_runtime::{
    app_server_policy::AppServerPolicy,
    session_runtime_manager::{SessionResidencyDiagnostic, SessionRuntimeManager},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostAdmissionError {
    ServerDraining,
    RequestCapacity,
    AttachmentCapacity,
}

#[derive(Debug, Default)]
struct HostState {
    lifecycle: ServerLifecycle,
    attachments: usize,
    refusals: BTreeMap<String, u64>,
    shutdown_failures: u64,
    shutdown_timeouts: u64,
}
impl HostState {
    fn refuse(&mut self, reason: &str) {
        let count = self.refusals.entry(reason.into()).or_default();
        *count = count.saturating_add(1);
    }
    fn accepting(&mut self) -> Result<(), HostAdmissionError> {
        if self.lifecycle != ServerLifecycle::Accepting {
            self.refuse("server_draining");
            return Err(HostAdmissionError::ServerDraining);
        }
        Ok(())
    }
}
#[derive(Debug)]
struct HostInner {
    #[cfg(test)]
    file_read_probe: Arc<super::product_host::ReadProbe>,
    product_host: Mutex<Option<super::product_host::Authority>>,
    authority_id: String,
    manager: SessionRuntimeManager,
    state: Mutex<HostState>,
    requests: watch::Sender<usize>,
    upload_operations: watch::Sender<usize>,
    transport: Arc<TransportResources>,
    archives: super::archive_download::ArchiveDownloads,
    uploads: super::upload_transfer::UploadTransfers,
    file_reads: Arc<tokio::sync::Semaphore>,
    file_routes: Mutex<
        std::collections::HashMap<
            super::protocol::AttachmentTarget,
            std::sync::Weak<super::connection::Route>,
        >,
    >,
}
#[derive(Clone, Debug)]
pub struct AppServerHost(Arc<HostInner>);

pub(crate) struct ServerOperation(watch::Sender<usize>);
impl Drop for ServerOperation {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n -= 1);
    }
}

/// A host capacity reservation, deliberately independent of a residency pin.
#[derive(Debug)]
pub(crate) struct AttachmentPermit {
    host: Arc<HostInner>,
    released: AtomicBool,
}
impl AttachmentPermit {
    pub(crate) fn release(&self) {
        let mut state = self.host.state.lock().expect("host mutex");
        if !self.released.swap(true, Ordering::Relaxed) {
            state.attachments -= 1;
        }
    }
}
impl Drop for AttachmentPermit {
    fn drop(&mut self) {
        self.release();
    }
}

impl AppServerHost {
    #[cfg(test)]
    pub(crate) fn file_read_probe(&self) -> Arc<super::product_host::ReadProbe> {
        self.0.file_read_probe.clone()
    }
    /// Native process composition only. There is no public RPC for minting or
    /// replacing this credential; replacing it synchronously revokes old reads.
    pub(crate) fn bind_product_host(
        &self,
        credential: Option<super::transport::websocket::Credential>,
    ) {
        *self.0.product_host.lock().expect("Product Host authority") =
            credential.map(super::product_host::Authority::new);
    }
    pub(super) fn authenticate_product_host(
        &self,
        offered: &[&str],
    ) -> Option<tokio_util::sync::CancellationToken> {
        self.0
            .product_host
            .lock()
            .expect("Product Host authority")
            .as_ref()?
            .authenticate(offered)
    }
    pub(super) fn register_file_route(&self, route: &Arc<super::connection::Route>) {
        let mut routes = self.0.file_routes.lock().expect("file routes");
        routes.retain(|_, route| {
            route
                .upgrade()
                .is_some_and(|route| route.attachment.read_authority().is_ok())
        });
        routes.insert(route.target.clone(), Arc::downgrade(route));
    }
    pub(super) fn file_route(
        &self,
        target: &super::protocol::AttachmentTarget,
    ) -> Option<Arc<super::connection::Route>> {
        let route = self
            .0
            .file_routes
            .lock()
            .expect("file routes")
            .get(target)?
            .upgrade()?;
        (route.target == *target && route.attachment.read_authority().is_ok()).then_some(route)
    }
    pub(crate) fn file_reads(&self) -> Arc<tokio::sync::Semaphore> {
        self.0.file_reads.clone()
    }
    pub(crate) fn authority_id(&self) -> &str {
        &self.0.authority_id
    }

    pub(crate) fn uploads(&self) -> &super::upload_transfer::UploadTransfers {
        &self.0.uploads
    }
    pub(crate) fn archives(&self) -> &super::archive_download::ArchiveDownloads {
        &self.0.archives
    }

    #[must_use]
    pub fn new(manager: SessionRuntimeManager, policy: AppServerPolicy) -> Self {
        manager.bind_process_policy(policy);
        Self(Arc::new(HostInner {
            #[cfg(test)]
            file_read_probe: Arc::default(),
            product_host: Mutex::default(),
            authority_id: uuid::Uuid::new_v4().to_string(),
            manager,
            state: Mutex::default(),
            requests: watch::channel(0).0,
            upload_operations: watch::channel(0).0,
            transport: Arc::default(),
            archives: super::archive_download::ArchiveDownloads::default(),
            uploads: super::upload_transfer::UploadTransfers::default(),
            file_reads: Arc::new(tokio::sync::Semaphore::new(
                crate::tools::session_files::SESSION_FILE_MAX_READS,
            )),
            file_routes: Mutex::default(),
        }))
    }
    #[must_use]
    pub fn manager(&self) -> &SessionRuntimeManager {
        &self.0.manager
    }
    #[must_use]
    pub fn policy(&self) -> AppServerPolicy {
        self.0.manager.process_policy()
    }

    /// Commit request ownership and its synchronous downstream admission under
    /// the same boundary as host drain. Never await or execute work here.
    pub(crate) fn admit_request<T>(
        &self,
        commit: impl FnOnce(ServerOperation) -> T,
    ) -> Result<T, HostAdmissionError> {
        let mut state = self.0.state.lock().expect("host mutex");
        state.accepting()?;
        if *self.0.requests.borrow()
            >= self.policy().max_connections * super::transport::IN_FLIGHT_REQUESTS
        {
            state.refuse("request_capacity");
            return Err(HostAdmissionError::RequestCapacity);
        }
        self.0.requests.send_modify(|n| *n += 1);
        Ok(commit(ServerOperation(self.0.requests.clone())))
    }
    pub(crate) fn prepare_upload<T>(
        &self,
        commit: impl FnOnce() -> T,
    ) -> Result<T, HostAdmissionError> {
        // Keep the admission mutex through publication of the prepared intent.
        let mut state = self.0.state.lock().expect("host mutex");
        state.accepting()?;
        Ok(commit())
    }
    pub(crate) fn admit_upload<T>(
        &self,
        commit: impl FnOnce(ServerOperation) -> T,
    ) -> Result<T, HostAdmissionError> {
        let mut state = self.0.state.lock().expect("host mutex");
        state.accepting()?;
        self.0.upload_operations.send_modify(|n| *n += 1);
        Ok(commit(ServerOperation(self.0.upload_operations.clone())))
    }
    pub(crate) fn admit_attachment(&self) -> Result<AttachmentPermit, HostAdmissionError> {
        let mut state = self.0.state.lock().expect("host mutex");
        state.accepting()?;
        if state.attachments >= self.policy().max_external_attachments {
            state.refuse("attachment_capacity");
            return Err(HostAdmissionError::AttachmentCapacity);
        }
        state.attachments += 1;
        Ok(AttachmentPermit {
            host: self.0.clone(),
            released: AtomicBool::new(false),
        })
    }
    pub(crate) fn admit_connection(&self, websocket: bool) -> Option<ConnectionLease> {
        let mut state = self.0.state.lock().expect("host mutex");
        if state.accepting().is_err() {
            self.0.transport.refuse();
            return None;
        }
        self.0.transport.reserve(
            websocket,
            if websocket {
                self.policy().max_connections
            } else {
                1
            },
        )
    }
    pub(crate) fn attachment_capacity_refused(&self) {
        self.0
            .state
            .lock()
            .expect("host mutex")
            .refuse("attachment_capacity");
    }
    pub(crate) fn transport_failure(&self) {
        self.0.transport.delivery_failed();
    }
    pub(crate) fn server_draining(&self) -> bool {
        self.0.state.lock().expect("host mutex").lifecycle != ServerLifecycle::Accepting
    }

    #[cfg(test)]
    pub(crate) fn admission_boundary_is_held(&self) -> bool {
        self.0.state.try_lock().is_err()
    }

    /// The single host commit shared by requests, attachments, and connections.
    /// # Panics
    /// Panics if the host mutex is poisoned.
    pub fn begin_drain(&self) {
        let mut state = self.0.state.lock().expect("host mutex");
        if state.lifecycle == ServerLifecycle::Accepting {
            state.lifecycle = ServerLifecycle::Draining;
        }
        drop(state);
        self.uploads().revoke_prepared();
    }
    /// Supervise existing runtimes immediately, including when an accepted
    /// request is still pending. A final pass after request settlement covers
    /// every late load claimed by a pre-drain request. Neither pass fails fast.
    /// # Panics
    /// Panics if the host mutex is poisoned.
    pub async fn drain(&self) -> Vec<String> {
        self.begin_drain();
        let requests = async {
            self.0
                .upload_operations
                .subscribe()
                .wait_for(|n| *n == 0)
                .await
                .expect("upload owner");
            self.0
                .requests
                .subscribe()
                .wait_for(|n| *n == 0)
                .await
                .expect("host request owner");
        };
        let (mut failures, ()) = tokio::join!(self.manager().drain_all_runtimes(), requests);
        failures.extend(self.manager().drain_all_runtimes().await);
        failures.sort();
        failures.dedup();
        if !failures.is_empty() {
            self.0.state.lock().expect("host mutex").shutdown_failures += 1;
        }
        failures
    }
    pub(crate) fn finish_drain(&self) -> Result<(), String> {
        let mut state = self.0.state.lock().expect("host mutex");
        if state.lifecycle != ServerLifecycle::Draining
            || !self.manager().is_empty()
            || *self.0.requests.borrow() != 0
            || *self.0.upload_operations.borrow() != 0
            || !self.uploads().is_empty()
            || state.attachments != 0
            || self.0.transport.has_connections()
        {
            return Err("server settlement is not proven".into());
        }
        state.lifecycle = ServerLifecycle::Terminated;
        Ok(())
    }
    pub(crate) fn forced_resources(&self, timeout: bool) -> String {
        if timeout {
            self.0.state.lock().expect("host mutex").shutdown_timeouts += 1;
        }
        format!(
            "{}; pending_protocol_operations={}; physical_connections_remain={}; active_uploads={}",
            self.manager().unproven_resources(),
            *self.0.requests.borrow(),
            self.0.transport.has_connections(),
            *self.0.upload_operations.borrow()
        )
    }
    /// Observations are never used as settlement or admission authority.
    /// # Panics
    /// Panics if an ownership mutex is poisoned.
    #[must_use]
    pub fn diagnostics(&self) -> ServerDiagnostics {
        let (lifecycle, external_attachments, mut refusals, shutdown_failures, shutdown_timeouts) = {
            let state = self.0.state.lock().expect("host mutex");
            (
                state.lifecycle,
                state.attachments,
                state.refusals.clone(),
                state.shutdown_failures,
                state.shutdown_timeouts,
            )
        };
        let residency = self.manager().diagnostics();
        for (reason, count) in residency.admission_refusals {
            *refusals.entry(reason).or_default() += count;
        }
        ServerDiagnostics {
            lifecycle,
            policy: self.policy(),
            external_attachments,
            loaded: residency.loaded,
            loading: residency.loading,
            unloading: residency.unloading,
            active_roots: residency.active_roots,
            sessions: residency.sessions,
            admission_refusals: refusals,
            unload_failures: residency.unload_failures,
            shutdown_failures,
            shutdown_timeouts,
            transport: self.0.transport.snapshot(),
        }
    }
}
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
pub enum ServerLifecycle {
    #[default]
    Accepting,
    Draining,
    Terminated,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ServerDiagnostics {
    pub lifecycle: ServerLifecycle,
    pub policy: AppServerPolicy,
    pub loaded: usize,
    pub loading: usize,
    pub unloading: usize,
    pub active_roots: usize,
    pub external_attachments: usize,
    pub sessions: Vec<SessionResidencyDiagnostic>,
    pub admission_refusals: std::collections::BTreeMap<String, u64>,
    pub shutdown_failures: u64,
    pub shutdown_timeouts: u64,
    pub unload_failures: u64,
    pub transport: super::transport::resources::TransportDiagnostics,
}
