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
use std::os::unix::{
    fs::{DirBuilderExt as _, PermissionsExt as _},
    process::CommandExt as _,
};
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
    LocalExecutionEndpoint, RuntimeResidencyPolicy, SessionExecutionScope, SessionRuntimeManager,
};
use rustx::runtime::identity::SessionId;
use rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT;

const MANAGED_STORE: &str = ".local/share/rustx/environments/python-tools";

pub(crate) fn seed_managed_python_uv(home: &Path, uv: &Path) -> PathBuf {
    let Some(warmed) = std::env::var_os("RUSTX_MANAGED_PYTHON_TEST_WHEEL_CACHE") else {
        return uv.to_path_buf();
    };
    // The Client still creates the project and materializes its own Store.
    // Supplying cached wheels lets the physical test run without PyPI.
    let cache = home.join(MANAGED_STORE).join("uv-cache");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&cache)
        .expect("private Store cache");
    let copy = Command::new("/bin/cp")
        .arg("-a")
        .arg(PathBuf::from(warmed).join("."))
        .arg(cache)
        .output()
        .expect("copy the prepared wheels");
    assert!(
        copy.status.success(),
        "{}",
        String::from_utf8_lossy(&copy.stderr)
    );

    let uv = uv.canonicalize().expect("canonical uv");
    let bin = home.join("bin");
    std::fs::create_dir(&bin).expect("private fixture binaries");
    let wrapper = bin.join("uv");
    let uv = uv.to_str().expect("UTF-8 uv path").replace('\'', "'\\''");
    std::fs::write(
        &wrapper,
        format!("#!/bin/sh\nexec '{uv}' --offline \"$@\"\n"),
    )
    .expect("offline uv launcher");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700))
        .expect("executable uv launcher");
    wrapper
}

fn managed_python_path(home: &Path) -> std::ffi::OsString {
    let path = std::env::var_os("PATH").expect("the test runner has a PATH");
    if std::env::var_os("RUSTX_MANAGED_PYTHON_TEST_WHEEL_CACHE").is_none() {
        return path;
    }
    let uv = std::env::split_paths(&path)
        .map(|directory| directory.join("uv"))
        .find(|binary| binary.is_file())
        .expect("the test runner has uv");
    let uv = seed_managed_python_uv(home, &uv);
    std::env::join_paths(
        std::iter::once(uv.parent().expect("uv directory").to_path_buf())
            .chain(std::env::split_paths(&path)),
    )
    .expect("Client fixture PATH")
}

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
    #[allow(dead_code)] // Managed Python integration tests use the exact bound source owner.
    endpoint: std::sync::Arc<LocalExecutionEndpoint>,
    /// The admitted Execution Client child, wrapped in its kill/reap
    /// guard from the moment `spawn` returns so a mid-setup panic can
    /// never leave a running Client behind.
    child: ChildGuard,
    /// The owning manager: dropping it unclaims the endpoint claim, which
    /// is also why the scope's `Weak` never outlives the rig.
    manager: SessionRuntimeManager,
    /// The private runtime/home root the controller and manager state
    /// live under, declared last so handles drop first.
    temporary: tempfile::TempDir,
    _mount_parent: Option<MountParent>,
}

