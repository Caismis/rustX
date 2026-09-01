//! Managed Python tool packages at the capability coordinator (Issue #174):
//! deterministic preparation-accounting contracts driven through the real
//! `CapabilityCoordinator` with a recorded process backend.
//!
//! The coordinator's Python store is installed with a scripted
//! `SupervisedProcessRunner` (the `install_python_store` test seam): runtime
//! probes and uv lock/sync commands are recorded, never executed. The
//! scripted `uv sync` writes a deliberately *non-executable* stub
//! interpreter, so the generic MCP connect of the synthesized binding fails
//! immediately at exec without any real process ever running — the
//! invariants under test are preparation accounting, state reuse, and
//! failure isolation. The real-wire contracts (one server per folder,
//! process reuse, framing, startup diagnosis) are owned by the
//! `tests/tools/mcp_managed.rs` boundary suite with real `FastMCP` children.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use rustx::capabilities::{
    CapabilityCoordinator, CapabilityCoordinatorConfig, CapabilitySourceId, CapabilitySourceState,
};
use rustx::runtime::identity::ConversationId;
use rustx::tools::environment::ToolEnvironment;
use rustx::tools::executor::ToolRegistry;
use rustx::tools::python::python_server_id;
use rustx::tools::workspace::Workspace;

use super::super::common;

/// The scripted process backend: runtime probes answer fixed versions, `uv
/// lock` succeeds unless a failure is injected, and `uv sync` writes a
/// non-executable stub into the staging venv so the later MCP connect fails
/// fast (EACCES) with no real process. Every command is recorded.
#[derive(Debug, Default)]
struct RecordedRunner {
    commands: Mutex<Vec<String>>,
    fail_lock: AtomicBool,
}

impl RecordedRunner {
    fn lock_command_count(&self) -> usize {
        self.commands
            .lock()
            .expect("commands")
            .iter()
            .filter(|command| command.contains(" lock "))
            .count()
    }

    fn fail_builds(&self) {
        self.fail_lock.store(true, Ordering::Release);
    }

    fn stop_failing_builds(&self) {
        self.fail_lock.store(false, Ordering::Release);
    }
}

impl crate::runtime::process_runner::SupervisedProcessRunner for RecordedRunner {
    fn run(
        &self,
        spec: crate::runtime::process_runner::SupervisedCommandSpec,
        _control: Option<crate::runtime::process_runner::RunnerTestControl>,
    ) -> BoxFuture<'_, Result<crate::runtime::process_runner::CapturedProcessResult, String>> {
        self.commands
            .lock()
            .expect("commands")
            .push(spec.command.clone());
        if spec.command.contains(" sync ") {
            // The stub is deliberately non-executable: the candidate's
            // generic MCP connect fails at exec, so no real stdio boundary
            // is ever crossed in this deterministic suite.
            let program = spec.cwd.join("venv").join(if cfg!(windows) {
                "Scripts/python.exe"
            } else {
                "bin/python"
            });
            std::fs::create_dir_all(program.parent().expect("venv bin")).expect("venv bin");
            std::fs::write(program, b"scripted stub interpreter\n").expect("stub interpreter");
        }
        let result = if spec.command.contains(" lock ") && self.fail_lock.load(Ordering::Acquire) {
            crate::runtime::process_runner::CapturedProcessResult {
                exit_code: Some(1),
                intent: crate::runtime::process_runner::ProcessOutcomeIntent::Completed,
                stdout: Vec::new(),
                stderr: b"injected build failure\n".to_vec(),
            }
        } else {
            let stdout = if spec.command.contains("python3") {
                b"Python 3.12.13\n".to_vec()
            } else if spec.command.contains("--version") {
                b"uv 0.11.12\n".to_vec()
            } else {
                Vec::new()
            };
            crate::runtime::process_runner::CapturedProcessResult {
                exit_code: Some(0),
                intent: crate::runtime::process_runner::ProcessOutcomeIntent::Completed,
                stdout,
                stderr: Vec::new(),
            }
        };
        Box::pin(async move { Ok(result) })
    }
}

