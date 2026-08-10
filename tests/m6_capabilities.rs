//! M6 deterministic tests: capability snapshots, quiescent commits,
//! environment materialization/publication, and background environment
//! retention.
//!
//! Every materialization test uses the deterministic fake backend
//! (`common::FakeSkillEnvironmentBackend`): no test ever touches a public
//! package registry. Race semantics use exact synchronization points
//! (watches, notify gates, registry state) — never sleeps.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use rustx::capabilities::{
    CapabilityCoordinator, CapabilityCoordinatorConfig, CapabilityPreparationError,
};
use rustx::runtime::identity::{ConversationId, ToolCallId, ToolExecutionId, ToolId};
use rustx::runtime::inbound::ConversationInboundMailbox;
use rustx::skills::Ecosystem;
use rustx::tools::artifacts::ArtifactStore;
use rustx::tools::background::{
    BackgroundDispatchOutcome, BackgroundResources, ConversationBackgroundRegistry,
};
use rustx::tools::environment::ToolEnvironment;
use rustx::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
use rustx::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode,
};
use rustx::tools::workspace::Workspace;

#[path = "common/mod.rs"]
mod common;

/// One conversation fixture: workspace (with skills), environment store,
/// artifact store, mailbox, background registry, and the coordinator.
struct Conversation {
    dir: tempfile::TempDir,
    pub workspace: Workspace,
    pub background: ConversationBackgroundRegistry,
    pub coordinator: CapabilityCoordinator,
    pub backend: common::FakeSkillEnvironmentBackend,
}

fn write_skill(root: &std::path::Path, name: &str, description: &str, deps: &[(&str, &str)]) {
    let dir = root.join(".agents/skills").join(name);
    std::fs::create_dir_all(&dir).expect("skill dir");
    let mut frontmatter = format!("---\nname: {name}\ndescription: \"{description}\"\n");
    if !deps.is_empty() {
        frontmatter.push_str("metadata:\n");
        for (key, value) in deps {
            use std::fmt::Write as _;
            let _ = writeln!(frontmatter, "  {key}: '{value}'");
        }
    }
    frontmatter.push_str("---\nbody\n");
    std::fs::write(dir.join("SKILL.md"), frontmatter).expect("SKILL.md");
}

fn python_deps(json: &str) -> (&'static str, &'static str) {
    (
        "rustx.python-dependencies",
        Box::leak(json.to_owned().into_boxed_str()),
    )
}

fn node_deps(json: &str) -> (&'static str, &'static str) {
    (
        "rustx.node-dependencies",
        Box::leak(json.to_owned().into_boxed_str()),
    )
}

fn conversation() -> Conversation {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let workspace = Workspace::new(&workspace_root).expect("workspace");
    let conversation_id = ConversationId::new("conv-m6");
    let mailbox = ConversationInboundMailbox::new(conversation_id.clone());
    let artifacts = ArtifactStore::new(conversation_id.clone(), dir.path().join("artifacts"))
        .expect("artifacts");
    let background = ConversationBackgroundRegistry::new(
        conversation_id,
        BackgroundResources {
            mailbox,
            workspace: workspace.clone(),
            artifacts,
            clock: Arc::new(rustx::runtime::SystemClock),
            event_sink: None,
        },
    );
    let backend = common::FakeSkillEnvironmentBackend::new();
    let coordinator = CapabilityCoordinator::with_backend(
        CapabilityCoordinatorConfig {
            workspace: workspace.clone(),
            tool_registry: Arc::new(ToolRegistry::new()),
            base_environment: ToolEnvironment::new(),
            environment_store_root: dir.path().join("skill-env"),
        },
        Arc::new(backend.clone()),
    )
    .expect("coordinator");
    Conversation {
        dir,
        workspace,
        background,
        coordinator,
        backend,
    }
}

async fn prepare_and_commit(
    coordinator: &CapabilityCoordinator,
) -> rustx::capabilities::CapabilitySnapshot {
    let candidate = coordinator.prepare_candidate().await.expect("prepare");
    coordinator
        .commit(candidate)
        .expect("commit")
        .as_ref()
        .clone()
}

// ---------------------------------------------------------------------------
// Environment materialization (sections 28/29)
// ---------------------------------------------------------------------------

