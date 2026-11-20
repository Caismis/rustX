//! Shared real Session-execution test rig.
//!
//! One compiled source serves both boundaries: the root crate includes it
//! as a `pub(crate)` `#[path]` module for its unit tests, and each
//! integration test crate includes it under `tests/common`. It builds the
//! exact production route an Agent-side consumer relies on — a real
//! [`SessionRuntimeManager`], a real catalog Session, a real reserved
//! local execution endpoint, and a real staged `rustx-execution-client`
//! child process admitted over the authenticated Unix-socket handshake.
//! There is no injectable executor and no in-process fallback: a read
//! dispatched through the returned scope crosses the process boundary.
//!
//! [`ExecutionFixture::new`] is synchronous because callers (the native
//! Read unit harness, `NativeFixture`-style integration fixtures) are
//! themselves synchronous constructors used inside async tests. The
//! runtime work happens on a dedicated thread running a fresh
//! current-thread Tokio runtime, so nothing ambient is borrowed and no
//! outer runtime flavor matters.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rustx::credentials::CredentialSnapshot;
use rustx::local_runtime::composition::LocalRuntimeDependencies;
use rustx::local_runtime::configuration::{
    SessionConfigInput, UserConfigManager, UserConfigSources,
};
use rustx::local_runtime::session::SessionPersistentState;
use rustx::local_runtime::session_controller::SessionController;
use rustx::local_runtime::session_runtime_manager::{
    RuntimeResidencyPolicy, SessionExecutionScope, SessionRuntimeManager,
};
use rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT;

/// The staged `rustx-execution-client` sibling this test binary was
/// prepared with — the same real binary deployment the production
/// bootstrap validates.
fn staged_client() -> PathBuf {
    let path = std::env::current_exe()
        .expect("test binary path")
        .parent()
        .expect("binary directory")
        .join("rustx-execution-client");
    assert!(
        path.is_file(),
        "staged rustx-execution-client missing beside the test binary at \
         {path:?} — run test-support/prepare_native_tests.py"
    );
    path
}

/// One live Session execution route: the Session's reserved endpoint,
/// the admitted real Execution Client child, and the non-owning scope an
/// Agent-side consumer dispatches through. The manager and private
/// runtime root stay owned here for the fixture's lifetime; dropping
/// kills and reaps the child.
pub struct ExecutionFixture {
    /// The non-owning Session execution route tests bind into a
    /// `Workspace` or a `ConversationRuntimeConfig`.
    pub scope: SessionExecutionScope,
    /// The admitted Execution Client child, wrapped in its kill/reap
    /// guard from the moment `spawn` returns so a mid-setup panic can
    /// never leave a running Client behind.
    _child: ChildGuard,
    /// The owning manager: dropping it unclaims the endpoint claim, which
    /// is also why the scope's `Weak` never outlives the rig.
    _manager: SessionRuntimeManager,
    /// The private runtime/home root the controller and manager state
    /// live under, declared last so handles drop first.
    _temporary: tempfile::TempDir,
}

/// Kills and reaps the child on drop, unconditionally: an orderly exit
/// that already happened makes `kill` a no-op error and `wait` reaps
/// either way.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl ExecutionFixture {
    /// Builds one real Session execution route for `workspace`: a private
    /// controller/home root, one catalog Session for the canonical
    /// workspace, one reserved endpoint, and one staged Execution Client
    /// admitted within the production handshake bound.
    ///
    /// # Panics
    ///
    /// Panics when `workspace` does not exist or cannot be canonicalized
    /// — the Session catalog records the canonical spelling (macOS
    /// temporary roots traverse a `/var` symlink, so the spelling the
    /// fixture receives is not always the one the binding must reserve).
    #[must_use]
    pub fn new(workspace: &Path) -> Self {
        let workspace = std::fs::canonicalize(workspace).expect("fixture workspace root");
        std::thread::Builder::new()
            .name("execution-fixture".to_owned())
            .spawn(move || Self::setup(&workspace))
            .expect("execution fixture setup thread")
            .join()
            .expect("execution fixture setup panicked")
    }

    /// The async setup, run on a dedicated current-thread runtime so the
    /// synchronous constructor stays usable inside any async test.
    fn setup(workspace: &Path) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fixture runtime");
        runtime.block_on(async {
            let temporary = tempfile::tempdir().expect("private execution runtime root");
            let runtime_root = temporary.path().join("runtime");
            let home = temporary.path().join("home");
            std::fs::create_dir_all(&runtime_root).expect("runtime root");
            std::fs::create_dir_all(home.join("rustx")).expect("home root");
            let controller = SessionController::open(&runtime_root).expect("session controller");
            let user_config = UserConfigManager::new(UserConfigSources {
                home_directory: home.clone(),
                config_path: home.join("rustx").join("rustx.toml"),
                runtime_root: runtime_root.clone(),
            })
            .expect("user configuration");
            let session = controller
                .create_session(SessionPersistentState::from_input(
                    &SessionConfigInput::new(workspace.to_path_buf()),
                ))
                .await
                .expect("catalog session")
                .session;
            let manager = SessionRuntimeManager::new(
                controller,
                user_config,
                CredentialSnapshot::default(),
                LocalRuntimeDependencies::default(),
                RuntimeResidencyPolicy {
                    max_resident_runtimes: Some(8),
                    idle_grace_ms: 300_000,
                },
            )
            .expect("session runtime manager");
            let endpoint = manager
                .reserve_local_execution(&session.id)
                .await
                .expect("reserved local execution");
            let mut child = ChildGuard(
                Command::new(staged_client())
                    .env_clear()
                    .current_dir("/")
                    .arg("--binding-file")
                    .arg(endpoint.bootstrap_path())
                    .arg("--workspace")
                    .arg(workspace)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("staged execution client spawns"),
            );

            // Bounded connect: the listener accepts nonblocking inside the
            // production handshake window, and an early Client exit fails
            // fast instead of waiting out the bound.
            let rustx::local_runtime::session_runtime_manager::ExecutionListener::Unix(listener) =
                endpoint.listener()
            else {
                panic!("the reserved local endpoint is a Unix listener");
            };
            listener
                .set_nonblocking(true)
                .expect("nonblocking listener");
            // The paired handshake: the data lane's `Pending` keeps
            // accepting for the mandatory control lane until the pair
            // commits `Live` — one shared deadline over both halves.
            let deadline = Instant::now() + LOCAL_HANDSHAKE_TIMEOUT;
            loop {
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            if let Some(status) = child.0.try_wait().expect("client status") {
                                panic!("execution client exited before admit: {status}");
                            }
                            assert!(
                                Instant::now() < deadline,
                                "execution client did not connect within {LOCAL_HANDSHAKE_TIMEOUT:?}"
                            );
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("execution listener accept failed: {error}"),
                    }
                };
                match endpoint
                    .admit_local_stream_deadline(stream, deadline)
                    .expect("admit exchange")
                {
                    rustx::local_runtime::session_runtime_manager::EndpointAdmission::Live => {
                        break;
                    }
                    rustx::local_runtime::session_runtime_manager::EndpointAdmission::Pending => {}
                    rustx::local_runtime::session_runtime_manager::EndpointAdmission::Rejected(
                        reject,
                    ) => panic!("execution client rejected: {reject:?}"),
                }
            }

            let scope = manager.execution_scope(&session.id);
            Self {
                scope,
                _child: child,
                _manager: manager,
                _temporary: temporary,
            }
        })
    }
}