/// Bubblewrap's parent-death signal follows the thread that spawned it.
/// Retain that thread until the fixture has killed and reaped its Client.
struct MountParent {
    lifetime: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for MountParent {
    fn drop(&mut self) {
        drop(self.lifetime.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
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
        Self::with_mount_view(workspace, None)
    }

    /// Runs the real Client with a private Linux bind mount over the logical workspace.
    #[must_use]
    pub fn with_mount_view(workspace: &Path, physical: Option<&Path>) -> Self {
        let workspace = std::fs::canonicalize(workspace).expect("fixture workspace root");
        let setup = std::thread::Builder::new().name("execution-fixture".to_owned());
        let Some(physical) = physical.map(Path::to_path_buf) else {
            return setup
                .spawn(move || Self::setup(&workspace, None, None, false))
                .expect("execution fixture setup thread")
                .join()
                .expect("execution fixture setup panicked");
        };
        let (ready, fixture) = std::sync::mpsc::sync_channel(0);
        let (lifetime, release) = std::sync::mpsc::channel::<()>();
        let thread = setup
            .spawn(move || {
                if ready
                    .send(Self::setup(&workspace, Some(&physical), None, false))
                    .is_ok()
                {
                    let _ = release.recv();
                }
            })
            .expect("mount fixture setup thread");
        let fixture = fixture.recv().expect("mount fixture setup panicked");
        Self {
            _mount_parent: Some(MountParent {
                lifetime: Some(lifetime),
                thread: Some(thread),
            }),
            ..fixture
        }
    }

    /// Binds the real Client to an existing catalog Session.
    #[allow(dead_code)] // used by the in-crate process-death fixtures
    pub(crate) fn for_session(
        workspace: &Path,
        controller: SessionController,
        session: SessionId,
    ) -> Self {
        let workspace = workspace.canonicalize().expect("fixture workspace root");
        std::thread::Builder::new()
            .name("execution-fixture".to_owned())
            .spawn(move || Self::setup(&workspace, None, Some((controller, session)), false))
            .expect("execution fixture setup thread")
            .join()
            .expect("execution fixture setup panicked")
    }

    /// Provision a private deployment home for a real Python Store and uv.
    #[allow(dead_code)] // used by the tools integration harness
    pub fn for_managed_python(workspace: &Path) -> Self {
        let workspace = workspace.canonicalize().expect("fixture workspace root");
        std::thread::spawn(move || Self::setup(&workspace, None, None, true))
            .join()
            .unwrap()
    }

    #[allow(dead_code)] // used by the tools integration harness
    pub async fn managed_demand(
        &self,
        sources: impl IntoIterator<Item = impl Into<String>>,
    ) -> rustx::capabilities::source::ToolSourceDemand {
        let endpoint = self.endpoint.clone();
        let user = self.temporary.path().join("home/rustx/.agents");
        let catalog = tokio::task::spawn_blocking(move || {
            rustx::local_runtime::managed_python_resources::discover_bound(
                &endpoint,
                &user,
                &rustx::runtime::CancellationSignal::new(),
            )
        })
        .await
        .unwrap()
        .unwrap();
        rustx::capabilities::source::ToolSourceDemand::new(
            sources
                .into_iter()
                .map(|source| rustx::capabilities::ToolSourceId::try_from(source.into()).unwrap()),
            catalog,
        )
    }

    #[allow(dead_code)] // used by the tools integration harness
    pub fn managed_store(&self) -> PathBuf {
        self.temporary.path().join("home").join(MANAGED_STORE)
    }

    #[allow(dead_code)] // used by the native Managed Python boundary suites
    pub async fn settle(mut self) {
        let host = rustx::app_server::host::AppServerHost::new(
            self.manager.clone(),
            rustx::local_runtime::app_server_policy::AppServerPolicy::default(),
        );
        let failures = host.drain().await;
        assert!(failures.is_empty(), "Client settlement: {failures:?}");
        let status = tokio::task::spawn_blocking(move || self.child.0.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(
            status.success(),
            "Client exits cleanly after drain: {status}"
        );
    }

    /// The async setup, run on a dedicated current-thread runtime so the
    /// synchronous constructor stays usable inside any async test.
    fn setup(
        workspace: &Path,
        physical: Option<&Path>,
        session: Option<(SessionController, SessionId)>,
        prepare_python: bool,
    ) -> Self {
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
            let (controller, session) = if let Some(existing) = session {
                existing
            } else {
                let controller =
                    SessionController::open(&runtime_root).expect("session controller");
                let session = controller
                    .create_session(SessionPersistentState::from_input(
                        &SessionConfigInput::new(workspace.to_path_buf()),
                    ))
                    .await
                    .expect("catalog session")
                    .session;
                (controller, session.id)
            };
            let user_config = UserConfigManager::new(UserConfigSources {
                home_directory: home.clone(),
                config_path: home.join("rustx").join("rustx.toml"),
                runtime_root,
            })
            .expect("user configuration");
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
                .reserve_local_execution(&session)
                .await
                .expect("reserved local execution");
            let mut command = if let Some(physical) = physical {
                let scratch = temporary.path().join("scratch");
                std::fs::create_dir(&scratch).expect("Client scratch mount point");
                let mut command = Command::new("/usr/bin/bwrap");
                command.args(["--die-with-parent", "--ro-bind", "/", "/", "--dev", "/dev", "--bind"])
                    .arg(physical)
                    .arg(workspace)
                    .arg("--tmpfs").arg(&scratch)
                    .arg("--setenv").arg("TMPDIR").arg(&scratch)
                    .arg("--").arg(staged_client());
                command
            } else {
                Command::new(staged_client())
            };
            command.env_clear();
            if prepare_python {
                command.env("HOME", &home).env("PATH", managed_python_path(&home));
            }
            let mut child = ChildGuard(
                command
                    .current_dir("/")
                    .arg("--binding-file")
                    .arg(endpoint.bootstrap_path())
                    .arg("--workspace")
                    .arg(workspace)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .process_group(0)
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

            let scope = manager.execution_scope(&session);
            Self {
                scope,
                endpoint,
                child,
                manager,
                temporary,
                _mount_parent: None,
            }
        })
    }
}
