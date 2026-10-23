//! Managed Python source concurrency through the coordinator's real store owner.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::FutureExt;
use tokio::sync::{Semaphore, mpsc};

use super::{CapabilityCoordinator, CapabilityCoordinatorConfig};
use crate::capabilities::availability::CapabilityAvailability;
use crate::capabilities::source::{ToolSourceDemand, ToolSourceId};
use crate::runtime::identity::ConversationId;
use crate::runtime::process_runner::{
    CapturedProcessResult, ProcessOutcomeIntent, RunnerTestControl, SupervisedCommandSpec,
    SupervisedProcessRunner,
};
use crate::runtime::resources::ManagedPythonCatalog;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::ToolRegistry;
use crate::tools::python::PythonToolStore;
use crate::tools::workspace::Workspace;

#[derive(Debug, PartialEq, Eq)]
enum BuildEvent {
    Lock(String),
    Sync(String),
    CancellationRequested(String),
    Settled(String),
}

/// Locks remain pending until explicitly released. Runtime identity probes
/// complete immediately, so polling two callers proves both reached the
/// fingerprint coalescing boundary before the physical owner can publish.
struct BuildRunner {
    events: mpsc::UnboundedSender<BuildEvent>,
    releases: BTreeMap<String, Semaphore>,
    settlements: BTreeMap<String, Semaphore>,
    probe_calls: AtomicUsize,
    lock_calls: AtomicUsize,
    sync_calls: AtomicUsize,
    active_locks: AtomicUsize,
    peak_locks: AtomicUsize,
    interpreter_script: Option<String>,
}

impl SupervisedProcessRunner for BuildRunner {
    fn run(
        &self,
        spec: SupervisedCommandSpec,
        _control: Option<RunnerTestControl>,
    ) -> futures_util::future::BoxFuture<'_, Result<CapturedProcessResult, String>> {
        Box::pin(async move {
            let mut intent = ProcessOutcomeIntent::Completed;
            let stdout = if spec.command.contains("--version") {
                self.probe_calls.fetch_add(1, Ordering::SeqCst);
                if spec.command.contains("python3") {
                    b"Python 3.12.13\n".to_vec()
                } else {
                    b"uv 0.11.12\n".to_vec()
                }
            } else {
                let project: toml::Value = toml::from_str(
                    &std::fs::read_to_string(spec.cwd.join("pyproject.toml"))
                        .expect("generated project"),
                )
                .expect("project TOML");
                let package = project["project"]["name"]
                    .as_str()
                    .expect("project name")
                    .strip_prefix("rustx-python-tool-")
                    .expect("managed project")
                    .to_owned();
                if spec.command.contains(" lock ") {
                    self.lock_calls.fetch_add(1, Ordering::SeqCst);
                    let active = self.active_locks.fetch_add(1, Ordering::SeqCst) + 1;
                    self.peak_locks.fetch_max(active, Ordering::SeqCst);
                    self.events
                        .send(BuildEvent::Lock(package.clone()))
                        .expect("lock event");
                    tokio::select! {
                        biased;
                        () = spec.cancellation.cancelled() => {
                            self.events
                                .send(BuildEvent::CancellationRequested(package.clone()))
                                .expect("cancellation request");
                            self.settlements[&package]
                                .acquire()
                                .await
                                .expect("physical settlement")
                                .forget();
                            intent = ProcessOutcomeIntent::Cancelled;
                            self.events
                                .send(BuildEvent::Settled(package.clone()))
                                .expect("settlement event");
                        }
                        permit = self.releases[&package].acquire() => {
                            permit.expect("release lock").forget();
                        }
                    }
                    self.active_locks.fetch_sub(1, Ordering::SeqCst);
                } else {
                    assert!(spec.command.contains(" sync "));
                    self.sync_calls.fetch_add(1, Ordering::SeqCst);
                    let interpreter = spec.cwd.join("venv").join(if cfg!(windows) {
                        "Scripts/python.exe"
                    } else {
                        "bin/python"
                    });
                    std::fs::create_dir_all(interpreter.parent().expect("interpreter parent"))
                        .expect("venv directory");
                    std::fs::write(
                        &interpreter,
                        self.interpreter_script
                            .as_deref()
                            .unwrap_or("interpreter stub"),
                    )
                    .expect("interpreter");
                    #[cfg(unix)]
                    if self.interpreter_script.is_some() {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(
                            &interpreter,
                            std::fs::Permissions::from_mode(0o755),
                        )
                        .expect("executable fixture interpreter");
                    }
                    self.events
                        .send(BuildEvent::Sync(package))
                        .expect("sync event");
                }
                Vec::new()
            };
            Ok(CapturedProcessResult {
                exit_code: Some(0),
                intent,
                stdout,
                stderr: Vec::new(),
            })
        })
    }
}