/// One coordinator fixture: a workspace with managed packages, the real
/// coordinator, and its Python store installed with the recorded backend.
struct Fixture {
    workspace_root: PathBuf,
    coordinator: CapabilityCoordinator,
    runner: Arc<RecordedRunner>,
    /// The canonical managed-package store root
    /// (`<environment store>/python-tools`).
    store_root: PathBuf,
    /// The storage-root owner, declared LAST: fields drop in declaration
    /// order, so the coordinator drops before the directory goes away.
    _dir: tempfile::TempDir,
}

const SERVER_V1: &str = "from fastmcp import FastMCP\nmcp = FastMCP('demo')\n";
const SERVER_V2: &str = "from fastmcp import FastMCP\nmcp = FastMCP('demo-v2')\n";

fn fixture() -> Fixture {
    fixture_with_servers(std::collections::BTreeMap::new())
}

fn fixture_with_servers(mcp_servers: rustx::tools::mcp::McpServerBindings) -> Fixture {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let package_root = workspace_root.join(".agents/tools/demo");
    std::fs::create_dir_all(&package_root).expect("package folder");
    std::fs::write(package_root.join("server.py"), SERVER_V1).expect("server source");
    std::fs::write(package_root.join("requirements.txt"), "# none\n").expect("requirements");
    let coordinator = CapabilityCoordinator::with_backend(
        CapabilityCoordinatorConfig {
            conversation_id: ConversationId::new("conv-python-tools"),
            workspace: Workspace::new(&workspace_root).expect("workspace"),
            base_tool_registry: Arc::new(ToolRegistry::new()),
            tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
            skill_discovery: rustx::skills::SkillDiscoveryConfig {
                automatic_roots: vec![workspace_root.join(".agents/skills")],
                explicit_paths: Vec::new(),
            },
            mcp_servers,
            base_environment: ToolEnvironment::new(),
            environment_store_root: dir.path().join("skill-env"),
        },
        Arc::new(common::FakeSkillEnvironmentBackend::new()),
    )
    .expect("coordinator");
    // The coordinator canonicalizes the environment store root while
    // establishing it; the managed-package store must be installed at that
    // exact location so on-disk assertions observe its writes.
    let store_root = std::fs::canonicalize(dir.path().join("skill-env"))
        .expect("environment store root")
        .join("python-tools");
    let runner = Arc::new(RecordedRunner::default());
    let store = crate::tools::python::PythonToolStore::with_binaries_and_runner(
        store_root.clone(),
        PathBuf::from("/fake/uv"),
        PathBuf::from("/fake/python3"),
        runner.clone(),
    )
    .expect("python store");
    coordinator.install_python_store(store);
    Fixture {
        workspace_root,
        coordinator,
        runner,
        store_root,
        _dir: dir,
    }
}

/// The fingerprint-keyed published state directories of the store.
fn state_dirs(store_root: &Path) -> Vec<PathBuf> {
    let mut dirs = std::fs::read_dir(store_root.join("packages"))
        .expect("packages directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("sha256:"))
        })
        .collect::<Vec<_>>();
    dirs.sort();
    dirs
}

fn demo_source() -> CapabilitySourceId {
    CapabilitySourceId::Mcp(python_server_id("demo"))
}

/// The package compiled into the generic MCP source plane and failed its
/// (scripted, non-executable) connect: the source is observably unavailable.
fn assert_demo_connect_failed(candidate: &rustx::capabilities::PreparedCapabilityCandidate) {
    let Some(CapabilitySourceState::Unavailable { reason }) =
        candidate.availability().get(&demo_source())
    else {
        panic!(
            "the package's synthesized MCP source is evaluated: {:?}",
            candidate.availability()
        );
    };
    assert!(
        reason.contains("MCP discovery failed"),
        "the connect failure is the scripted stub's exec failure: {reason}"
    );
}

