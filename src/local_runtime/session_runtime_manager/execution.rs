//! Process-owned local execution binding claims for Sessions.
//!
//! The registry reserves one Server-side execution binding per Session for
//! the trusted native composer only. Nothing here is exposed through the
//! App Server protocol: credential and bootstrap material never cross a
//! client request. Admission is decided under the catalog visibility cut
//! and the residency mutex together; the private publication — credential
//! generation, socket bind, bootstrap write — happens exactly once on a
//! spawned blocking task after the slot is claimed. A published endpoint is
//! never replaced, a closed claim is never recovered, and endpoint teardown
//! never runs under a registry or residency mutex.

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::num::NonZeroU64;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rustx_execution_protocol::{
    Capability, ClientInstanceId, EnvironmentId, Generation, is_normalized_absolute_path,
};
use rustx_execution_server::{Binding, Reservation};

use super::super::session::SessionId;
use super::{RuntimeManagerError, SessionRuntimeManager, error};

/// One Session's local execution endpoint, published once.
///
/// Owned by the trusted native composer: the App Server protocol has no
/// path to the listener, the bundle, or the credential inside it. Nothing
/// accepts a connection or spawns a Client by itself — the composer that
/// holds the endpoint decides both.
pub struct LocalExecutionEndpoint {
    binding: Binding,
    listener: UnixListener,
    bootstrap: PathBuf,
    _private: tempfile::TempDir,
}

impl LocalExecutionEndpoint {
    /// Reserves the binding inside a fresh owner-only private directory.
    ///
    /// `workspace` must be the exact normalized absolute path the Session
    /// catalog recorded. It is validated lexically and never touched on
    /// disk — the path authorizes the binding's workspace claim, it is not
    /// opened, canonicalized, or created.
    ///
    /// # Errors
    ///
    /// Returns `io::ErrorKind::InvalidInput` when `workspace` is not a
    /// normalized absolute path, or the OS error when the private
    /// directory, socket, or bootstrap bundle cannot be created.
    fn new(session: &SessionId, workspace: PathBuf) -> io::Result<Self> {
        if !is_normalized_absolute_path(&workspace) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace must be a normalized absolute path",
            ));
        }
        let private = tempfile::Builder::new()
            .prefix("rx-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?;
        // Only the private directory is canonicalized — the socket and
        // bundle must be normalized absolute children for `reserve_local`.
        let root = private.path().canonicalize()?;
        let reservation = Reservation {
            session: rustx_execution_protocol::SessionId::new(session.as_str()),
            environment: EnvironmentId::new(uuid::Uuid::now_v7().to_string()),
            instance: ClientInstanceId::new(uuid::Uuid::now_v7().to_string()),
            generation: Generation::new(NonZeroU64::MIN),
            workspace,
            required_capabilities: BTreeSet::from([Capability::Connection]),
        };
        let (binding, listener) =
            Binding::reserve_local(reservation, &root.join("s"), &root.join("b"))?;
        Ok(Self {
            binding,
            listener,
            bootstrap: root.join("b"),
            _private: private,
        })
    }

    /// The owner-only bootstrap bundle path the trusted composer hands to
    /// the Execution Client it chooses to spawn.
    #[must_use]
    pub fn bootstrap_path(&self) -> &Path {
        &self.bootstrap
    }

    /// The bound local listener for this binding.
    #[must_use]
    pub fn listener(&self) -> &UnixListener {
        &self.listener
    }

    /// The single arbitration point for Hellos and typed exchanges. The
    /// typed gate is the only read path; raw reservation or credential
    /// material is never exposed.
    #[must_use]
    pub fn binding(&self) -> &Binding {
        &self.binding
    }
}

impl std::fmt::Debug for LocalExecutionEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalExecutionEndpoint")
            .field("bootstrap", &self.bootstrap)
            .finish_non_exhaustive()
    }
}