/// Two Skills with compatible Python dependencies produce one merged
/// Python environment; two Skills with compatible Node dependencies
/// produce one merged Node environment.
#[tokio::test]
async fn compatible_skills_share_one_merged_environment_per_ecosystem() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[
            python_deps(r#"{"pypdf":"5.9.0"}"#),
            node_deps(r#"{"pdf-lib":"1.17.1"}"#),
        ],
    );
    write_skill(
        conversation.workspace.root(),
        "slides",
        "Slides skill.",
        &[
            python_deps(r#"{"pillow":"11.3.0"}"#),
            node_deps(r#"{"@scope/pkg":"2.0.0"}"#),
        ],
    );
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    let python = snapshot.python_environment().expect("python env");
    let node = snapshot.node_environment().expect("node env");
    assert_eq!(
        conversation
            .backend
            .materialization_count(Ecosystem::Python),
        1,
        "one shared Python environment, never one per Skill"
    );
    assert_eq!(
        conversation.backend.materialization_count(Ecosystem::Node),
        1,
        "one shared Node environment, never one per Skill"
    );
    let prepared = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect("prepare");
    let merged_python = prepared
        .skill_packages()
        .iter()
        .flat_map(|package| package.dependencies().python.clone())
        .collect::<BTreeMap<_, _>>();
    assert_eq!(merged_python.len(), 2);
    // The effective environment carries both bin prefixes.
    let entries = snapshot
        .effective_environment()
        .child_environment(conversation.workspace.root());
    let path = entries
        .iter()
        .find(|(key, _)| key == "PATH")
        .expect("PATH")
        .1
        .clone();
    assert!(path.starts_with(&format!(
        "{}:{}:",
        python.bin_dir.display(),
        node.bin_dir.display()
    )));
    assert!(entries.iter().any(|(key, value)| {
        key == "VIRTUAL_ENV" && value == &python.root.display().to_string()
    }));
    assert!(entries.iter().any(|(key, value)| {
        key == "NODE_PATH" && value == &node.modules_dir.display().to_string()
    }));
}

/// No ecosystem dependencies → no runtime requirement and no empty
/// environment materialization.
#[tokio::test]
async fn no_dependencies_means_no_environment_and_no_runtime_requirement() {
    let conversation = conversation();
    write_skill(conversation.workspace.root(), "shell", "Shell skill.", &[]);
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    assert!(snapshot.python_environment().is_none());
    assert!(snapshot.node_environment().is_none());
    assert_eq!(conversation.backend.calls(), Vec::new());
    let entries = snapshot
        .effective_environment()
        .child_environment(conversation.workspace.root());
    let path = entries
        .iter()
        .find(|(key, _)| key == "PATH")
        .expect("PATH")
        .1
        .clone();
    assert_eq!(path, "/usr/local/bin:/usr/bin:/bin");
}

/// An already-published identical digest is reused; the published
/// environment is never modified during reuse.
#[tokio::test]
async fn published_identical_digest_is_reused_and_never_modified() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let first = prepare_and_commit(&conversation.coordinator).await;
    let python = first.python_environment().expect("python env");
    let marker_before =
        std::fs::read(python.root.join(rustx::skills::ENVIRONMENT_MANIFEST_FILE)).expect("marker");
    let modified_before = std::fs::metadata(python.root.join("bin/python"))
        .expect("env file")
        .modified()
        .expect("mtime");

    // A second preparation with the same declarations reuses the published
    // digest directory without installing into it again.
    let second = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(
        second.python_environment().expect("env").digest,
        python.digest
    );
    assert_eq!(
        conversation
            .backend
            .materialization_count(Ecosystem::Python),
        1,
        "a published environment is never installed into again"
    );
    let marker_after =
        std::fs::read(python.root.join(rustx::skills::ENVIRONMENT_MANIFEST_FILE)).expect("marker");
    assert_eq!(marker_before, marker_after);
    let modified_after = std::fs::metadata(python.root.join("bin/python"))
        .expect("env file")
        .modified()
        .expect("mtime");
    assert_eq!(
        modified_before, modified_after,
        "reuse never mutates the environment"
    );
}

/// The absolute environment store path never changes the environment
/// digest: two stores in different roots with the same inputs produce the
/// same digest.
#[tokio::test]
async fn absolute_store_path_does_not_change_the_digest() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let first = prepare_and_commit(&conversation.coordinator).await;
    let digest = first.python_environment().expect("env").digest.clone();

    let conversation_two = {
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace_root = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let workspace = Workspace::new(&workspace_root).expect("workspace");
        let conversation_id = ConversationId::new("conv-m6-two");
        let mailbox = ConversationInboundMailbox::new(conversation_id.clone());
        let background = ConversationBackgroundRegistry::new(
            conversation_id,
            BackgroundResources {
                mailbox,
                workspace: workspace.clone(),
                artifacts: ArtifactStore::new(
                    ConversationId::new("conv-m6-two"),
                    dir.path().join("artifacts"),
                )
                .expect("artifacts"),
                clock: Arc::new(rustx::runtime::SystemClock),
                event_sink: None,
            },
        );
        let _ = background;
        let backend = common::FakeSkillEnvironmentBackend::new();
        let coordinator = CapabilityCoordinator::with_backend(
            CapabilityCoordinatorConfig {
                workspace: workspace.clone(),
                tool_registry: Arc::new(ToolRegistry::new()),
                base_environment: ToolEnvironment::new(),
                environment_store_root: dir.path().join("skill-env"),
            },
            Arc::new(backend.clone()),
        )
        .expect("coordinator");
        Conversation {
            dir,
            workspace,
            background,
            coordinator,
            backend,
        }
    };
    write_skill(
        conversation_two.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let second = prepare_and_commit(&conversation_two.coordinator).await;
    assert_eq!(
        second.python_environment().expect("env").digest,
        digest,
        "the store root is never part of the environment identity"
    );
}

/// A corrupt published manifest means the digest directory is never
/// reused and never mutated; the candidate fails explicitly.
#[tokio::test]
async fn corrupt_published_manifest_fails_the_candidate() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    let python = snapshot.python_environment().expect("python env");
    std::fs::write(
        python.root.join(rustx::skills::ENVIRONMENT_MANIFEST_FILE),
        b"{}",
    )
    .expect("corrupt the marker");
    let error = conversation
        .coordinator
        .prepare_candidate()
        .await
        .map(drop)
        .expect_err("corrupt environment must fail the candidate");
    assert!(matches!(
        error,
        CapabilityPreparationError::Environment(
            rustx::skills::EnvironmentPreparationError::CorruptPublishedEnvironment { .. }
        )
    ));
}