struct PreparationFixture {
    _directory: tempfile::TempDir,
    coordinator: CapabilityCoordinator,
    demand: ToolSourceDemand,
    runner: Arc<BuildRunner>,
    events: mpsc::UnboundedReceiver<BuildEvent>,
    store: PythonToolStore,
}

impl PreparationFixture {
    fn new(names: &[&str]) -> Self {
        Self::with_interpreter(names, None)
    }

    fn with_interpreter(names: &[&str], interpreter_script: Option<String>) -> Self {
        let directory = tempfile::tempdir().expect("fixture directory");
        let workspace_root = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let mut packages = BTreeMap::new();
        for name in names {
            let root = workspace_root.join(".agents/tools").join(name);
            std::fs::create_dir_all(&root).expect("package directory");
            std::fs::write(root.join("server.py"), format!("# {name}\n")).expect("frozen source");
            std::fs::write(root.join("requirements.txt"), b"").expect("requirements");
            packages.insert(
                ToolSourceId::ManagedPython((*name).to_owned()),
                crate::tools::python::discover_package(&root, name),
            );
        }
        let demand = ToolSourceDemand::new(
            packages.keys().cloned().collect::<Vec<_>>(),
            ManagedPythonCatalog::new(packages),
        );
        let workspace = Workspace::new(&workspace_root).expect("workspace");
        let mut activation = crate::capabilities::AgentActivation::default();
        activation.profile.tools.sources = demand
            .sources
            .iter()
            .cloned()
            .map(|source| {
                (
                    source,
                    crate::capabilities::selection::SourceToolSelection::All,
                )
            })
            .collect();
        let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            source_demand: demand.clone(),
            conversation_id: ConversationId::new("conv_498fb65d-10d6-70e1-b247-5f70bf426e8b"),
            workspace: workspace.clone(),
            base_tool_registry: Arc::new(ToolRegistry::new()),
            extension_tools: crate::extensions::ExtensionToolPlane::none(),
            agent_activation: activation,
            skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                workspace.root().join(".agents/skills"),
            ),
            mcp_servers: BTreeMap::new(),
            base_environment: ToolEnvironment::new(),
            environment_store_root: directory.path().join("environments"),
        })
        .expect("coordinator");
        let (events, receiver) = mpsc::unbounded_channel();
        let runner = Arc::new(BuildRunner {
            events,
            releases: names
                .iter()
                .map(|name| ((*name).to_owned(), Semaphore::new(0)))
                .collect(),
            settlements: names
                .iter()
                .map(|name| ((*name).to_owned(), Semaphore::new(0)))
                .collect(),
            probe_calls: AtomicUsize::new(0),
            lock_calls: AtomicUsize::new(0),
            sync_calls: AtomicUsize::new(0),
            active_locks: AtomicUsize::new(0),
            peak_locks: AtomicUsize::new(0),
            interpreter_script,
        });
        let store = PythonToolStore::with_binaries_and_runner(
            directory.path().join("python-tools"),
            PathBuf::from("/fixture/uv"),
            PathBuf::from("/fixture/python3"),
            runner.clone(),
        )
        .expect("recorded Python store");
        coordinator.install_python_store(store.clone());
        Self {
            _directory: directory,
            coordinator,
            demand,
            runner,
            events: receiver,
            store,
        }
    }

    async fn next_event(&mut self) -> BuildEvent {
        tokio::time::timeout(Duration::from_secs(10), self.events.recv())
            .await
            .expect("Python preparation must make progress without releasing a sibling")
            .expect("build event")
    }
}