/// The claimable state of one Session's execution endpoint.
struct Slot {
    /// The catalog workspace captured at claim; compared exactly, never
    /// re-derived while the slot lives.
    workspace: PathBuf,
    phase: Mutex<Phase>,
}

enum Phase {
    /// Claimed; the private endpoint is being prepared on a blocking task.
    Preparing,
    /// The one published endpoint; every later claim returns this value.
    Published(Arc<LocalExecutionEndpoint>),
    /// Fenced by delete/drain or a failed preparation. A published endpoint
    /// is kept inside the slot so its teardown never happens under the
    /// phase or registry mutex — it is released when the slot itself is
    /// dropped outside all locks. The value is deliberately never read.
    #[allow(dead_code)]
    Closed(Option<Arc<LocalExecutionEndpoint>>),
}

impl Slot {
    /// Publishes the prepared endpoint while the slot is still open. On
    /// refusal the caller still owns `endpoint` and drops it outside all
    /// locks.
    fn publish(&self, endpoint: &Arc<LocalExecutionEndpoint>) -> bool {
        let mut phase = self.phase.lock().expect("execution slot phase");
        if matches!(*phase, Phase::Preparing) {
            *phase = Phase::Published(endpoint.clone());
            return true;
        }
        false
    }

    /// Terminally closes the slot: an in-flight publication is refused, a
    /// published binding is revoked under its own mutex, and its endpoint
    /// stays owned by the slot until the slot drops outside locks. No
    /// filesystem I/O happens here.
    fn close(&self) {
        let mut phase = self.phase.lock().expect("execution slot phase");
        match &*phase {
            Phase::Preparing => *phase = Phase::Closed(None),
            Phase::Published(endpoint) => {
                let endpoint = endpoint.clone();
                endpoint.binding().revoke();
                *phase = Phase::Closed(Some(endpoint));
            }
            Phase::Closed(_) => {}
        }
    }
}

/// The claim a caller won under the registry mutex.
enum Claim {
    /// The previously published endpoint; never replaced.
    Existing(Arc<LocalExecutionEndpoint>),
    /// The worker prepares and publishes the endpoint on this slot.
    Create(Arc<Slot>),
}

/// Runs preparation and publication inside the blocking task: a caller that
/// drops the returned handle can never strand the slot in `Preparing` —
/// the worker either publishes the endpoint or closes the claim.
fn spawn_preparation(
    slot: Arc<Slot>,
    prepare: impl FnOnce(PathBuf) -> io::Result<LocalExecutionEndpoint> + Send + 'static,
) -> tokio::task::JoinHandle<Result<Arc<LocalExecutionEndpoint>, RuntimeManagerError>> {
    tokio::task::spawn_blocking(move || {
        struct CloseOnFailure<'a>(Option<&'a Slot>);
        impl Drop for CloseOnFailure<'_> {
            fn drop(&mut self) {
                if let Some(slot) = self.0 {
                    slot.close();
                }
            }
        }
        let mut guard = CloseOnFailure(Some(slot.as_ref()));
        let endpoint = Arc::new(prepare(slot.workspace.clone()).map_err(error)?);
        if !slot.publish(&endpoint) {
            return Err(error("execution binding fenced before publication"));
        }
        guard.0 = None;
        Ok(endpoint)
    })
}

/// Per-process claim registry for local execution endpoints.
///
/// Session admission is fenced by residency state — a draining manager or a
/// Session under retirement — never by endpoint status. Endpoint drop order
/// guarantees teardown only outside locks.
#[derive(Default)]
pub(super) struct SessionExecutionRegistry(Mutex<HashMap<SessionId, Arc<Slot>>>);