/// An unchanged package is prepared exactly once across repeated
/// activations: the second preparation reuses the validated published state
/// (no second uv build), the coordinator keeps one stable store identity,
/// and the identical rediscovery is a commit no-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unchanged_package_is_prepared_once_across_activations() {
    let fixture = fixture();

    let candidate = fixture
        .coordinator
        .prepare_candidate()
        .await
        .expect("first prepare");
    assert_demo_connect_failed(&candidate);
    assert_eq!(fixture.runner.lock_command_count(), 1, "one physical build");
    let states = state_dirs(&fixture.store_root);
    assert_eq!(states.len(), 1, "one published state");
    let store_identity = fixture
        .coordinator
        .python_store_identity_token()
        .expect("the store is initialized");
    let first = fixture.coordinator.commit(candidate).expect("first commit");
    let first_revision = first.revision();

    // Reload/activation with unchanged package bytes: the validated state is
    // reused verbatim, no uv lock runs again, and the commit is a no-op.
    let candidate = fixture
        .coordinator
        .prepare_candidate()
        .await
        .expect("second prepare");
    assert_demo_connect_failed(&candidate);
    assert_eq!(
        fixture.runner.lock_command_count(),
        1,
        "an unchanged fingerprint reuses the prepared state"
    );
    assert_eq!(state_dirs(&fixture.store_root), states);
    assert_eq!(
        fixture.coordinator.python_store_identity_token(),
        Some(store_identity),
        "the coordinator retains the one stable store identity"
    );
    let second = fixture
        .coordinator
        .commit(candidate)
        .expect("second commit");
    assert_eq!(
        second.revision(),
        first_revision,
        "an identical rediscovery never fabricates a revision"
    );
}

/// A source edit is a new preparation identity: the next activation builds a
/// new fingerprint-keyed state, and the previously published state is left
/// byte-identical — filesystem edits never mutate prepared state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_source_edit_prepares_anew_and_preserves_the_prior_state() {
    let fixture = fixture();

    let candidate = fixture
        .coordinator
        .prepare_candidate()
        .await
        .expect("first prepare");
    fixture.coordinator.commit(candidate).expect("first commit");
    let states = state_dirs(&fixture.store_root);
    let [state_v1] = states.as_slice() else {
        panic!("exactly one published state: {states:?}");
    };
    let frozen_server = std::fs::read(state_v1.join("source/server.py")).expect("frozen source");
    let frozen_manifest = std::fs::read(state_v1.join("manifest.json")).expect("frozen manifest");
    assert_eq!(frozen_server, SERVER_V1.as_bytes());

    std::fs::write(
        fixture.workspace_root.join(".agents/tools/demo/server.py"),
        SERVER_V2,
    )
    .expect("edit the live source");
    let candidate = fixture
        .coordinator
        .prepare_candidate()
        .await
        .expect("second prepare");
    assert_demo_connect_failed(&candidate);
    assert_eq!(
        fixture.runner.lock_command_count(),
        2,
        "a changed fingerprint prepares anew"
    );
    let states = state_dirs(&fixture.store_root);
    assert_eq!(states.len(), 2, "the new state is a new directory");
    assert!(
        states.contains(state_v1),
        "the prior state is retained (no GC)"
    );
    assert_eq!(
        std::fs::read(state_v1.join("source/server.py")).expect("frozen source"),
        frozen_server,
        "the prior frozen source is byte-identical after the edit"
    );
    assert_eq!(
        std::fs::read(state_v1.join("manifest.json")).expect("frozen manifest"),
        frozen_manifest,
        "the prior manifest is byte-identical after the edit"
    );
}