/// Materialization failure leaves the active capability unchanged and
/// removes the staging directory.
#[tokio::test]
async fn materialization_failure_leaves_the_active_capability_unchanged() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    let revision = snapshot.revision();
    assert!(
        snapshot.python_environment().is_some(),
        "revision N has an environment"
    );

    conversation
        .backend
        .fail_python_materialization("injected install failure");
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.10.0"}"#)],
    );
    let error = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect_err("materialization failure");
    assert!(matches!(
        error,
        CapabilityPreparationError::Environment(
            rustx::skills::EnvironmentPreparationError::MaterializationFailed { .. }
        )
    ));
    let current = conversation.coordinator.current_snapshot();
    assert_eq!(
        current.revision(),
        revision,
        "failed preparation leaves the active revision authoritative"
    );
    // The staging directory was removed.
    let store_root = std::fs::read_dir(
        conversation
            .coordinator
            .current_snapshot()
            .effective_environment()
            .child_environment(conversation.workspace.root())
            .first()
            .map(|_| conversation.dir.path().join("skill-env"))
            .expect("store root"),
    )
    .expect("store exists");
    for entry in store_root {
        let name = entry.expect("entry").file_name();
        assert!(
            !name.to_string_lossy().starts_with(".staging-"),
            "staging state must be cleaned after failure"
        );
    }
}

/// Atomic publication is the only point at which a materialized
/// environment becomes reusable: while materialization is gated, no digest
/// directory exists; after publication it does.
#[tokio::test]
async fn atomic_publication_is_the_only_reusable_point() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let gate = conversation.backend.install_materialize_gate();
    let store_root = conversation.dir.path().join("skill-env").join("python");
    let prepare_task = {
        let coordinator = conversation.coordinator.clone();
        tokio::spawn(async move { coordinator.prepare_candidate().await })
    };
    gate.await_entered().await;
    // Materialization began; no digest directory is reusable yet.
    let entries: Vec<String> = std::fs::read_dir(&store_root)
        .expect("store")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        !entries.iter().any(|name| !name.starts_with(".staging-")),
        "no published environment exists before publication, got {entries:?}"
    );
    gate.release();
    prepare_task.await.expect("prepare task").expect("prepare");
    let entries: Vec<String> = std::fs::read_dir(&store_root)
        .expect("store")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        entries
            .iter()
            .any(|name| name.starts_with("sha256:") && !name.starts_with(".staging-")),
        "the environment becomes reusable only after atomic publication, got {entries:?}"
    );
}