impl SessionExecutionRegistry {
    /// Claims the Session's execution slot under the registry mutex alone.
    /// An occupied slot must name the identical workspace; an occupied claim
    /// is never swapped to a different path.
    fn claim(&self, session: &SessionId, workspace: PathBuf) -> Result<Claim, RuntimeManagerError> {
        let mut slots = self.0.lock().expect("execution registry mutex");
        if let Some(slot) = slots.get(session) {
            if slot.workspace.as_os_str() != workspace.as_os_str() {
                return Err(error("Session workspace changed since the execution claim"));
            }
            let phase = slot.phase.lock().expect("execution slot phase");
            return match &*phase {
                Phase::Published(endpoint) => Ok(Claim::Existing(endpoint.clone())),
                Phase::Preparing => Err(error("execution binding preparation in progress")),
                Phase::Closed(_) => Err(error("execution binding is closed; recovery is required")),
            };
        }
        // The slot is installed before any credential or socket exists, so
        // exactly one claimant ever prepares.
        let slot = Arc::new(Slot {
            workspace,
            phase: Mutex::new(Phase::Preparing),
        });
        slots.insert(session.clone(), slot.clone());
        Ok(Claim::Create(slot))
    }

    /// Fences the Session's existing claim. An absent Session gains no
    /// state: fencing never creates a slot.
    pub(super) fn fence(&self, session: &SessionId) {
        let slot = self
            .0
            .lock()
            .expect("execution registry mutex")
            .get(session)
            .cloned();
        if let Some(slot) = slot {
            slot.close();
        }
    }

    /// Fences every existing claim. Later claims are refused by the
    /// manager's permanent `draining` admission gate.
    pub(super) fn fence_all(&self) {
        let slots: Vec<Arc<Slot>> = self
            .0
            .lock()
            .expect("execution registry mutex")
            .values()
            .cloned()
            .collect();
        for slot in slots {
            slot.close();
        }
    }

    /// Removes the Session's claim so its endpoint teardown runs outside
    /// the registry and residency mutexes. The slot closes first in case a
    /// publication is still in flight; the last `Arc` owner drops the
    /// endpoint.
    pub(super) fn retire(&self, session: &SessionId) {
        let slot = self
            .0
            .lock()
            .expect("execution registry mutex")
            .remove(session);
        if let Some(slot) = slot {
            slot.close();
            drop(slot);
        }
    }
}