/// A failed build never replaces previously valid prepared state: the
/// failure lands on the package's own source availability, the staging
/// scratch is removed, the prior state directory is untouched, and reverting
/// the source reuses the prior state without another build.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_build_preserves_the_prior_published_state() {
    let fixture = fixture();

    let candidate = fixture
        .coordinator
        .prepare_candidate()
        .await
        .expect("first prepare");
    fixture.coordinator.commit(candidate).expect("first commit");
    let states = state_dirs(&fixture.store_root);
    let [state_v1] = states.as_slice() else {
        panic!("exactly one published state: {states:?}");
    };
    let frozen_manifest = std::fs::read(state_v1.join("manifest.json")).expect("frozen manifest");

    // The next build of the edited package fails before publication.
    fixture.runner.fail_builds();
    std::fs::write(
        fixture.workspace_root.join(".agents/tools/demo/server.py"),
        SERVER_V2,
    )
    .expect("edit the live source");
    let candidate = fixture
        .coordinator
        .prepare_candidate()
        .await
        .expect("a package failure never fails the candidate");
    let Some(CapabilitySourceState::Unavailable { reason }) =
        candidate.availability().get(&demo_source())
    else {
        panic!(
            "the failed build lands on the package's own source: {:?}",
            candidate.availability()
        );
    };
    assert!(
        reason.contains("injected build failure"),
        "the availability carries the build diagnostic: {reason}"
    );
    assert_eq!(
        state_dirs(&fixture.store_root),
        vec![state_v1.clone()],
        "no new state is published by a failed build"
    );
    assert!(
        !std::fs::read_dir(fixture.store_root.join("packages"))
            .expect("packages directory")
            .any(|entry| entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".build-")),
        "the failed staging scratch is removed"
    );
    assert_eq!(
        std::fs::read(state_v1.join("manifest.json")).expect("frozen manifest"),
        frozen_manifest,
        "the prior state is never mutated by a failed rebuild"
    );

    // Reverting the source reuses the still-valid prior state: no third
    // build, and the package's source recovers to the (scripted) connect
    // failure rather than the build failure.
    fixture.runner.stop_failing_builds();
    std::fs::write(
        fixture.workspace_root.join(".agents/tools/demo/server.py"),
        SERVER_V1,
    )
    .expect("restore the original source");
    let candidate = fixture
        .coordinator
        .prepare_candidate()
        .await
        .expect("third prepare");
    assert_eq!(
        fixture.runner.lock_command_count(),
        2,
        "the reverted fingerprint reuses the prior state without rebuilding"
    );
    let Some(CapabilitySourceState::Unavailable { reason }) =
        candidate.availability().get(&demo_source())
    else {
        panic!("the source is evaluated: {:?}", candidate.availability());
    };
    assert!(
        !reason.contains("injected build failure"),
        "the recovered preparation no longer carries the build failure: {reason}"
    );
}

/// A managed package never shadows a configured MCP server: a folder named
/// after an existing server identity is rejected with a diagnostic naming
/// the collision, recorded on that source's availability.
///
/// The configured server's program does not exist, so its own connect fails
/// deterministically at spawn (ENOENT) with no real process; the collision
/// diagnostic must win over that connect failure in the recorded state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_package_never_shadows_a_configured_server_identity() {
    let server_id = python_server_id("demo");
    let fixture = fixture_with_servers(std::collections::BTreeMap::from([(
        server_id,
        rustx::tools::mcp::McpServerBinding {
            transport: rustx::tools::mcp::McpTransportConfig::Stdio {
                program: "/nonexistent/rustx-issue174-configured-server".to_owned(),
                args: Vec::new(),
                cwd: None,
                environment: std::collections::BTreeMap::new(),
            },
            policy: rustx::tools::types::ToolInvocationPolicy::default(),
        },
    )]));

    let candidate = fixture
        .coordinator
        .prepare_candidate()
        .await
        .expect("prepare");
    let Some(CapabilitySourceState::Unavailable { reason }) =
        candidate.availability().get(&demo_source())
    else {
        panic!(
            "the colliding source is evaluated: {:?}",
            candidate.availability()
        );
    };
    assert!(
        reason.contains("python:demo") && reason.contains("already uses"),
        "the rejection names the identity collision, not the configured \
         server's connect failure: {reason}"
    );
}