/// A dependency conflict is reported before any environment materialization
/// (no package-manager subprocess runs).
#[tokio::test]
async fn conflict_is_reported_before_materialization() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "a",
        "Skill A.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    write_skill(
        conversation.workspace.root(),
        "b",
        "Skill B.",
        &[python_deps(r#"{"pypdf":"5.10.0"}"#)],
    );
    let error = conversation
        .coordinator
        .prepare_candidate()
        .await
        .map(drop)
        .expect_err("conflict");
    assert!(matches!(
        error,
        CapabilityPreparationError::DependencyConflict(conflict)
            if conflict.package == "pypdf" && conflict.declarations.len() == 2
    ));
    assert_eq!(conversation.backend.calls(), Vec::new());
}

// ---------------------------------------------------------------------------
// Capability snapshot and quiescence (section 31)
// ---------------------------------------------------------------------------

/// Attempt N snapshots one immutable revision; every acquisition while the
/// lease is held observes the same snapshot.
#[tokio::test]
async fn attempt_lease_pins_one_immutable_revision() {
    let conversation = conversation();
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    let lease = conversation.coordinator.acquire_attempt_lease();
    assert_eq!(lease.snapshot().revision(), snapshot.revision());
    let snapshot_again = conversation.coordinator.current_snapshot();
    assert_eq!(snapshot_again.revision(), snapshot.revision());
    assert_eq!(&**lease.snapshot(), &snapshot);
    drop(lease);
    assert_eq!(conversation.coordinator.active_attempts(), 0);
}

/// A capability commit while an attempt lease is active is rejected as
/// busy; after the lease releases, the candidate commits atomically and
/// the next attempt observes the new revision.
#[tokio::test]
async fn commit_is_busy_while_a_lease_is_active_then_commits_atomically() {
    let conversation = conversation();
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    let revision_n = snapshot.revision();
    let lease = conversation.coordinator.acquire_attempt_lease();

    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let candidate = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect("prepare");
    let error = conversation
        .coordinator
        .commit(candidate)
        .expect_err("busy");
    assert_eq!(error, rustx::capabilities::CapabilityCommitError::Busy);
    assert_eq!(
        conversation.coordinator.current_snapshot().revision(),
        revision_n,
        "a rejected commit never mutates the active revision"
    );

    drop(lease);
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let candidate = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect("prepare");
    let committed = conversation.coordinator.commit(candidate).expect("commit");
    assert_eq!(
        committed.revision(),
        rustx::runtime::identity::CapabilityRevision::new(revision_n.get() + 1)
    );
    // The next attempt snapshots the new revision.
    let next_lease = conversation.coordinator.acquire_attempt_lease();
    assert_eq!(next_lease.revision(), committed.revision());
    assert_eq!(
        next_lease.snapshot().skill_catalog_attachment(),
        committed.skill_catalog_attachment()
    );
}

/// Candidate N+1 preparation cannot mutate attempt N: preparation runs
/// while the lease is held and the pinned snapshot stays byte-identical.
#[tokio::test]
async fn candidate_preparation_cannot_mutate_an_active_attempt() {
    let conversation = conversation();
    let _snapshot = prepare_and_commit(&conversation.coordinator).await;
    let lease = conversation.coordinator.acquire_attempt_lease();
    let pinned = lease.snapshot().clone();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let _candidate = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect("prepare");
    assert_eq!(
        lease.snapshot().as_ref(),
        &*pinned,
        "preparation never mutates the attempt's snapshot"
    );
    drop(lease);
}

/// Unchanged rediscovery/preparation is a no-op and does not fabricate a
/// new revision.
#[tokio::test]
async fn unchanged_preparation_is_a_noop() {
    let conversation = conversation();
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    let candidate = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect("prepare");
    let committed = conversation.coordinator.commit(candidate).expect("commit");
    assert_eq!(committed.revision(), snapshot.revision());
    assert_eq!(committed.as_ref(), &snapshot);
}