impl SessionRuntimeManager {
    /// Reserves the Session's process-owned local execution endpoint for the
    /// trusted native composer.
    ///
    /// Admission is decided under the catalog visibility cut and the
    /// residency mutex together: a draining manager or a Session under
    /// retirement never prepares a binding. At most one preparation runs —
    /// the slot is claimed synchronously before any credential, socket, or
    /// bootstrap exists — and every later claim of a published endpoint
    /// returns the identical endpoint, never a replacement. A closed claim
    /// stays closed; there is no recovery path.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeManagerError::Draining`] while the manager drains and
    /// [`RuntimeManagerError::TransitionFailed`] for missing catalog
    /// metadata, retirement fencing, a concurrent preparation, a closed
    /// claim, or a failed endpoint preparation.
    ///
    /// # Panics
    ///
    /// Panics if the residency mutex is poisoned.
    pub async fn reserve_local_execution(
        &self,
        session: &SessionId,
    ) -> Result<Arc<LocalExecutionEndpoint>, RuntimeManagerError> {
        let claim = {
            let catalog = self.sessions.catalog.lock().await;
            let (_, settings) = catalog.lineage(session, None).map_err(error)?;
            // Under the same catalog visibility cut: residency admission is
            // the only authority that may fence a claim.
            let registry = self.registry.0.lock().expect("registry mutex");
            if registry.draining {
                return Err(RuntimeManagerError::Draining);
            }
            if registry.retiring_sessions.contains(session) {
                return Err(error("Session retirement has fenced runtime admission"));
            }
            self.execution.claim(session, settings.cwd.clone())
            // Both guards release here, before any I/O or await.
        };
        match claim? {
            Claim::Existing(endpoint) => Ok(endpoint),
            Claim::Create(slot) => {
                // The spawned preparation owns publication: a dropped
                // caller never reissues and never strands the slot.
                let session = session.clone();
                spawn_preparation(slot, move |workspace| {
                    LocalExecutionEndpoint::new(&session, workspace)
                })
                .await
                .map_err(error)?
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use rustx_execution_protocol::transport::{read_record, write_record};
    use rustx_execution_protocol::{Bootstrap, Hello, HelloReply, PROTOCOL_VERSION};
    use rustx_execution_server::BindingPhase;

    use crate::credentials::CredentialSnapshot;
    use crate::local_runtime::composition::LocalRuntimeDependencies;
    use crate::local_runtime::configuration::UserConfigManager;
    use crate::local_runtime::configuration::{SessionConfigInput, UserConfigSources};
    use crate::local_runtime::session::SessionPersistentState;
    use crate::local_runtime::session::deletion::SessionDeleteResult;
    use crate::local_runtime::session_controller::SessionController;

    use super::*;

    fn workspace() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap();
        (directory, path)
    }

    fn bundle(endpoint: &LocalExecutionEndpoint) -> Bootstrap {
        let mut file = std::fs::File::open(endpoint.bootstrap_path()).unwrap();
        read_record(&mut file).unwrap()
    }

    fn create(
        registry: &SessionExecutionRegistry,
        session: &SessionId,
        workspace: &Path,
    ) -> Arc<Slot> {
        match registry.claim(session, workspace.to_path_buf()).unwrap() {
            Claim::Create(slot) => slot,
            Claim::Existing(_) => panic!("no endpoint has been published yet"),
        }
    }

    fn prepare(slot: &Slot, session: &SessionId, workspace: &Path) -> Arc<LocalExecutionEndpoint> {
        let endpoint =
            Arc::new(LocalExecutionEndpoint::new(session, workspace.to_path_buf()).unwrap());
        assert!(slot.publish(&endpoint));
        endpoint
    }

    #[test]
    fn concurrent_claims_publish_one_endpoint_that_every_later_claim_shares() {
        let registry = SessionExecutionRegistry::default();
        let session = SessionId::generate();
        let (_dir, workspace) = workspace();
        let barrier = std::sync::Barrier::new(2);
        let outcomes: Vec<_> = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..2 {
                let barrier = &barrier;
                let registry = &registry;
                let session = &session;
                let workspace = &workspace;
                handles.push(scope.spawn(move || {
                    barrier.wait();
                    registry.claim(session, workspace.clone())
                }));
            }
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        let mut slot = None;
        for outcome in outcomes {
            match outcome {
                Ok(Claim::Create(claimed)) => assert!(slot.replace(claimed).is_none()),
                Ok(Claim::Existing(_)) => panic!("no endpoint is published yet"),
                Err(error) => assert!(
                    error.to_string().contains("preparation in progress"),
                    "{error}"
                ),
            }
        }
        let slot = slot.expect("exactly one claimant prepares");
        let endpoint = prepare(&slot, &session, &workspace);
        let published = match registry.claim(&session, workspace.clone()).unwrap() {
            Claim::Existing(endpoint) => endpoint,
            Claim::Create(_) => panic!("the published endpoint is claimed as-is"),
        };
        assert!(Arc::ptr_eq(&published, &endpoint));
        // The one bundle — including its credential — is byte-identical for
        // every claim because a Session ever publishes a single bootstrap.
        assert_eq!(
            std::fs::read(published.bootstrap_path()).unwrap(),
            std::fs::read(endpoint.bootstrap_path()).unwrap()
        );
    }

    #[test]
    fn two_sessions_sharing_one_workspace_hold_distinct_binding_material() {
        let registry = SessionExecutionRegistry::default();
        let (_dir, workspace) = workspace();
        let sessions = [SessionId::generate(), SessionId::generate()];
        let endpoints: Vec<_> = sessions
            .iter()
            .map(|session| {
                let slot = create(&registry, session, &workspace);
                prepare(&slot, session, &workspace)
            })
            .collect();
        let [a, b]: [_; 2] = endpoints.try_into().expect("two endpoints");
        let a_bundle = bundle(&a);
        let b_bundle = bundle(&b);
        assert_eq!(a_bundle.workspace, b_bundle.workspace);
        assert_ne!(a_bundle.session, b_bundle.session);
        assert_ne!(a_bundle.socket_path, b_bundle.socket_path);
        assert_ne!(a_bundle.environment, b_bundle.environment);
        assert_ne!(a_bundle.instance, b_bundle.instance);
        assert_ne!(
            std::fs::read(a.bootstrap_path()).unwrap(),
            std::fs::read(b.bootstrap_path()).unwrap(),
            "distinct credentials: bundles never repeat"
        );
        assert!(
            a_bundle.credential.as_bytes() != b_bundle.credential.as_bytes(),
            "two Sessions never share a binding credential"
        );
    }

    #[test]
    fn a_fenced_claim_rejects_the_late_publication_forever() {
        let registry = SessionExecutionRegistry::default();
        let session = SessionId::generate();
        let (_dir, workspace) = workspace();
        let slot = create(&registry, &session, &workspace);
        registry.fence(&session);
        let endpoint = Arc::new(LocalExecutionEndpoint::new(&session, workspace.clone()).unwrap());
        // The prepared endpoint is real but its binding is still Reserved;
        // publication is refused, and the stale endpoint never goes live.
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Reserved);
        assert!(!slot.publish(&endpoint));
        drop(endpoint);
        match registry.claim(&session, workspace.clone()) {
            Err(error) => assert!(error.to_string().contains("recovery"), "{error}"),
            Ok(_) => panic!("a fenced claim is never reissued"),
        }
    }

    #[tokio::test]
    async fn a_dropped_caller_never_strands_the_claim() {
        let registry = SessionExecutionRegistry::default();
        let session = SessionId::generate();
        let (_dir, workspace) = workspace();
        let slot = create(&registry, &session, &workspace);
        // The worker parks inside preparation so the caller's disappearance
        // is provably ordered before publication.
        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let parked = session.clone();
        let handle = spawn_preparation(slot, move |path| {
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
            LocalExecutionEndpoint::new(&parked, path)
        });
        // A secondary task awaits the real worker handle and forwards its
        // result to a oneshot waiter the test then drops; joining that task
        // proves the worker finished without any sleep.
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let forward = tokio::spawn(async move {
            let _ = done_tx.send(handle.await);
        });
        entered_rx.recv().unwrap();
        drop(done_rx);
        let _ = release_tx.send(());
        forward.await.unwrap();
        // The worker published despite the dropped waiter: the registry
        // hands every later claim the identical endpoint.
        let first = match registry.claim(&session, workspace.clone()).unwrap() {
            Claim::Existing(endpoint) => endpoint,
            Claim::Create(_) => panic!("preparation published without a waiter"),
        };
        let second = match registry.claim(&session, workspace.clone()).unwrap() {
            Claim::Existing(endpoint) => endpoint,
            Claim::Create(_) => panic!("preparation published without a waiter"),
        };
        assert!(Arc::ptr_eq(&first, &second));
    }

    /// A real controller, catalog and manager: two cold Sessions recorded
    /// against one canonical workspace, durable only — never loaded.
    async fn fixture(count: usize) -> (tempfile::TempDir, SessionRuntimeManager, Vec<SessionId>) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(home.join("rustx")).unwrap();
        let runtime = root.path().join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        let controller = SessionController::open(&runtime).unwrap();
        let workspace_dir = root.path().join("workspace");
        std::fs::create_dir(&workspace_dir).unwrap();
        let workspace = workspace_dir.canonicalize().unwrap();
        let mut sessions = Vec::new();
        for _ in 0..count {
            sessions.push(
                controller
                    .create_session(SessionPersistentState::from_input(
                        &SessionConfigInput::new(workspace.clone()),
                    ))
                    .await
                    .unwrap()
                    .session
                    .id,
            );
        }
        let manager = SessionRuntimeManager::new(
            controller,
            UserConfigManager::new(UserConfigSources {
                home_directory: home.clone(),
                config_path: home.join("rustx").join("rustx.toml"),
                runtime_root: runtime,
            })
            .unwrap(),
            CredentialSnapshot::default(),
            LocalRuntimeDependencies::default(),
            super::super::RuntimeResidencyPolicy {
                max_resident_runtimes: 8,
                idle_grace_ms: 300_000,
            },
        )
        .unwrap();
        (root, manager, sessions)
    }

