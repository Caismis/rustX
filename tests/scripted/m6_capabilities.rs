//! M6 deterministic tests: capability snapshots, quiescent commits,
//! environment materialization/publication, and background environment
//! retention.
//!
//! Every materialization test uses the deterministic fake backend
//! (`common::FakeSkillEnvironmentBackend`): no test ever touches a public
//! package registry. Race semantics use exact synchronization points
//! (watches, notify gates, registry state) — never sleeps.

use std::collections::BTreeMap;
use std::future::Future;
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
use rustx::tools::{NativeToolPolicies, NativeToolResources, register_native_tools};

use super::{common, support};

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

fn write_hidden_skill(root: &std::path::Path, name: &str, description: &str) {
    let dir = root.join(".agents/skills").join(name);
    std::fs::create_dir_all(&dir).expect("hidden skill dir");
    std::fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: \"{description}\"\ndisable-model-invocation: true\n---\nbody\n"
        ),
    )
    .expect("hidden SKILL.md");
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
    conversation_with_options(rustx::capabilities::ToolActivationPolicy::default())
}

fn conversation_with_options(
    tool_activation: rustx::capabilities::ToolActivationPolicy,
) -> Conversation {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let workspace = Workspace::new(&workspace_root).expect("workspace");
    let conversation_id = ConversationId::new("conv-m6");
    let mailbox = ConversationInboundMailbox::new(conversation_id.clone());
    let artifacts = ArtifactStore::new(conversation_id.clone(), dir.path().join("artifacts"))
        .expect("artifacts");
    let background = ConversationBackgroundRegistry::new(
        conversation_id.clone(),
        BackgroundResources {
            mailbox,
            workspace: workspace.clone(),
            artifacts,
            tool_output: rustx::tools::managed_output::ManagedToolOutput::new(
                conversation_id.clone(),
                dir.path().join("artifacts/tool-output"),
            )
            .expect("managed tool output"),
            clock: Arc::new(rustx::runtime::SystemClock),
            event_sink: None,
        },
    );
    let backend = common::FakeSkillEnvironmentBackend::new();
    let mut base_tool_registry = ToolRegistry::new();
    register_native_tools(
        &mut base_tool_registry,
        NativeToolResources {
            background: background.clone(),
            subagents: None,
        },
        NativeToolPolicies::default(),
    )
    .expect("native tools");
    let coordinator = CapabilityCoordinator::with_backend(
        CapabilityCoordinatorConfig {
            conversation_id: conversation_id.clone(),
            workspace: workspace.clone(),
            base_tool_registry: Arc::new(base_tool_registry),
            tool_activation,
            skill_discovery: rustx::skills::SkillDiscoveryConfig {
                automatic_roots: vec![
                    workspace.root().join(".rustx/skills"),
                    workspace.root().join(".agents/skills"),
                ],
                explicit_paths: Vec::new(),
            },
            mcp_servers: std::collections::BTreeMap::new(),
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

#[tokio::test]
async fn hidden_skills_keep_attempt_provenance_but_not_model_visibility() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "visible",
        "Visible guidance.",
        &[],
    );
    write_hidden_skill(
        conversation.workspace.root(),
        "runtime-only",
        "Runtime-only guidance.",
    );

    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(snapshot.skills().bindings().len(), 2);
    assert_eq!(snapshot.skills().catalog_entries().len(), 1);
    assert_eq!(snapshot.skills().catalog_entries()[0].name, "visible");
    assert_eq!(
        snapshot.skills().catalog_entries()[0].location,
        ".rustx/skills/visible/SKILL.md"
    );
    let rendered_catalog = snapshot.skill_catalog().expect("visible Skill catalog");
    assert_eq!(rendered_catalog.matches("## Skills").count(), 1);
    assert!(rendered_catalog.contains("visible"));
    assert!(rendered_catalog.contains("<description>Visible guidance.</description>"));
    assert!(rendered_catalog.contains("<location>.rustx/skills/visible/SKILL.md</location>"));
    assert!(!rendered_catalog.contains("runtime-only"));
    assert!(
        snapshot
            .skills()
            .resources()
            .resolve(std::path::Path::new(".rustx/skills/runtime-only/SKILL.md"))
            .is_some()
    );

    let manifest = snapshot.to_capabilities_manifest();
    let manifest_names = manifest
        .skills
        .iter()
        .map(|binding| binding.skill_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(manifest_names, vec!["runtime-only", "visible"]);

    let client_view =
        crate::runtime_client::projection::capability_view(&snapshot, &BTreeMap::new());
    assert_eq!(
        client_view
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["visible"],
        "Runtime Client Skills are the model-visible projection"
    );
    assert_eq!(
        client_view.skills[0].location,
        ".rustx/skills/visible/SKILL.md"
    );
}

/// Normal rustX agent composition keeps canonical native Read active while
/// optional Tool activation changes. Skill visibility therefore remains a
/// Skill-level decision, and the same immutable visible catalog feeds both
/// the Effective System Prompt and Runtime Client projection.
#[tokio::test]
async fn mandatory_native_read_survives_optional_activation_filters() {
    let policies = [
        rustx::capabilities::ToolActivationPolicy {
            no_tools: true,
            ..rustx::capabilities::ToolActivationPolicy::default()
        },
        rustx::capabilities::ToolActivationPolicy {
            no_builtin_tools: true,
            ..rustx::capabilities::ToolActivationPolicy::default()
        },
        rustx::capabilities::ToolActivationPolicy {
            tools: Some(vec!["write".to_owned()]),
            ..rustx::capabilities::ToolActivationPolicy::default()
        },
        rustx::capabilities::ToolActivationPolicy {
            exclude_tools: vec!["read".to_owned()],
            ..rustx::capabilities::ToolActivationPolicy::default()
        },
        rustx::capabilities::ToolActivationPolicy {
            default_tools: Some(vec!["write".to_owned()]),
            ..rustx::capabilities::ToolActivationPolicy::default()
        },
    ];

    for policy in policies {
        let conversation = conversation_with_options(policy);
        write_skill(
            conversation.workspace.root(),
            "visible",
            "Visible guidance.",
            &[],
        );
        let snapshot = prepare_and_commit(&conversation.coordinator).await;
        assert_eq!(snapshot.skills().bindings().len(), 1);
        assert!(
            snapshot
                .skills()
                .resources()
                .resolve(std::path::Path::new(".rustx/skills/visible/SKILL.md"))
                .is_some()
        );
        assert!(snapshot.tool_registry().names().contains(&"read"));
        assert_eq!(snapshot.skills().catalog_entries().len(), 1);
        assert_eq!(
            snapshot.skills().catalog_entries()[0].location,
            ".rustx/skills/visible/SKILL.md"
        );
        let catalog = snapshot.skill_catalog().expect("visible Skill catalog");
        assert!(catalog.contains("<location>.rustx/skills/visible/SKILL.md</location>"));
        let view = crate::runtime_client::projection::capability_view(&snapshot, &BTreeMap::new());
        assert_eq!(view.skills.len(), 1);
        assert_eq!(view.skills[0].location, ".rustx/skills/visible/SKILL.md");
    }
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
    let wrapper = std::fs::read_to_string(python.root.join("bin/fake-tool")).expect("wrapper");
    assert_eq!(
        wrapper.lines().next(),
        Some(format!("#!{}", python.root.join("bin/python").display()).as_str()),
        "console-script shebang points at the final immutable environment path"
    );
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
            conversation_id.clone(),
            BackgroundResources {
                mailbox,
                workspace: workspace.clone(),
                artifacts: ArtifactStore::new(
                    ConversationId::new("conv-m6-two"),
                    dir.path().join("artifacts"),
                )
                .expect("artifacts"),
                tool_output: rustx::tools::managed_output::ManagedToolOutput::new(
                    ConversationId::new("conv-m6-two"),
                    dir.path().join("artifacts/tool-output"),
                )
                .expect("managed tool output"),
                clock: Arc::new(rustx::runtime::SystemClock),
                event_sink: None,
            },
        );
        let _ = background;
        let backend = common::FakeSkillEnvironmentBackend::new();
        let coordinator = CapabilityCoordinator::with_backend(
            CapabilityCoordinatorConfig {
                conversation_id: conversation_id.clone(),
                workspace: workspace.clone(),
                base_tool_registry: Arc::new(ToolRegistry::new()),
                tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
                skill_discovery: rustx::skills::SkillDiscoveryConfig::default_for_workspace(
                    &workspace,
                ),
                mcp_servers: std::collections::BTreeMap::new(),
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

/// The ready-marker commit is the only point at which a Python environment
/// becomes reusable: while materialization is gated, the final digest
/// directory is incomplete; after publication its marker exists.
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
    let digest_name = entries
        .iter()
        .find(|name| name.starts_with("sha256:"))
        .expect("final digest directory is created before Python materialization completes");
    assert!(
        !store_root
            .join(digest_name)
            .join(rustx::skills::ENVIRONMENT_MANIFEST_FILE)
            .exists(),
        "an incomplete final-path environment has no reusable ready marker"
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
    let digest_name = entries
        .iter()
        .find(|name| name.starts_with("sha256:"))
        .expect("published digest directory");
    assert!(
        store_root
            .join(digest_name)
            .join(rustx::skills::ENVIRONMENT_MANIFEST_FILE)
            .is_file(),
        "the ready marker is the Python publication boundary"
    );
}

/// Node retains its independent staging-to-rename publication contract:
/// materialization has a private staging directory and no final digest
/// directory exists until the manifest and rename complete.
#[tokio::test]
async fn node_staging_is_published_by_atomic_rename() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "node-skill",
        "Node skill.",
        &[node_deps(r#"{"pdf-lib":"1.17.1"}"#)],
    );
    let gate = conversation.backend.install_materialize_gate();
    let node_root = conversation.dir.path().join("skill-env").join("node");
    let prepare = {
        let coordinator = conversation.coordinator.clone();
        tokio::spawn(async move { coordinator.prepare_candidate().await })
    };
    gate.await_entered().await;

    let entries = std::fs::read_dir(&node_root)
        .expect("node store")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert!(
        entries
            .iter()
            .any(|name| name.to_string_lossy().starts_with(".staging-")),
        "Node materialization uses a private staging directory"
    );
    assert!(
        !entries
            .iter()
            .any(|name| name.to_string_lossy().starts_with("sha256:")),
        "Node final digest is not visible before rename"
    );

    gate.release();
    let candidate = prepare.await.expect("prepare task").expect("prepare");
    let node = candidate.node_environment().expect("Node environment");
    assert!(node.root.is_dir());
    assert!(
        node.root
            .join(rustx::skills::ENVIRONMENT_MANIFEST_FILE)
            .is_file()
    );
    assert!(
        std::fs::read_dir(&node_root)
            .expect("node store")
            .all(|entry| {
                !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".staging-")
            }),
        "published Node staging is consumed by rename"
    );
}

/// Same-digest preparations share one in-process build owner. The barrier
/// proves the two preparations overlap while only one Python materialization
/// is active; both callers then receive the same immutable identities.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_digest_preparations_coalesce_for_python_and_node() {
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
    let gate = conversation.backend.install_materialize_gate();
    let first_coordinator = conversation.coordinator.clone();
    let second_coordinator = conversation.coordinator.clone();
    let first = tokio::spawn(async move { first_coordinator.prepare_candidate().await });
    let second = tokio::spawn(async move { second_coordinator.prepare_candidate().await });
    gate.await_entered().await;
    assert_eq!(
        conversation
            .backend
            .materialization_count(Ecosystem::Python),
        1,
        "one caller owns the overlapping Python build"
    );
    gate.release();
    let first = first.await.expect("first task").expect("first prepare");
    let second = second.await.expect("second task").expect("second prepare");
    assert_eq!(
        first
            .python_environment()
            .expect("Python environment")
            .digest,
        second
            .python_environment()
            .expect("Python environment")
            .digest
    );
    assert_eq!(
        first.node_environment().expect("Node environment").digest,
        second.node_environment().expect("Node environment").digest
    );
    assert_eq!(
        conversation
            .backend
            .materialization_count(Ecosystem::Python),
        1
    );
    assert_eq!(
        conversation.backend.materialization_count(Ecosystem::Node),
        1,
        "the same generic build coordination covers Node"
    );
}

/// Cancelling the initiating preparation only drops that caller's wait. The
/// EnvironmentStore-owned Python build remains the sole final-path writer;
/// the second caller joins it while the materialization gate is held.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropped_prepare_waiter_does_not_cancel_python_build_owner() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let gate = conversation.backend.install_materialize_gate();
    let first_coordinator = conversation.coordinator.clone();
    let first = tokio::spawn(async move { first_coordinator.prepare_candidate().await });
    gate.await_entered().await;

    first.abort();
    assert!(
        first
            .await
            .expect_err("caller A was aborted")
            .is_cancelled()
    );
    assert_eq!(
        conversation
            .backend
            .materialization_count(Ecosystem::Python),
        1,
        "aborting the initiating caller does not drop the physical build"
    );

    let mut second = Box::pin(conversation.coordinator.prepare_candidate());
    let waker = futures_util::task::noop_waker_ref();
    let mut context = std::task::Context::from_waker(waker);
    assert!(
        matches!(second.as_mut().poll(&mut context), std::task::Poll::Pending),
        "caller B joins the gated in-flight build"
    );
    assert_eq!(
        conversation
            .backend
            .materialization_count(Ecosystem::Python),
        1,
        "joining a live build never creates a second final-path writer"
    );
    let store_root = conversation.dir.path().join("skill-env").join("python");
    assert!(
        std::fs::read_dir(&store_root)
            .expect("python store")
            .filter_map(Result::ok)
            .all(|entry| {
                !entry
                    .path()
                    .join(rustx::skills::ENVIRONMENT_MANIFEST_FILE)
                    .exists()
            }),
        "the gated Python final path is not reusable before publication"
    );

    gate.release();
    let candidate = second.await.expect("caller B prepare");
    assert!(candidate.python_environment().is_some());
    assert_eq!(
        conversation
            .backend
            .materialization_count(Ecosystem::Python),
        1
    );
}

/// An EnvironmentStore-owned same-digest build failure is shared by a waiter
/// even after the initiating caller is gone. Retry ownership becomes legal
/// only after the failed physical writer has returned and published failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropped_build_waiter_cannot_start_an_early_retry() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    conversation
        .backend
        .fail_python_materialization("injected same-digest failure");
    let gate = conversation.backend.install_materialize_gate();
    let first_coordinator = conversation.coordinator.clone();
    let first = tokio::spawn(async move { first_coordinator.prepare_candidate().await });
    gate.await_entered().await;

    first.abort();
    assert!(
        first
            .await
            .expect_err("caller A was aborted")
            .is_cancelled()
    );
    let mut second = Box::pin(conversation.coordinator.prepare_candidate());
    let waker = futures_util::task::noop_waker_ref();
    let mut context = std::task::Context::from_waker(waker);
    assert!(
        matches!(second.as_mut().poll(&mut context), std::task::Poll::Pending),
        "caller B remains attached to the live failed build"
    );
    assert_eq!(
        conversation
            .backend
            .materialization_count(Ecosystem::Python),
        1,
        "no retry writer starts while the original materialization is gated"
    );
    gate.release();
    assert!(second.await.is_err());
    assert_eq!(
        conversation
            .backend
            .materialization_count(Ecosystem::Python),
        1
    );
    let retry = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect("retry can build after the failed owner");
    assert!(!retry.skill_packages().is_empty());
    assert_eq!(
        conversation
            .backend
            .materialization_count(Ecosystem::Python),
        2
    );
}