/// A description-only change increments the capability revision without
/// changing the environment identity; a dependency-only change changes
/// both.
#[tokio::test]
async fn description_change_bumps_revision_without_changing_environment_identity() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "First description.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    let python_digest = snapshot.python_environment().expect("env").digest.clone();

    write_skill(
        conversation.workspace.root(),
        "pdf",
        "Second description.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let second = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(
        second.revision(),
        rustx::runtime::identity::CapabilityRevision::new(snapshot.revision().get() + 1)
    );
    assert_ne!(
        second.skills().bindings(),
        snapshot.skills().bindings(),
        "a description change yields a new SkillVersionId binding"
    );
    assert_eq!(
        second.python_environment().expect("env").digest,
        python_digest,
        "the environment identity is unchanged when dependency inputs are unchanged"
    );

    write_skill(
        conversation.workspace.root(),
        "pdf",
        "Second description.",
        &[python_deps(r#"{"pypdf":"5.10.0"}"#)],
    );
    let third = prepare_and_commit(&conversation.coordinator).await;
    assert_ne!(
        third.python_environment().expect("env").digest,
        python_digest,
        "a dependency change changes the environment identity"
    );
}

/// A stale candidate cannot overwrite a newer revision.
#[tokio::test]
async fn stale_candidate_cannot_overwrite_a_newer_revision() {
    let conversation = conversation();
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let stale = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect("prepare");
    // The active revision advances first.
    let first_commit = conversation.coordinator.commit(stale).expect("commit");
    assert_eq!(first_commit.revision().get(), snapshot.revision().get() + 1);

    write_skill(
        conversation.workspace.root(),
        "pdf",
        "Changed again.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let second = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect("prepare");
    conversation.coordinator.commit(second).expect("commit");

    // A candidate prepared from the now-obsolete base revision is stale.
    // Re-prepare from the current revision, then advance the active
    // revision further so this candidate becomes stale.
    let candidate = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect("prepare");
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "Even newer.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect("prepare");
    conversation
        .coordinator
        .commit(
            conversation
                .coordinator
                .prepare_candidate()
                .await
                .expect("prepare"),
        )
        .expect("commit");
    let error = conversation
        .coordinator
        .commit(candidate)
        .expect_err("stale");
    assert!(matches!(
        error,
        rustx::capabilities::CapabilityCommitError::StaleCandidate { .. }
    ));
}

/// Failed candidate preparation leaves the current revision authoritative.
#[tokio::test]
async fn failed_preparation_leaves_revision_authoritative() {
    let conversation = conversation();
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    write_skill(
        conversation.workspace.root(),
        "bad",
        "Bad skill.",
        &[python_deps(r#"{"pypdf":"5.9.0","other":"not a version"}"#)],
    );
    let _ = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect_err("malformed candidate");
    assert_eq!(
        conversation.coordinator.current_snapshot().revision(),
        snapshot.revision()
    );
}

// ---------------------------------------------------------------------------
// Background environment retention (section 32)
// ---------------------------------------------------------------------------

/// A recording executor that captures the effective `ToolEnvironment` from
/// its execution context and parks until released.
struct RecordingParkingExecutor {
    environment: Arc<Mutex<Option<ToolEnvironment>>>,
    seen: tokio::sync::watch::Sender<bool>,
    release: Arc<tokio::sync::Notify>,
}

impl RecordingParkingExecutor {
    fn new() -> (
        Self,
        tokio::sync::watch::Receiver<bool>,
        Arc<tokio::sync::Notify>,
    ) {
        let (seen, seen_rx) = tokio::sync::watch::channel(false);
        let release = Arc::new(tokio::sync::Notify::new());
        (
            Self {
                environment: Arc::new(Mutex::new(None)),
                seen,
                release: release.clone(),
            },
            seen_rx,
            release,
        )
    }

    fn recorded_environment(&self) -> Option<ToolEnvironment> {
        self.environment.lock().expect("env lock").clone()
    }
}

impl ToolExecutor for RecordingParkingExecutor {
    fn execute<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> futures_util::future::BoxFuture<'a, ToolExecutionResult> {
        *self.environment.lock().expect("env lock") = Some(context.environment.clone());
        let _ = self.seen.send(true);
        let release = self.release.clone();
        Box::pin(async move {
            release.notified().await;
            ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
            }
        })
    }
}

fn background_invocation() -> ToolInvocation {
    ToolInvocation {
        call_id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-bg"),
        tool_name: "bg".to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    }
}

/// A deterministic background Bash execution under revision N retains
/// environment N after attempt N terminates, while revision N+1 activates
/// and new executions use environment N+1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // one coherent retention scenario
async fn background_execution_retains_its_dispatching_environment() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    // Revision N with python runtime A.
    let snapshot_n = prepare_and_commit(&conversation.coordinator).await;
    let lease_n = conversation.coordinator.acquire_attempt_lease();
    let environment_n = lease_n.snapshot().effective_environment().clone();

    // Dispatch a background execution under revision N: the environment is
    // captured at prepare time, before the ownership commit.
    let (executor, mut started, release) = RecordingParkingExecutor::new();
    let executor: Arc<RecordingParkingExecutor> = Arc::new(executor);
    let prepared = conversation
        .background
        .prepare_dispatch(
            &background_invocation(),
            &(executor.clone() as Arc<dyn ToolExecutor>),
            environment_n.clone(),
        )
        .expect("prepare");
    let outcome = conversation
        .background
        .commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new());
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    started
        .wait_for(|started| *started)
        .await
        .expect("background execution started under revision N");
    assert_eq!(
        executor
            .recorded_environment()
            .expect("captured environment"),
        environment_n,
        "the detached execution captured environment N"
    );

    // Attempt N terminates; the background execution stays active.
    drop(lease_n);

    // Revision N+1 activates with a different Python runtime identity. The
    // active detached execution does not block the capability commit.
    conversation.backend.set_python_runtime("Python 3.13.0");
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let snapshot_n1 = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(
        snapshot_n1.revision().get(),
        snapshot_n.revision().get() + 1,
        "an active detached background execution never blocks a capability commit"
    );
    assert_ne!(
        snapshot_n1.python_environment().expect("env").digest,
        snapshot_n.python_environment().expect("env").digest
    );

    // The old background execution still has environment N.
    assert_eq!(
        executor
            .recorded_environment()
            .expect("captured environment"),
        environment_n,
        "revision N+1 activation never mutates the old background environment"
    );

    // A new foreground execution (attempt N+1) uses environment N+1.
    let lease_n1 = conversation.coordinator.acquire_attempt_lease();
    assert_eq!(
        lease_n1.snapshot().effective_environment(),
        &snapshot_n1.effective_environment().clone(),
        "attempt N+1 uses environment N+1"
    );

    // A new background execution under N+1 captures environment N+1.
    let (executor2, mut started2, release2) = RecordingParkingExecutor::new();
    let executor2: Arc<RecordingParkingExecutor> = Arc::new(executor2);
    let prepared2 = conversation
        .background
        .prepare_dispatch(
            &background_invocation(),
            &(executor2.clone() as Arc<dyn ToolExecutor>),
            lease_n1.snapshot().effective_environment().clone(),
        )
        .expect("prepare");
    let outcome2 = conversation
        .background
        .commit_dispatch(prepared2, &rustx::runtime::CancellationSignal::new());
    let BackgroundDispatchOutcome::Accepted {
        execution_id: id2, ..
    } = outcome2
    else {
        panic!("accepted");
    };
    started2
        .wait_for(|started| *started)
        .await
        .expect("second background execution started");
    assert_eq!(
        executor2
            .recorded_environment()
            .expect("captured environment"),
        snapshot_n1.effective_environment().clone(),
        "a new background execution uses environment N+1"
    );

    // Release both executions; each settles.
    release.notify_one();
    release2.notify_one();
    wait_for_terminal(&conversation, &execution_id).await;
    wait_for_terminal(&conversation, &id2).await;
    drop(lease_n1);
}