#[tokio::test]
async fn distinct_python_packages_overlap_and_return_in_source_order() {
    let mut fixture = PreparationFixture::new(&["beta", "alpha"]);
    let coordinator = fixture.coordinator.clone();
    let demand = fixture.demand.clone();
    let (ready, mut ready_packages) = tokio::sync::mpsc::unbounded_channel();
    let preparation = tokio::spawn(async move {
        coordinator
            .prepare_python_packages(
                &demand,
                &mut CapabilityAvailability::new(),
                None,
                |server, binding| {
                    ready.send(server).unwrap();
                    std::future::ready(binding)
                },
            )
            .await
    });

    let mut entered = Vec::new();
    for _ in 0..2 {
        let BuildEvent::Lock(package) = fixture.next_event().await else {
            panic!("both package locks must enter before either is released");
        };
        entered.push(package);
    }
    entered.sort();
    assert_eq!(entered, ["alpha", "beta"]);
    fixture.runner.releases["beta"].add_permits(1);
    assert_eq!(fixture.next_event().await, BuildEvent::Sync("beta".into()));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), ready_packages.recv())
            .await
            .expect("a ready package enters its connection phase without waiting for alpha")
            .unwrap(),
        crate::tools::python::python_server_id("beta"),
    );
    assert!(
        !preparation.is_finished(),
        "alpha still owns its unfinished build"
    );
    fixture.runner.releases["alpha"].add_permits(1);
    let bindings = tokio::time::timeout(Duration::from_secs(10), preparation)
        .await
        .expect("both package builds settle")
        .expect("preparation task");
    assert!(bindings.iter().all(|(_, outcome)| outcome.is_ok()));
    assert_eq!(
        bindings
            .iter()
            .map(|(server, _)| server.as_str())
            .collect::<Vec<_>>(),
        ["python:alpha", "python:beta"],
        "completion order never becomes source order"
    );
    assert_eq!(fixture.runner.lock_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.runner.sync_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn overlapping_python_preparations_coalesce_identical_fingerprints() {
    let mut fixture = PreparationFixture::new(&["demo"]);
    let (joined, mut joins) = mpsc::unbounded_channel();
    fixture.store.install_build_join_observer(joined);
    let store_identity = fixture.coordinator.python_store_identity_token();
    let coordinator = fixture.coordinator.clone();
    let demand = fixture.demand.clone();
    let mut first_availability = CapabilityAvailability::new();
    let mut second_availability = CapabilityAvailability::new();
    let mut first = Box::pin(coordinator.prepare_python_packages(
        &demand,
        &mut first_availability,
        None,
        |_, binding| std::future::ready(binding),
    ));
    let mut second = Box::pin(coordinator.prepare_python_packages(
        &demand,
        &mut second_availability,
        None,
        |_, binding| std::future::ready(binding),
    ));
    assert!(first.as_mut().now_or_never().is_none());
    assert!(second.as_mut().now_or_never().is_none());
    assert!(
        joins
            .try_recv()
            .expect("second caller joined the build")
            .starts_with("sha256:")
    );
    assert_eq!(
        fixture.runner.probe_calls.load(Ordering::SeqCst),
        4,
        "both callers reached the fingerprint boundary before the build can publish"
    );
    assert_eq!(fixture.next_event().await, BuildEvent::Lock("demo".into()));
    assert_eq!(fixture.runner.lock_calls.load(Ordering::SeqCst), 1);
    fixture.runner.releases["demo"].add_permits(1);
    let (first, second) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(first, second)
    })
    .await
    .expect("both callers observe the physical build's publication");
    assert_eq!(
        first, second,
        "both callers observe one prepared launch binding"
    );
    assert!(first[0].1.is_ok());
    assert_eq!(fixture.runner.lock_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.runner.sync_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.coordinator.python_store_identity_token(),
        store_identity
    );
}

#[tokio::test]
async fn coordinator_cancellation_retains_python_build_slots_until_physical_settlement() {
    let mut fixture = PreparationFixture::new(&["a", "b", "c", "d", "e"]);
    let coordinator = fixture.coordinator.clone();
    let lifecycle = crate::runtime::types::ConversationLifecycle::new();
    let _publication = coordinator
        .claim_conversation_runtime(&lifecycle)
        .expect("runtime claim");
    assert!(lifecycle.activate());
    let preparation = tokio::spawn(async move { coordinator.prepare_candidate().await });
    let mut entered = Vec::new();
    for _ in 0..4 {
        let BuildEvent::Lock(package) = fixture.next_event().await else {
            panic!("four source slots must reach their physical build gates");
        };
        entered.push(package);
    }
    entered.sort();
    assert_eq!(entered, ["a", "b", "c", "d"]);

    assert!(lifecycle.begin_drain());
    fixture.coordinator.cancel_conversation_preparation();
    let mut cancelling = Vec::new();
    for _ in 0..4 {
        let BuildEvent::CancellationRequested(package) = fixture.next_event().await else {
            panic!("cancellation requests cannot publish unsettled physical builds");
        };
        cancelling.push(package);
    }
    cancelling.sort();
    assert_eq!(cancelling, entered);
    assert!(!preparation.is_finished());
    assert!(
        !lifecycle.mark_quiescent(),
        "physical builds retain preparation admission"
    );
    assert_eq!(
        fixture
            .coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        0
    );
    assert_eq!(fixture.runner.active_locks.load(Ordering::SeqCst), 4);
    assert_eq!(
        fixture.runner.probe_calls.load(Ordering::SeqCst),
        8,
        "the queued fifth source cannot start during coordinator cancellation"
    );

    for package in ["a", "b", "c", "d"] {
        fixture.runner.settlements[package].add_permits(1);
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(10), preparation)
            .await
            .expect("all physical builds settle before source permits are released")
            .expect("preparation task")
            .is_err()
    );
    assert_eq!(fixture.runner.probe_calls.load(Ordering::SeqCst), 8);
    assert_eq!(fixture.runner.lock_calls.load(Ordering::SeqCst), 4);
    assert_eq!(fixture.runner.sync_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.runner.peak_locks.load(Ordering::SeqCst), 4);
    assert_eq!(fixture.runner.active_locks.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture
            .coordinator
            .inner
            .source_preparation_slots
            .available_permits(),
        4
    );
    assert_eq!(
        fixture
            .coordinator
            .inner
            .mcp_preparations
            .load(Ordering::Relaxed),
        0
    );
    assert!(
        lifecycle.mark_quiescent(),
        "global cancellation waits for physical settlement"
    );
}