#[test]
fn environment_store_inside_workspace_is_rejected_before_creation() {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let workspace = Workspace::new(&workspace_root).expect("workspace");
    let store = workspace.root().join("private-env");
    let Err(error) = CapabilityCoordinator::with_backend(
        CapabilityCoordinatorConfig {
            conversation_id: ConversationId::new("conv-isolation"),
            workspace,
            base_tool_registry: Arc::new(ToolRegistry::new()),
            tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
            skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: ToolEnvironment::new(),
            environment_store_root: store.clone(),
        },
        Arc::new(common::FakeSkillEnvironmentBackend::new()),
    ) else {
        panic!("nested store must be rejected")
    };
    assert!(matches!(
        error,
        CapabilityPreparationError::EnvironmentStoreOverlapsWorkspace { .. }
    ));
    assert!(!store.exists(), "rejected creation must leave no store");
}

#[test]
fn workspace_inside_environment_store_is_rejected() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join("private-env");
    let workspace_root = store.join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let workspace = Workspace::new(&workspace_root).expect("workspace");
    let Err(error) = CapabilityCoordinator::with_backend(
        CapabilityCoordinatorConfig {
            conversation_id: ConversationId::new("conv-isolation"),
            workspace,
            base_tool_registry: Arc::new(ToolRegistry::new()),
            tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
            skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: ToolEnvironment::new(),
            environment_store_root: store,
        },
        Arc::new(common::FakeSkillEnvironmentBackend::new()),
    ) else {
        panic!("workspace containing the store must be rejected")
    };
    assert!(matches!(
        error,
        CapabilityPreparationError::EnvironmentStoreOverlapsWorkspace { .. }
    ));
}