    #[tokio::test]
    async fn revocation_denies_the_live_connection_and_draining_rejects_new_claims() {
        let (_root, manager, sessions) = fixture(2).await;
        let endpoint = manager.reserve_local_execution(&sessions[0]).await.unwrap();
        let published = bundle(&endpoint);
        let mut client = std::os::unix::net::UnixStream::connect(&published.socket_path).unwrap();
        let hello = Hello {
            version: PROTOCOL_VERSION,
            session: published.session.clone(),
            environment: published.environment.clone(),
            instance: published.instance.clone(),
            generation: published.generation,
            workspace: published.workspace.clone(),
            capabilities: BTreeSet::from([Capability::Connection]),
            credential: published.credential.clone(),
        };
        let (accepted, _) = endpoint.listener().accept().unwrap();
        write_record(&mut client, &hello).unwrap();
        let mut connection = endpoint.binding().accept_local(accepted).unwrap().unwrap();
        assert!(matches!(
            read_record::<HelloReply>(&mut client).unwrap(),
            HelloReply::Ok(_)
        ));
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Live);
        manager.execution.fence(&sessions[0]);
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Revoked);
        // The typed gate denies before the wire: the client socket carries
        // only the earlier admission reply, nothing new.
        let error = connection.ping().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        client.set_nonblocking(true).unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(
            client.read(&mut byte).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        // Draining permanently closes claims for Sessions that were never
        // bound — the permanent gate, not endpoint status, refuses them.
        manager.begin_draining();
        match manager.reserve_local_execution(&sessions[1]).await {
            Err(RuntimeManagerError::Draining) => {}
            other => panic!("draining rejects a fresh claim: {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_manager_claims_cold_catalog_metadata_and_delete_revokes() {
        let (_root, manager, sessions) = fixture(2).await;
        let endpoint = manager.reserve_local_execution(&sessions[0]).await.unwrap();
        let published = bundle(&endpoint);
        assert_eq!(published.session.as_str(), sessions[0].as_str());
        assert_eq!(published.generation.get(), 1);
        let again = manager.reserve_local_execution(&sessions[0]).await.unwrap();
        assert!(Arc::ptr_eq(&again, &endpoint));
        // A stale user-supplied revision is rejected eagerly: no admission
        // fence and no endpoint state is created before it.
        let SessionDeleteResult::Stale {
            actual_revision, ..
        } = manager
            .delete_session(&sessions[0], "not-the-revision")
            .await
            .unwrap()
        else {
            panic!("a wrong revision is stale before any fencing")
        };
        assert!(Arc::ptr_eq(
            &manager.reserve_local_execution(&sessions[0]).await.unwrap(),
            &endpoint
        ));
        let SessionDeleteResult::Preview { preview } =
            manager.sessions.delete_preview(&sessions[0]).await
        else {
            panic!("the cold Session is deletable")
        };
        let result = manager
            .delete_session(&sessions[0], &preview.target_revision)
            .await
            .unwrap();
        assert!(matches!(result, SessionDeleteResult::Deleted { .. }));
        assert_eq!(preview.target_revision, actual_revision);
        assert_eq!(endpoint.binding().status().phase, BindingPhase::Revoked);
        assert!(
            manager.reserve_local_execution(&sessions[0]).await.is_err(),
            "a deleted Session can never reclaim an execution binding"
        );
        // The retired slot dropped its own endpoint reference; once the
        // test's handles are gone the private directory is gone too.
        drop(again);
        drop(endpoint);
        assert!(
            !std::path::Path::new(&published.socket_path).exists(),
            "the retired endpoint's private directory was removed"
        );
    }
}
