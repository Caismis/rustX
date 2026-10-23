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
                    std::fs::write(interpreter, b"interpreter stub").expect("interpreter");
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
}

impl PreparationFixture {
    fn new(names: &[&str]) -> Self {
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
        let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
            source_demand: demand.clone(),
            conversation_id: ConversationId::new("conv_498fb65d-10d6-70e1-b247-5f70bf426e8b"),
            workspace: workspace.clone(),
            base_tool_registry: Arc::new(ToolRegistry::new()),
            extension_tools: crate::extensions::ExtensionToolPlane::none(),
            agent_activation: crate::capabilities::AgentActivation::default(),
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
        });
        coordinator.install_python_store(
            PythonToolStore::with_binaries_and_runner(
                directory.path().join("python-tools"),
                PathBuf::from("/fixture/uv"),
                PathBuf::from("/fixture/python3"),
                runner.clone(),
            )
            .expect("recorded Python store"),
        );
        Self {
            _directory: directory,
            coordinator,
            demand,
            runner,
            events: receiver,
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
async fn python_build_slots_are_shared_and_retained_until_cancellation_settles() {
    let mut fixture = PreparationFixture::new(&["a", "b", "c", "d", "e", "z"]);
    let coordinator = fixture.coordinator.clone();
    let mut cancelled_demand = fixture.demand.clone();
    cancelled_demand
        .sources
        .remove(&ToolSourceId::ManagedPython("z".into()));
    let mut sibling_demand = fixture.demand.clone();
    sibling_demand.sources = [ToolSourceId::ManagedPython("z".into())].into();
    let cancellation = crate::runtime::cancellation::CancellationSignal::new();
    let mut cancelled_availability = CapabilityAvailability::new();
    let mut sibling_availability = CapabilityAvailability::new();
    let mut cancelled = Box::pin(coordinator.prepare_python_packages(
        &cancelled_demand,
        &mut cancelled_availability,
        Some(&cancellation),
        |_, binding| std::future::ready(binding),
    ));
    let mut sibling = Box::pin(coordinator.prepare_python_packages(
        &sibling_demand,
        &mut sibling_availability,
        None,
        |_, binding| std::future::ready(binding),
    ));
    assert!(cancelled.as_mut().now_or_never().is_none());
    assert!(sibling.as_mut().now_or_never().is_none());
    assert_eq!(fixture.runner.probe_calls.load(Ordering::SeqCst), 8);
    let mut entered = Vec::new();
    for _ in 0..4 {
        let BuildEvent::Lock(package) = fixture.next_event().await else {
            panic!("four source slots must reach their physical build gates");
        };
        entered.push(package);
    }
    entered.sort();
    assert_eq!(entered, ["a", "b", "c", "d"]);

    cancellation.cancel();
    let mut cancelling = Vec::new();
    for _ in 0..4 {
        let BuildEvent::CancellationRequested(package) = fixture.next_event().await else {
            panic!("cancellation requests cannot publish unsettled physical builds");
        };
        cancelling.push(package);
    }
    cancelling.sort();
    assert_eq!(cancelling, entered);
    assert!(cancelled.as_mut().now_or_never().is_none());
    assert!(sibling.as_mut().now_or_never().is_none());
    assert_eq!(fixture.runner.active_locks.load(Ordering::SeqCst), 4);
    assert_eq!(
        fixture.runner.probe_calls.load(Ordering::SeqCst),
        8,
        "neither preparation can admit a fifth source before physical settlement"
    );

    fixture.runner.settlements["a"].add_permits(1);
    assert_eq!(fixture.next_event().await, BuildEvent::Settled("a".into()));
    assert!(cancelled.as_mut().now_or_never().is_none());
    assert!(sibling.as_mut().now_or_never().is_none());
    assert_eq!(fixture.next_event().await, BuildEvent::Lock("z".into()));
    assert_eq!(fixture.runner.probe_calls.load(Ordering::SeqCst), 10);
    assert_eq!(fixture.runner.active_locks.load(Ordering::SeqCst), 4);

    for package in ["b", "c", "d"] {
        fixture.runner.settlements[package].add_permits(1);
    }
    fixture.runner.releases["z"].add_permits(1);
    let (cancelled, sibling) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(cancelled, sibling)
    })
    .await
    .expect("all physical builds settle before source permits are released");
    assert_eq!(cancelled.len(), 5);
    assert!(cancelled.iter().all(|(_, outcome)| outcome.is_err()));
    assert_eq!(
        cancelled[4].1.as_ref().unwrap_err(),
        "Python package preparation cancelled",
        "the queued cancelled package never starts runtime probes"
    );
    assert_eq!(sibling.len(), 1);
    assert!(sibling[0].1.is_ok());
    assert_eq!(fixture.runner.probe_calls.load(Ordering::SeqCst), 10);
    assert_eq!(fixture.runner.lock_calls.load(Ordering::SeqCst), 5);
    assert_eq!(fixture.runner.sync_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.runner.peak_locks.load(Ordering::SeqCst), 4);
    assert_eq!(fixture.runner.active_locks.load(Ordering::SeqCst), 0);
}