async fn wait_for_terminal(conversation: &Conversation, execution_id: &ToolExecutionId) {
    for _ in 0..400 {
        let snapshot = conversation
            .background
            .snapshot(execution_id)
            .expect("snapshot");
        if snapshot.state.is_terminal() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("background execution never reached a terminal state");
}

// ---------------------------------------------------------------------------
// Agent loop integration: the attempt uses the snapshot's catalog and
// environment on every turn (sections 22/31/33)
// ---------------------------------------------------------------------------

/// Every model turn of one attempt carries the exact same Skill catalog
/// attachment and effective environment: the attempt runs multiple turns
/// while its lease is held, and the catalog never changes mid-attempt.
#[tokio::test]
#[allow(clippy::too_many_lines)] // one coherent multi-turn scenario
async fn every_turn_uses_the_attempts_immutable_catalog_and_environment() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    // The conversation fixture coordinator uses an empty registry; this
    // test needs the fake tool registered, so it builds its own coordinator
    // over the same workspace/store.
    let mut tools = ToolRegistry::new();
    let fake_tool = common::fake::FakeTool::new(
        common::tool("alpha", "tool-alpha"),
        common::fake::success_result("ok"),
    );
    fake_tool.register(&mut tools);
    let tools = Arc::new(tools);
    let coordinator = CapabilityCoordinator::with_backend(
        CapabilityCoordinatorConfig {
            workspace: conversation.workspace.clone(),
            tool_registry: tools.clone(),
            base_environment: ToolEnvironment::new(),
            environment_store_root: conversation.dir.path().join("skill-env-2"),
        },
        Arc::new(conversation.backend.clone()),
    )
    .expect("coordinator");
    let snapshot = prepare_and_commit(&coordinator).await;
    let lease = coordinator.acquire_attempt_lease();
    let catalog = snapshot
        .skill_catalog_attachment()
        .expect("catalog")
        .clone();

    // A two-turn model script: turn 1 is a tool-call turn, turn 2 stops.
    let call = common::fake::ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let mut first = vec![common::fake::FakeStep::Emit(
        rustx::model::ModelEvent::Started,
    )];
    for event in common::fake::tool_call_events(0, &call) {
        first.push(common::fake::FakeStep::Emit(event));
    }
    first.push(common::fake::FakeStep::Emit(
        rustx::model::ModelEvent::Completed {
            finish_reason: rustx::model::ModelFinishReason::ToolCalls,
            usage: None,
        },
    ));
    let model = common::fake::FakeModel::new(vec![
        first,
        vec![
            common::fake::FakeStep::Emit(rustx::model::ModelEvent::Started),
            common::fake::FakeStep::Emit(rustx::model::ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            common::fake::FakeStep::Emit(rustx::model::ModelEvent::Completed {
                finish_reason: rustx::model::ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ]);
    let tool_runtime = common::tool_runtime_with_mailbox(
        "conv-m6",
        conversation.background.resources().mailbox.clone(),
    );
    let cancellation = rustx::agent::AgentCancellation::new(
        rustx::runtime::types::CancellationReason::UserRequested,
    );
    let request = rustx::agent::AgentExecutionRequest {
        agent_id: rustx::runtime::identity::AgentId::new("agent-1"),
        conversation_id: rustx::runtime::identity::ConversationId::new("conv-m6"),
        attempt_id: rustx::runtime::identity::AttemptId::new("attempt-1"),
        initial_messages: vec![],
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: "fake-model".to_owned(),
        protocol: rustx::model::ModelProtocol::OpenAiChatCompletions,
        reasoning: rustx::model::ReasoningEffort::Medium,
        max_output_tokens: 512,
    };
    let runtime = rustx::context::ContextRuntime::new(
        rustx::context::ContextEngine::new(
            rustx::context::ContextConfig {
                context_window_tokens: 10_000_000,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            Arc::new(rustx::context::DefaultTokenEstimator),
        )
        .expect("engine"),
        Arc::new(common::context::FakeContextSummarizer::new(Vec::new())),
        Arc::new(rustx::context::InMemoryCheckpointStore::new()),
    );
    let result = rustx::agent::AgentExecution::new(
        request,
        &model,
        &lease,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .expect("execution")
    .run()
    .await;
    assert!(matches!(
        result.outcome,
        rustx::events::types::AttemptOutcome::Completed { .. }
    ));
    let requests = model.requests();
    assert_eq!(requests.len(), 2, "two model turns");
    for request in &requests {
        assert_eq!(
            request
                .skill_catalog
                .as_ref()
                .expect("catalog on every turn"),
            &catalog,
            "every model turn uses the attempt's immutable Skill catalog"
        );
    }
    // The catalog is never canonical history, never returned in the result
    // messages, and never a committed-message event.
    assert!(
        result.messages.iter().all(|message| {
            !serde_json::to_string(message)
                .expect("serialize")
                .contains("## Skills")
        }),
        "the Skill catalog must never appear in canonical history"
    );
    assert!(
        result
            .events
            .iter()
            .all(|event| !serde_json::to_string(event)
                .expect("serialize")
                .contains("## Skills")),
        "the Skill catalog must never appear in committed-message events"
    );
    drop(lease);
}