#[test]
fn external_environment_store_is_accepted() {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let workspace = Workspace::new(&workspace_root).expect("workspace");
    let store = dir.path().join("external").join("private-env");
    let coordinator = CapabilityCoordinator::with_backend(
        CapabilityCoordinatorConfig {
            conversation_id: ConversationId::new("conv-isolation"),
            workspace,
            base_tool_registry: Arc::new(ToolRegistry::new()),
            tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
            skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: ToolEnvironment::new(),
            environment_store_root: store.clone(),
        },
        Arc::new(common::FakeSkillEnvironmentBackend::new()),
    )
    .expect("external store");
    assert!(
        coordinator
            .current_snapshot()
            .workspace_root()
            .is_absolute()
    );
    assert!(store.is_dir());
}

#[cfg(unix)]
#[test]
fn symlink_prefix_environment_store_is_rejected_before_creation() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    std::fs::create_dir_all(&outside).expect("outside");
    symlink(&workspace_root, outside.join("link")).expect("link");
    let workspace = Workspace::new(&workspace_root).expect("workspace");
    let configured = outside.join("link/private-env");
    let Err(error) = CapabilityCoordinator::with_backend(
        CapabilityCoordinatorConfig {
            conversation_id: ConversationId::new("conv-isolation"),
            workspace,
            base_tool_registry: Arc::new(ToolRegistry::new()),
            tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
            skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: ToolEnvironment::new(),
            environment_store_root: configured,
        },
        Arc::new(common::FakeSkillEnvironmentBackend::new()),
    ) else {
        panic!("symlink-prefix escape must be rejected")
    };
    assert!(matches!(
        error,
        CapabilityPreparationError::EnvironmentStoreOverlapsWorkspace { .. }
    ));
    assert!(
        !workspace_root.join("private-env").exists(),
        "rejected symlink-prefix configuration must not create inside Workspace"
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
    assert_eq!(snapshot.skill_catalog(), None);
    assert_eq!(
        committed.skill_catalog().as_deref(),
        Some(concat!(
            "## Skills\n\n",
            "The following skills provide specialized instructions for specific tasks.\n",
            "Use the Read tool to load a skill when the task matches its description.\n",
            "Use the exact location shown below; do not construct or rewrite Skill paths.\n\n",
            "<available_skills>\n",
            "  <skill>\n",
            "    <name>pdf</name>\n",
            "    <description>PDF skill.</description>\n",
            "    <location>.rustx/skills/pdf/SKILL.md</location>\n",
            "  </skill>\n",
            "</available_skills>"
        )),
        "a later capability revision owns its own catalog rather than inheriting history"
    );
    // The next attempt snapshots the new revision.
    let next_lease = conversation.coordinator.acquire_attempt_lease();
    assert_eq!(next_lease.revision(), committed.revision());
    assert_eq!(
        next_lease.snapshot().skill_catalog(),
        committed.skill_catalog()
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
    release: tokio::sync::watch::Sender<bool>,
}

impl RecordingParkingExecutor {
    fn new() -> (
        Self,
        tokio::sync::watch::Receiver<bool>,
        tokio::sync::watch::Sender<bool>,
    ) {
        let (seen, seen_rx) = tokio::sync::watch::channel(false);
        let (release, _release_rx) = tokio::sync::watch::channel(false);
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
        let started = self.seen.clone();
        let mut release = self.release.subscribe();
        Box::pin(async move {
            started.send_replace(true);
            release
                .wait_for(|released| *released)
                .await
                .expect("background release channel stays open");
            ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
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
            None,
        )
        .expect("prepare");
    let outcome = conversation
        .background
        .commit_dispatch(prepared, &rustx::runtime::CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
        panic!("accepted");
    };
    await_background_started(&mut started, "background execution under revision N").await;
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
            None,
        )
        .expect("prepare");
    let outcome2 = conversation
        .background
        .commit_dispatch(prepared2, &rustx::runtime::CancellationSignal::new())
        .expect("dispatch commits");
    let BackgroundDispatchOutcome::Accepted {
        execution_id: id2, ..
    } = outcome2
    else {
        panic!("accepted");
    };
    await_background_started(&mut started2, "second background execution").await;
    assert_eq!(
        executor2
            .recorded_environment()
            .expect("captured environment"),
        snapshot_n1.effective_environment().clone(),
        "a new background execution uses environment N+1"
    );

    // Release both executions; each settles.
    release.send_replace(true);
    release2.send_replace(true);
    wait_for_terminal(&conversation, &execution_id).await;
    wait_for_terminal(&conversation, &id2).await;
    drop(lease_n1);
}

async fn wait_for_terminal(conversation: &Conversation, execution_id: &ToolExecutionId) {
    tokio::time::timeout(
        std::time::Duration::from_secs(120),
        conversation.background.wait_until_terminal(execution_id),
    )
    .await
    .expect("background terminal wait exceeded liveness guard")
    .expect("background execution must remain registered");
}

async fn await_background_started(
    started: &mut tokio::sync::watch::Receiver<bool>,
    description: &'static str,
) {
    tokio::time::timeout(
        std::time::Duration::from_secs(120),
        started.wait_for(|is_started| *is_started),
    )
    .await
    .unwrap_or_else(|_| panic!("{description}: start wait exceeded liveness guard"))
    .expect("background start channel stays open");
}

// ---------------------------------------------------------------------------
// Agent loop integration: the attempt uses the snapshot's catalog and
// environment on every turn (sections 22/31/33)
// ---------------------------------------------------------------------------

/// Every model turn of one attempt carries the exact same Skill catalog in the
/// Effective System Prompt and the same effective environment: the attempt
/// runs multiple turns while its lease is held, and the catalog never changes
/// mid-attempt.
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
    // The test also needs the fake tool registered, so it builds its own
    // coordinator over the same workspace/store while retaining native Read.
    let mut tools = ToolRegistry::new();
    register_native_tools(
        &mut tools,
        NativeToolResources {
            background: conversation.background.clone(),
            subagents: None,
        },
        NativeToolPolicies::default(),
    )
    .expect("native tools");
    let fake_tool = support::fake::FakeTool::new(
        common::tool("alpha", "tool-alpha"),
        support::fake::success_result("ok"),
    );
    fake_tool.register(&mut tools);
    let tools = Arc::new(tools);
    let coordinator = CapabilityCoordinator::with_backend(
        CapabilityCoordinatorConfig {
            conversation_id: ConversationId::new("conv-m6"),
            workspace: conversation.workspace.clone(),
            base_tool_registry: tools.clone(),
            tool_activation: rustx::capabilities::ToolActivationPolicy::default(),
            skill_discovery: rustx::skills::SkillDiscoveryConfig {
                automatic_roots: vec![conversation.workspace.root().join(".agents/skills")],
                explicit_paths: Vec::new(),
            },
            mcp_servers: std::collections::BTreeMap::new(),
            base_environment: ToolEnvironment::new(),
            environment_store_root: conversation.dir.path().join("skill-env-2"),
        },
        Arc::new(conversation.backend.clone()),
    )
    .expect("coordinator");
    let snapshot = prepare_and_commit(&coordinator).await;
    let lease = coordinator.acquire_attempt_lease();
    let catalog = snapshot.skill_catalog().expect("catalog").clone();

    // A two-turn model script: turn 1 is a tool-call turn, turn 2 stops.
    let call = support::fake::ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let mut first = vec![support::fake::FakeStep::Emit(
        rustx::model::ModelEvent::Started,
    )];
    for event in support::fake::tool_call_events(0, &call) {
        first.push(support::fake::FakeStep::Emit(event));
    }
    first.push(support::fake::FakeStep::Emit(
        rustx::model::ModelEvent::Completed {
            finish_reason: rustx::model::ModelFinishReason::ToolCalls,
            usage: None,
        },
    ));
    let model = support::fake::fake_model(vec![
        first,
        vec![
            support::fake::FakeStep::Emit(rustx::model::ModelEvent::Started),
            support::fake::FakeStep::Emit(rustx::model::ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            support::fake::FakeStep::Emit(rustx::model::ModelEvent::Completed {
                finish_reason: rustx::model::ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ]);
    let tool_runtime = rustx::tools::runtime::ConversationToolRuntime::new(
        ConversationId::new("conv-m6"),
        conversation.workspace.root(),
        conversation.dir.path().join("agent-artifacts"),
    )
    .expect("tool runtime");
    let cancellation = rustx::agent::AgentCancellation::new(
        rustx::runtime::types::CancellationReason::UserRequested,
    );
    let request = rustx::agent::AgentExecutionRequest {
        agent_id: rustx::runtime::identity::AgentId::new("agent-1"),
        conversation_id: rustx::runtime::identity::ConversationId::new("conv-m6"),
        attempt_id: rustx::runtime::identity::AttemptId::new("attempt-1"),
        conversation: rustx::conversation::ConversationState::new(),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: support::attempt_model(model.clone(), "fake-model"),
    };
    let runtime = rustx::context::ContextRuntime::for_attempt(
        rustx::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(rustx::context::DefaultTokenEstimator),
        rustx::context::AgentStatusComposer::default(),
        &request.model,
    )
    .expect("context runtime");
    let result = rustx::agent::AgentExecution::new(
        request,
        lease,
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
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
            request.effective_system_prompt, catalog,
            "the attempt's immutable Skill snapshot renders into the Effective System Prompt"
        );
        assert!(
            !request.messages.iter().any(|message| {
                serde_json::to_string(message)
                    .expect("serialize canonical message")
                    .contains("## Skills")
            }),
            "Skill routing metadata never enters canonical conversation messages"
        );
        assert!(
            !request.effective_system_prompt.contains("body"),
            "the initial system catalog never contains a full SKILL.md body"
        );
    }
    assert!(
        result.messages().iter().all(|message| {
            !serde_json::to_string(message)
                .expect("serialize")
                .contains("## Skills")
        }),
        "the committed ledger contains no Skill catalog fact"
    );
    assert_eq!(
        coordinator.active_attempts(),
        0,
        "settling the execution releases its owned attempt lease"
    );
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill revision two.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let next_candidate = coordinator
        .prepare_candidate()
        .await
        .expect("prepare the next capability revision");
    assert!(
        coordinator.commit(next_candidate).is_ok(),
        "a settled execution immediately permits the next capability commit"
    );
}