#[cfg(all(unix, feature = "mcp-fixture"))]
fn fixture_interpreter(entrypoint: &str) -> String {
    use crate::tools::mcp::fixture::{FIXTURE_MODE_ENV, fixture_spawn_args};

    let command = std::iter::once(
        std::env::current_exe()
            .expect("test executable")
            .display()
            .to_string(),
    )
    .chain(fixture_spawn_args(entrypoint))
    .map(|argument| format!("'{}'", argument.replace('\'', "'\\''")))
    .collect::<Vec<_>>()
    .join(" ");
    format!("#!/bin/sh\nexport {FIXTURE_MODE_ENV}=1\nexec {command}\n")
}

#[cfg(all(unix, feature = "mcp-fixture"))]
#[tokio::test]
async fn dropping_one_candidate_preserves_another_candidates_shared_python_build() {
    use crate::tools::mcp::fixture::{FixtureServer, serve_if_fixture_mode};

    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let interpreter = fixture_interpreter(
        "capabilities::coordinator::python_preparation_tests::dropping_one_candidate_preserves_another_candidates_shared_python_build",
    );
    let mut fixture = PreparationFixture::with_interpreter(&["demo"], Some(interpreter));
    let (joined, mut joins) = mpsc::unbounded_channel();
    fixture.store.install_build_join_observer(joined);
    let first_coordinator = fixture.coordinator.clone();
    let first = tokio::spawn(async move { first_coordinator.prepare_candidate().await });
    assert_eq!(fixture.next_event().await, BuildEvent::Lock("demo".into()));

    let second_coordinator = fixture.coordinator.clone();
    let second = tokio::spawn(async move { second_coordinator.prepare_candidate().await });
    assert!(
        tokio::time::timeout(Duration::from_secs(10), joins.recv())
            .await
            .expect("second candidate reaches the shared build")
            .expect("joined fingerprint")
            .starts_with("sha256:")
    );
    assert_eq!(fixture.runner.probe_calls.load(Ordering::SeqCst), 4);
    first.abort();
    assert!(
        first
            .await
            .expect_err("first candidate waiter was dropped")
            .is_cancelled()
    );
    fixture.runner.releases["demo"].add_permits(1);
    assert_eq!(fixture.next_event().await, BuildEvent::Sync("demo".into()));
    let candidate = tokio::time::timeout(Duration::from_secs(10), second)
        .await
        .expect("the surviving candidate completes")
        .expect("second candidate task")
        .expect("the surviving candidate prepares successfully");
    assert_eq!(
        candidate
            .availability()
            .get(&ToolSourceId::ManagedPython("demo".into())),
        Some(&crate::capabilities::CapabilitySourceState::Ready)
    );
    assert!(
        candidate
            .candidate_registry
            .definitions()
            .iter()
            .any(|tool| tool.name == "echo"
                && matches!(&tool.origin,
            crate::tools::types::ToolOrigin::ManagedPython { package } if package == "demo"))
    );
    let transport = candidate.mcp_runtimes[0]
        .connection()
        .acquire()
        .await
        .expect("live prepared source");
    let tools = transport
        .runtime()
        .list_tools()
        .await
        .expect("prepared source remains usable");
    assert!(tools.iter().any(|tool| tool.name == "echo"));
    drop(transport);
    assert_eq!(fixture.runner.lock_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.runner.sync_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.runner.active_locks.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture
            .coordinator
            .inner
            .mcp_preparations
            .load(Ordering::Relaxed),
        1,
        "the cancelled candidate never enters its private MCP continuation"
    );
    candidate.retire_uncommitted().await;
    fixture
        .coordinator
        .drain_conversation_owned()
        .await
        .expect("settled fixture source");
}
