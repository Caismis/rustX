//! Deterministic tests: capability snapshots, quiescent commits, and
//! background environment retention.
//!
//! Skill discovery publishes instruction metadata only. These tests never
//! materialize a Skill Python or Node environment. Race semantics use exact
//! synchronization points (watches, notify gates, registry state), never sleeps.

use std::sync::{Arc, Mutex};

use rustx::capabilities::{
    CapabilityCoordinator, CapabilityCoordinatorConfig, CapabilityPreparationError,
};
use rustx::runtime::identity::{ConversationId, ToolCallId, ToolExecutionId, ToolId};
use rustx::runtime::inbound::ConversationInboundMailbox;
use rustx::tools::ToolProgressCapability;
use rustx::tools::artifacts::ArtifactStore;
use rustx::tools::background::{
    BackgroundDispatchOutcome, BackgroundResources, ConversationBackgroundRegistry,
};
use rustx::tools::environment::ToolEnvironment;
use rustx::tools::executor::{
    ToolExecutionContext, ToolExecutionHandle, ToolExecutor, ToolRegistry,
};
use rustx::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode,
};
use rustx::tools::workspace::Workspace;
use rustx::tools::{NativeToolPolicies, NativeToolResources, register_native_tools};

use super::super::{common, support};

/// One conversation fixture: workspace (with skills), environment store,
/// artifact store, mailbox, background registry, and the coordinator.
struct Conversation {
    dir: tempfile::TempDir,
    pub workspace: Workspace,
    pub background: ConversationBackgroundRegistry,
    pub coordinator: CapabilityCoordinator,
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

/// The host `SKILL.md` location the catalog publishes for a fixture Skill.
fn skill_location(root: &std::path::Path, name: &str) -> String {
    root.join(".agents/skills")
        .join(name)
        .join("SKILL.md")
        .to_string_lossy()
        .into_owned()
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

/// This fixture explicitly admits Read and all eligible Skill metadata.
fn fixture_activation() -> rustx::capabilities::AgentActivation {
    let mut selection = rustx::capabilities::AgentActivation::default();
    selection.profile.tools.builtin = vec!["read".into()];
    selection.profile.skills = Some(rustx::runtime::agent_profile::AgentSkillSelection::All);
    selection
}

fn conversation() -> Conversation {
    conversation_with_options(fixture_activation())
}

fn conversation_with_options(
    agent_activation: rustx::capabilities::AgentActivation,
) -> Conversation {
    conversation_with(agent_activation, |_, workspace| {
        rustx::skills::SkillDiscoveryConfig::workspace_root(workspace.root().join(".agents/skills"))
    })
}

/// The fixture with an explicit Skill source authority (#280). The `home`
/// argument is the fixture's *isolated* temporary home: no test resolves the
/// global source against the developer's real home directory.
fn conversation_with(
    agent_activation: rustx::capabilities::AgentActivation,
    skill_discovery: impl FnOnce(&std::path::Path, &Workspace) -> rustx::skills::SkillDiscoveryConfig,
) -> Conversation {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let workspace = Workspace::new(&workspace_root).expect("workspace");
    let conversation_id = ConversationId::new("conv_6c966d7f-bdde-792f-8692-51542a5c1834");
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
    let mut base_tool_registry = ToolRegistry::new();
    register_native_tools(
        &mut base_tool_registry,
        NativeToolResources {
            subagent_catalog: rustx::runtime::subagent::AgentCatalog::empty(),
            background: background.clone(),
            subagents: None,
        },
        NativeToolPolicies::default(),
    )
    .expect("native tools");
    let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        source_demand: rustx::capabilities::source::ToolSourceDemand::default(),
        conversation_id: conversation_id.clone(),
        workspace: workspace.clone(),
        base_tool_registry: Arc::new(base_tool_registry),
        extension_tools: rustx::extensions::ExtensionToolPlane::none(),
        agent_activation,
        skill_discovery: skill_discovery(dir.path(), &workspace),
        mcp_servers: std::collections::BTreeMap::new(),
        base_environment: ToolEnvironment::new(),
        environment_store_root: dir.path().join("skill-env"),
    })
    .expect("coordinator");
    Conversation {
        dir,
        workspace,
        background,
        coordinator,
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
    let visible_location = skill_location(conversation.workspace.root(), "visible");
    assert_eq!(snapshot.skills().catalog_entries()[0].name, "visible");
    assert_eq!(
        snapshot.skills().catalog_entries()[0].location,
        visible_location
    );
    let rendered_catalog = snapshot.skill_catalog().expect("visible Skill catalog");
    assert_eq!(rendered_catalog.matches("## Skills").count(), 1);
    assert!(rendered_catalog.contains("visible"));
    assert!(rendered_catalog.contains("<description>Visible guidance.</description>"));
    assert!(
        rendered_catalog.contains(
            conversation
                .workspace
                .root()
                .join(".agents/skills")
                .to_str()
                .unwrap()
        )
    );
    assert!(rendered_catalog.contains(&visible_location));
    assert!(!rendered_catalog.contains("runtime-only"));
    // The hidden Skill stays in the snapshot with its own host location, but
    // never reaches the model-visible catalog.
    assert_eq!(
        snapshot.skills().locations(),
        vec![
            skill_location(conversation.workspace.root(), "runtime-only"),
            visible_location.clone()
        ]
    );

    let manifest = snapshot.to_capabilities_manifest();
    let manifest_names = manifest
        .skills
        .iter()
        .map(|binding| binding.skill_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(manifest_names, vec!["runtime-only", "visible"]);

    let client_view = crate::runtime_client::projection::views::capability_view(
        &snapshot,
        &crate::capabilities::CapabilityAvailability::new(),
        None,
    );
    assert_eq!(
        client_view
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["visible"],
        "Runtime Client Skills are the model-visible projection"
    );
    assert_eq!(client_view.skills[0].location, visible_location);
}

/// The reader controls prompt exposure without changing the frozen inventory.
#[tokio::test]
async fn skill_prompt_uses_the_available_reader() {
    for (tools, hint) in [
        (vec![], None),
        (vec!["write"], None),
        (vec!["bash"], Some("Use bash to load")),
        (vec!["read", "bash"], Some("Use the read tool to load")),
    ] {
        let mut policy = fixture_activation();
        policy.profile.tools.builtin = tools.into_iter().map(str::to_owned).collect();
        let conversation = conversation_with_options(policy);
        write_skill(
            conversation.workspace.root(),
            "visible",
            "Visible guidance.",
            &[],
        );
        let snapshot = prepare_and_commit(&conversation.coordinator).await;
        let location = skill_location(conversation.workspace.root(), "visible");
        assert_eq!(snapshot.skills().bindings().len(), 1);
        assert_eq!(snapshot.skills().catalog_entries()[0].location, location);
        match hint {
            Some(hint) => {
                let prompt = snapshot.skill_catalog().expect("readable Skill prompt");
                assert!(prompt.contains(hint));
                assert!(prompt.contains(&location));
            }
            None => assert!(snapshot.skill_catalog().is_none()),
        }
    }
}

// ---------------------------------------------------------------------------
// Skill sources, merge, diagnostics, and freezing (Issue #280)
// ---------------------------------------------------------------------------

/// A conversation whose automatic sources are an isolated global root plus
/// the workspace root. The global root is never the developer's real home.
fn conversation_with_sources(
    activation: rustx::capabilities::AgentActivation,
    home_name: &'static str,
) -> (Conversation, std::path::PathBuf) {
    let mut home = std::path::PathBuf::new();
    let conversation = conversation_with(activation, |root, workspace| {
        home = root.join(home_name);
        rustx::skills::SkillDiscoveryConfig {
            automatic: rustx::skills::automatic_skill_roots(Some(&home), workspace.root()),
        }
    });
    (conversation, home.join("rustx"))
}

/// #280 (6)(7)(9)(11)(13)(14): the whole ownership chain proven through the
/// real candidate/commit owner — validation, exclusion, the
/// `workspace > global` merge, provenance, root visibility, and the fact
/// that a root deny-list removes nothing from the committed catalog.
#[tokio::test]
async fn cfg280_the_committed_generation_owns_sources_merge_and_root_visibility() {
    let (conversation, home) = conversation_with_sources(
        rustx::capabilities::AgentActivation {
            profile: rustx::local_runtime::config::AgentProfileDocument {
                skills: Some(rustx::runtime::agent_profile::AgentSkillSelection::Exact(
                    vec!["rust-review".into(), "shared".into()],
                )),
                tools: fixture_activation().profile.tools,
                ..rustx::local_runtime::config::builtin_root_profile()
            },
            ..Default::default()
        },
        "home",
    );
    write_skill(&home, "legacy-java", "Legacy Java guidance.", &[]);
    write_skill(&home, "shared", "Global shared guidance.", &[]);
    write_skill(
        conversation.workspace.root(),
        "shared",
        "Workspace shared guidance.",
        &[],
    );
    write_skill(conversation.workspace.root(), "rust-review", "Review.", &[]);
    // One malformed package in the workspace source.
    let broken = conversation.workspace.root().join(".agents/skills/broken");
    std::fs::create_dir_all(&broken).expect("broken package");
    std::fs::write(broken.join("SKILL.md"), "not frontmatter\n").expect("broken SKILL.md");

    let snapshot = prepare_and_commit(&conversation.coordinator).await;

    // (11): the malformed package is excluded; everything else publishes.
    assert_eq!(
        snapshot
            .skills()
            .catalog_entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["legacy-java", "rust-review", "shared"]
    );
    // (13): workspace shadows global for the same identity.
    let shared = snapshot
        .skills()
        .packages()
        .iter()
        .find(|package| package.name() == "shared")
        .expect("shared package");
    assert_eq!(shared.source(), rustx::skills::SkillSource::Workspace);
    assert_eq!(shared.description(), "Workspace shared guidance.");
    // (14): the shadow is retained as generation provenance, not model input.
    let provenance = snapshot
        .skills()
        .provenance()
        .iter()
        .find(|entry| entry.name == "shared")
        .expect("shared provenance");
    assert_eq!(provenance.shadowed.len(), 1);
    assert_eq!(
        provenance.shadowed[0].source,
        rustx::skills::SkillSource::User
    );
    let rendered = snapshot.skill_catalog().expect("catalog");
    assert!(!rendered.contains("<name>shadow</name>"));
    assert!(!rendered.contains("Global shared guidance."));

    // Typed, canonically ordered diagnostics travel with the generation.
    let diagnostics = snapshot.skills().diagnostics();
    assert!(
        diagnostics
            .iter()
            .any(|fact| matches!(fact, rustx::skills::SkillDiagnostic::PackageInvalid { .. })),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|fact| matches!(fact, rustx::skills::SkillDiagnostic::Shadowed { .. })),
        "{diagnostics:?}"
    );
    let mut sorted = diagnostics.to_vec();
    sorted.sort();
    assert_eq!(sorted, diagnostics, "diagnostics must already be canonical");

    // (6)(7): the root sees the eligible catalog minus its deny-list, and
    // the denied Skill is still owned by the committed generation.
    let profile = snapshot.resolved_profile().expect("root profile");
    assert_eq!(profile.skills, ["rust-review", "shared"]);
    assert!(
        snapshot
            .skills()
            .packages()
            .iter()
            .any(|package| package.name() == "legacy-java")
    );
    assert!(
        snapshot
            .skills()
            .bindings()
            .iter()
            .any(|binding| binding.skill_id.as_str() == "legacy-java")
    );
    assert_eq!(
        snapshot
            .model_skill_entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["rust-review", "shared"]
    );
}

#[tokio::test]
async fn cfg332_both_fixed_roots_are_discovered_and_missing_roots_are_empty() {
    let (conversation, home) = conversation_with_sources(fixture_activation(), "home");
    let missing = prepare_and_commit(&conversation.coordinator).await;
    assert!(missing.skills().packages().is_empty());
    write_skill(&home, "user-only", "User.", &[]);
    write_skill(
        conversation.workspace.root(),
        "workspace-only",
        "Workspace.",
        &[],
    );
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(
        snapshot
            .skills()
            .catalog_entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["user-only", "workspace-only"]
    );
}

/// #280 (18)(19)(20)(21): a later generation changes future work only.
///
/// The attempt admitted against R1 keeps R1's frozen Skill identity,
/// version, and location after R2 publishes; a failed candidate leaves R1
/// completely authoritative; and no `SKILL.md` body is ever injected. The
/// proof is exact ownership — a held attempt lease — never a sleep.
#[tokio::test]
async fn cfg280_a_later_generation_never_mutates_an_admitted_attempt() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pinned",
        "First generation guidance.",
        &[],
    );
    std::fs::write(
        conversation
            .workspace
            .root()
            .join(".agents/skills/pinned/SKILL.md"),
        "---\nname: pinned\ndescription: First generation guidance.\n---\nR1-BODY-SENTINEL\n",
    )
    .expect("first generation body");
    let first = prepare_and_commit(&conversation.coordinator).await;
    let lease = conversation.coordinator.acquire_attempt_lease();
    let pinned = lease.snapshot().clone();
    let frozen_bindings = pinned.skills().bindings().to_vec();
    assert_eq!(frozen_bindings.len(), 1);
    assert!(
        !pinned
            .skill_catalog()
            .expect("catalog")
            .contains("R1-BODY-SENTINEL"),
        "a committed catalog is metadata plus a location, never a Skill body"
    );

    // Edit the package and add a second one: later generation material.
    std::fs::write(
        conversation
            .workspace
            .root()
            .join(".agents/skills/pinned/SKILL.md"),
        "---\nname: pinned\ndescription: Second generation guidance.\n---\nR2-BODY-SENTINEL\n",
    )
    .expect("second generation body");
    write_skill(conversation.workspace.root(), "late", "Late guidance.", &[]);

    let candidate = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect("prepare");
    // (18): preparing the later generation cannot touch the admitted attempt.
    assert_eq!(lease.snapshot().as_ref(), &*pinned);
    assert_eq!(lease.snapshot().skills().bindings(), frozen_bindings);
    // (19): the commit boundary is unchanged — a busy conversation cannot
    // activate a new revision, and the active revision stays authoritative.
    assert!(conversation.coordinator.commit(candidate).is_err());
    assert_eq!(
        conversation.coordinator.current_snapshot().revision(),
        first.revision()
    );
    drop(lease);

    let second = prepare_and_commit(&conversation.coordinator).await;
    assert_ne!(second.revision(), first.revision());
    assert_eq!(second.skills().packages().len(), 2);
    assert_ne!(second.skills().bindings(), frozen_bindings);
    // (21): neither generation's catalog carries a body.
    for snapshot in [&first, &second] {
        let catalog = snapshot.skill_catalog().expect("catalog");
        assert!(!catalog.contains("R1-BODY-SENTINEL"));
        assert!(!catalog.contains("R2-BODY-SENTINEL"));
    }
    // The first snapshot value itself still describes the first generation.
    assert_eq!(first.skills().bindings(), frozen_bindings);
    assert_eq!(
        first.skills().catalog_entries()[0].description,
        "First generation guidance."
    );
}

/// #280: a rediscovery is a publication no-op only when the **complete**
/// generation is unchanged — executable Skill semantics *and* every
/// generation-scoped Skill fact.
///
/// Both halves are proven through the real candidate/commit owner:
///
/// - a *diagnostics-only* change: a newly added malformed package excludes
///   itself, so the effective catalog, bindings, and locations are identical
///   while the generation now owns a typed exclusion fact;
/// - a *provenance-only* change: the lower-precedence global source starts
///   offering an identity the workspace source already won, so the winner, its
///   version binding, and its published location are identical while the
///   generation now owns shadowing provenance.
///
/// A byte-for-byte identical rediscovery stays a true no-op, and neither new
/// fact is model-visible.
#[tokio::test]
async fn cfg280_generation_scoped_skill_facts_are_published_not_collapsed() {
    for (change, diagnostics_only) in [("diagnostics", true), ("provenance", false)] {
        let (conversation, home) = conversation_with_sources(fixture_activation(), "home");
        write_skill(conversation.workspace.root(), "foo", "Foo guidance.", &[]);
        let first = prepare_and_commit(&conversation.coordinator).await;
        assert_eq!(first.skills().catalog_entries().len(), 1);
        assert!(first.skills().diagnostics().iter().all(|fact| matches!(
            fact,
            rustx::skills::SkillDiagnostic::SourceRootMissing { .. }
        )));
        assert!(first.skills().provenance()[0].shadowed.is_empty());

        // An identical rediscovery is a true no-op: no revision is fabricated.
        let unchanged = prepare_and_commit(&conversation.coordinator).await;
        assert_eq!(
            unchanged.revision(),
            first.revision(),
            "{change}: a byte-for-byte identical generation must not publish"
        );

        if diagnostics_only {
            // A newly added malformed package excludes itself: one new typed
            // fact, and nothing an execution can observe.
            let broken = conversation.workspace.root().join(".agents/skills/broken");
            std::fs::create_dir_all(&broken).expect("broken package");
            std::fs::write(broken.join("SKILL.md"), "not frontmatter\n").expect("broken SKILL.md");
        } else {
            // The lower-precedence global source starts offering the identity
            // the workspace source already won: new shadowing provenance, and
            // the same winner, binding, and location.
            write_skill(&home, "foo", "Foo guidance.", &[]);
        }

        // A previously admitted attempt stays pinned to the old generation.
        let lease = conversation.coordinator.acquire_attempt_lease();
        let pinned = lease.snapshot().clone();
        let candidate = conversation
            .coordinator
            .prepare_candidate()
            .await
            .expect("prepare");
        assert_eq!(pinned.revision(), first.revision(), "{change}");
        drop(lease);
        let second = conversation
            .coordinator
            .commit(candidate)
            .expect("commit")
            .as_ref()
            .clone();

        // The executable Skill semantics are unchanged...
        assert!(
            second.skills().semantically_equivalent(first.skills()),
            "{change}: this case must not change what an execution observes"
        );
        assert_eq!(second.skills().bindings(), first.skills().bindings());
        assert_eq!(
            second.skills().catalog_entries(),
            first.skills().catalog_entries()
        );
        assert_eq!(second.skills().locations(), first.skills().locations());
        // ...and the generation is still published, because a generation-scoped
        // Skill fact changed.
        assert!(
            !second.skills().publication_equivalent(first.skills()),
            "{change}"
        );
        assert_ne!(
            second.revision(),
            first.revision(),
            "{change}: a changed generation fact must publish a new revision"
        );
        if diagnostics_only {
            assert!(
                second.skills().diagnostics().iter().any(|fact| matches!(
                    fact,
                    rustx::skills::SkillDiagnostic::PackageInvalid { .. }
                )),
                "{:?}",
                second.skills().diagnostics()
            );
            assert_eq!(second.skills().provenance(), first.skills().provenance());
        } else {
            let provenance = &second.skills().provenance()[0];
            assert_eq!(provenance.source, rustx::skills::SkillSource::Workspace);
            assert_eq!(
                provenance.shadowed[0].source,
                rustx::skills::SkillSource::User
            );
            assert!(
                second
                    .skills()
                    .diagnostics()
                    .iter()
                    .any(|fact| matches!(fact, rustx::skills::SkillDiagnostic::Shadowed { .. }))
            );
        }

        // The old lease value still describes the old generation, while a
        // newly admitted lease observes the new one.
        assert_eq!(pinned.revision(), first.revision(), "{change}");
        assert_eq!(pinned.skills().diagnostics(), first.skills().diagnostics());
        assert_eq!(pinned.skills().provenance(), first.skills().provenance());
        let later = conversation.coordinator.acquire_attempt_lease();
        assert_eq!(later.snapshot().revision(), second.revision(), "{change}");
        drop(later);

        // Neither generation fact is model-visible.
        let rendered = second.skill_catalog().expect("catalog");
        assert!(!rendered.contains("<name>broken</name>"), "{change}");
        assert!(!rendered.contains("<name>shadow</name>"), "{change}");
        let view = crate::runtime_client::projection::views::capability_view(
            &second,
            &crate::capabilities::CapabilityAvailability::new(),
            None,
        );
        assert_eq!(
            view.skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            ["foo"],
            "{change}"
        );
    }
}

#[test]
fn environment_store_inside_workspace_is_rejected_before_creation() {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let workspace = Workspace::new(&workspace_root).expect("workspace");
    let store = workspace.root().join("private-env");
    let Err(error) = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        source_demand: rustx::capabilities::source::ToolSourceDemand::default(),
        conversation_id: ConversationId::new("conv_d1c8ffd4-e55a-7688-84ec-b44b65b846f7"),
        workspace,
        base_tool_registry: Arc::new(ToolRegistry::new()),
        extension_tools: rustx::extensions::ExtensionToolPlane::none(),
        agent_activation: fixture_activation(),
        skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
        mcp_servers: std::collections::BTreeMap::new(),
        base_environment: ToolEnvironment::new(),
        environment_store_root: store.clone(),
    }) else {
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
    let Err(error) = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        source_demand: rustx::capabilities::source::ToolSourceDemand::default(),
        conversation_id: ConversationId::new("conv_d1c8ffd4-e55a-7688-84ec-b44b65b846f7"),
        workspace,
        base_tool_registry: Arc::new(ToolRegistry::new()),
        extension_tools: rustx::extensions::ExtensionToolPlane::none(),
        agent_activation: fixture_activation(),
        skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
        mcp_servers: std::collections::BTreeMap::new(),
        base_environment: ToolEnvironment::new(),
        environment_store_root: store,
    }) else {
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
    let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        source_demand: rustx::capabilities::source::ToolSourceDemand::default(),
        conversation_id: ConversationId::new("conv_d1c8ffd4-e55a-7688-84ec-b44b65b846f7"),
        workspace,
        base_tool_registry: Arc::new(ToolRegistry::new()),
        extension_tools: rustx::extensions::ExtensionToolPlane::none(),
        agent_activation: fixture_activation(),
        skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
        mcp_servers: std::collections::BTreeMap::new(),
        base_environment: ToolEnvironment::new(),
        environment_store_root: store.clone(),
    })
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
    let Err(error) = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        source_demand: rustx::capabilities::source::ToolSourceDemand::default(),
        conversation_id: ConversationId::new("conv_d1c8ffd4-e55a-7688-84ec-b44b65b846f7"),
        workspace,
        base_tool_registry: Arc::new(ToolRegistry::new()),
        extension_tools: rustx::extensions::ExtensionToolPlane::none(),
        agent_activation: fixture_activation(),
        skill_discovery: rustx::skills::SkillDiscoveryConfig::default(),
        mcp_servers: std::collections::BTreeMap::new(),
        base_environment: ToolEnvironment::new(),
        environment_store_root: configured,
    }) else {
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

/// Conflicting dependency strings are inert metadata. Both Skills publish and
/// the effective environment stays the base environment.
#[tokio::test]
async fn conflicting_dependency_metadata_does_not_block_publication() {
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
        &[python_deps(r#"{"pypdf":"5.10.0"}"#), node_deps("not-json")],
    );
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(
        snapshot
            .skills()
            .catalog_entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(snapshot.effective_environment(), &ToolEnvironment::new());
    assert!(!conversation.dir.path().join("skill-env/python").exists());
    assert!(!conversation.dir.path().join("skill-env/node").exists());
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
    let prompt = committed.skill_catalog().unwrap();
    assert!(prompt.contains("<name>pdf</name>"));
    assert!(
        prompt.contains(
            conversation
                .workspace
                .root()
                .join(".agents/skills")
                .to_str()
                .unwrap()
        )
    );
    assert!(
        prompt.contains(&skill_location(conversation.workspace.root(), "pdf")),
        "prompt lists the exact file path"
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

/// A description change and uninterpreted dependency metadata each publish a
/// new Skill definition. Neither changes the effective tool environment.
#[tokio::test]
async fn description_or_dependency_metadata_keeps_the_same_environment() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "First description.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let snapshot = prepare_and_commit(&conversation.coordinator).await;

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
        second.effective_environment(),
        snapshot.effective_environment()
    );

    write_skill(
        conversation.workspace.root(),
        "pdf",
        "Second description.",
        &[python_deps(r#"{"pypdf":"5.10.0"}"#)],
    );
    let third = prepare_and_commit(&conversation.coordinator).await;
    assert_ne!(third.skills().bindings(), second.skills().bindings());
    assert_eq!(
        third.effective_environment(),
        snapshot.effective_environment(),
        "dependency metadata does not change the tool environment"
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

/// Conflicting dependency text prepares and publishes. Preparation itself
/// does not move the revision; only the commit does. The tool environment
/// stays the one already published.
#[tokio::test]
async fn conflicting_dependency_text_does_not_fail_preparation() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "a",
        "Skill A.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    write_skill(
        conversation.workspace.root(),
        "b",
        "Skill B.",
        &[python_deps(r#"{"pypdf":"5.10.0"}"#)],
    );
    let prepared = conversation
        .coordinator
        .prepare_candidate()
        .await
        .expect("conflicting dependency text still prepares");
    assert_eq!(
        conversation.coordinator.current_snapshot().revision(),
        snapshot.revision(),
        "preparation does not publish"
    );
    let committed = conversation
        .coordinator
        .commit(prepared)
        .expect("commit the expanded catalog");
    assert_ne!(committed.revision(), snapshot.revision());
    assert_eq!(
        committed.effective_environment(),
        snapshot.effective_environment()
    );
    let names = committed
        .skills()
        .catalog_entries()
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["a", "b"]);
}

#[tokio::test]
async fn cfg280_malformed_skill_dependencies_do_not_block_publication() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "good",
        "Good skill.",
        &[python_deps(r#"{"pypdf":"5.9.0"}"#)],
    );
    write_skill(
        conversation.workspace.root(),
        "bad",
        "Bad skill.",
        &[python_deps(r#"{"other":"not a version"}"#)],
    );
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(
        snapshot
            .skills()
            .catalog_entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["bad", "good"]
    );
    assert_eq!(snapshot.effective_environment(), &ToolEnvironment::new());
    assert!(
        snapshot
            .skills()
            .diagnostics()
            .iter()
            .all(|fact| { !format!("{fact:?}").contains("rustx.python-dependencies") }),
        "dependency text is not a Skill diagnostic"
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
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        *self.environment.lock().expect("env lock") = Some(context.environment.clone());
        let started = self.seen.clone();
        let mut release = self.release.subscribe();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                started.send_replace(true);
                release
                    .wait_for(|released| *released)
                    .await
                    .expect("background release channel stays open");
                ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                }
            }),
            context.cancellation.clone(),
        )
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}

fn background_invocation() -> ToolInvocation {
    ToolInvocation {
        id: rustx::tools::types::ToolInvocationId::Agent {
            call_id: ToolCallId::new("call-1"),
        },
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

    // A later definition publishes while the detached execution stays active.
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill, revised.",
        &[python_deps(r#"{"pypdf":"5.10.0"}"#)],
    );
    let snapshot_n1 = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(
        snapshot_n1.revision().get(),
        snapshot_n.revision().get() + 1,
        "an active detached background execution never blocks a capability commit"
    );
    assert_eq!(
        snapshot_n1.effective_environment(),
        snapshot_n.effective_environment(),
        "a Skill definition change leaves the tool environment unchanged"
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
        std::time::Duration::from_mins(2),
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
        std::time::Duration::from_mins(2),
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
            subagent_catalog: rustx::runtime::subagent::AgentCatalog::empty(),
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
    let coordinator = CapabilityCoordinator::new(CapabilityCoordinatorConfig {
        source_demand: rustx::capabilities::source::ToolSourceDemand::default(),
        conversation_id: ConversationId::new("conv_6c966d7f-bdde-792f-8692-51542a5c1834"),
        workspace: conversation.workspace.clone(),
        base_tool_registry: tools.clone(),
        extension_tools: rustx::extensions::ExtensionToolPlane::none(),
        agent_activation: rustx::capabilities::AgentActivation {
            profile: rustx::local_runtime::config::AgentProfileDocument {
                tools: rustx::capabilities::selection::ToolSelectionDocument {
                    builtin: tools
                        .definitions()
                        .into_iter()
                        .filter(|tool| tool.origin.source().is_none())
                        .map(|tool| tool.name.clone())
                        .collect(),
                    sources: tools
                        .definitions()
                        .into_iter()
                        .filter_map(|definition| definition.origin.source())
                        .map(|source| {
                            (
                                source,
                                rustx::capabilities::selection::SourceToolSelection::All,
                            )
                        })
                        .collect(),
                },
                ..fixture_activation().profile
            },
            ..Default::default()
        },
        skill_discovery: rustx::skills::SkillDiscoveryConfig::workspace_root(
            conversation.workspace.root().join(".agents/skills"),
        ),
        mcp_servers: std::collections::BTreeMap::new(),
        base_environment: ToolEnvironment::new(),
        environment_store_root: conversation.dir.path().join("skill-env-2"),
    })
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
        ConversationId::new("conv_6c966d7f-bdde-792f-8692-51542a5c1834"),
        conversation.workspace.root(),
        conversation.dir.path().join("agent-artifacts"),
    )
    .expect("tool runtime");
    let cancellation = rustx::agent::AgentCancellation::new(
        rustx::runtime::types::CancellationReason::UserRequested,
    );
    let request = rustx::agent::AgentExecutionRequest {
        agent_id: rustx::runtime::identity::AgentId::new("agent-1"),
        conversation_id: rustx::runtime::identity::ConversationId::new(
            "conv_6c966d7f-bdde-792f-8692-51542a5c1834",
        ),
        attempt_id: rustx::runtime::identity::AttemptId::new("attempt-1"),
        conversation: rustx::conversation::ConversationState::new(),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        model: support::attempt_model(model.clone(), "fake-model"),
    };
    let resources = rustx::runtime::RuntimeResourceSnapshot::new(
        rustx::runtime::RuntimeResourceRevision::new(1),
        Vec::new(),
        None,
        rustx::context::ContextAssembly::new(),
        Arc::new(snapshot.clone()),
    );
    let generation_facts = serde_json::to_value(resources.inspection()).unwrap();
    let runtime = rustx::context::ContextRuntime::for_attempt(
        rustx::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(rustx::context::DefaultTokenEstimator),
        Some(rustx::context::AgentStatusEngine::default()),
        &request.model,
        rustx::model::ModelTimeoutPolicy::default(),
        support::default_monotonic_clock(),
    )
    .expect("context runtime")
    .with_runtime_resources(&resources);
    let result = rustx::agent::AgentExecution::new(
        request,
        lease,
        &cancellation,
        support::default_execution_policy(),
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
    assert_eq!(
        serde_json::to_value(resources.inspection()).unwrap(),
        generation_facts,
        "turns never append or re-resolve generation diagnostics"
    );
    let reminder = rustx::runtime::permission::permission_reminder_text(
        rustx::runtime::ApprovalMode::FullAccess,
    );
    let expected_base = format!(
        "{}\n\nCurrent working directory: {}\n\n{catalog}",
        rustx::context::DEFAULT_SYSTEM_PROMPT,
        conversation.workspace.root().display()
    );
    let expected_first = format!("{expected_base}\n\n{reminder}");
    for (index, request) in requests.iter().enumerate() {
        let expected = if index == 0 {
            expected_first.as_str()
        } else {
            expected_base.as_str()
        };
        assert_eq!(
            request.effective_system_prompt, expected,
            "the attempt's immutable Skill snapshot extends the built-in System Prompt (request {index}; sparse Full Access reminder only on first start)"
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

fn assert_no_skill_environment(dir: &std::path::Path) {
    let store = dir.join("skill-env");
    assert!(
        store.is_dir(),
        "the runtime-private store root still exists"
    );
    for name in ["python", "node"] {
        assert!(
            !store.join(name).exists(),
            "skill discovery must not create a {name} environment"
        );
    }
}

/// Catalog, environment, and store-layout assertions for Skills that carry
/// uninterpreted dependency text. The launch proof lives in
/// `boundary_suites::skill_launch`, because it loads a recorder and awaits a
/// real interpreter.
pub(crate) async fn exercise_skill_catalog_without_environments() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill.",
        &[
            python_deps(r#"{"pypdf":"5.9.0","pillow":"not a version"}"#),
            node_deps(r#"{"pdf-lib":"1.17.1"}"#),
        ],
    );
    write_skill(
        conversation.workspace.root(),
        "slides",
        "Slides.",
        &[python_deps(r#"{"pypdf":"9.9.9"}"#), node_deps("malformed")],
    );
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(
        snapshot
            .skills()
            .catalog_entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["pdf", "slides"]
    );
    assert_eq!(snapshot.effective_environment(), &ToolEnvironment::new());
    assert_no_skill_environment(conversation.dir.path());
    write_skill(
        conversation.workspace.root(),
        "pdf",
        "PDF skill reloaded.",
        &[python_deps(r#"{"pypdf":"5.10.0"}"#)],
    );
    let reloaded = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(
        reloaded.effective_environment(),
        snapshot.effective_environment()
    );
    assert_eq!(
        reloaded
            .skills()
            .catalog_entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["pdf", "slides"]
    );
    assert_no_skill_environment(conversation.dir.path());
}

/// S01/S02 catalog half: selecting and reloading Skills that carry Python and
/// Node metadata publishes both names, leaves the effective environment
/// unchanged, and creates no Skill environment directory. The zero-launch
/// proof is `boundary_suites::skill_launch`.
#[tokio::test]
async fn skill_metadata_does_not_probe_or_materialize() {
    exercise_skill_catalog_without_environments().await;
}

/// S03: zero, one, many, and hidden Skills share one effective environment.
#[tokio::test]
async fn skill_population_does_not_change_the_effective_environment() {
    let conversation = conversation();
    let empty = prepare_and_commit(&conversation.coordinator).await;
    let baseline = empty.effective_environment().clone();
    write_skill(
        conversation.workspace.root(),
        "one",
        "One skill.",
        &[python_deps(r#"{"example":"1.0.0"}"#)],
    );
    let one = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(one.effective_environment(), &baseline);
    write_skill(
        conversation.workspace.root(),
        "two",
        "Two.",
        &[node_deps(r#"{"left-pad":"1.0.0"}"#)],
    );
    write_hidden_skill(conversation.workspace.root(), "hidden", "Hidden.");
    let many = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(many.effective_environment(), &baseline);
    assert_eq!(
        many.effective_environment()
            .child_environment(conversation.workspace.root()),
        baseline.child_environment(conversation.workspace.root())
    );
    assert!(
        many.skill_catalog()
            .is_some_and(|catalog| !catalog.contains("Hidden."))
    );
}

/// S04/S08: payload size, symlinks, and body edits do not hash the payload or
/// create an environment. Ordinary reads observe the current instruction bytes.
#[tokio::test]
async fn payload_edits_stay_outside_discovery_and_reads_see_current_bytes() {
    let conversation = conversation();
    write_skill(
        conversation.workspace.root(),
        "guide",
        "Guide.",
        &[python_deps(r#"{"example":"1.0.0"}"#)],
    );
    let package = conversation.workspace.root().join(".agents/skills/guide");
    std::fs::File::create(package.join("asset.bin"))
        .unwrap()
        .set_len(8 * 1024 * 1024)
        .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/nonexistent/payload", package.join("references")).unwrap();
    let snapshot = prepare_and_commit(&conversation.coordinator).await;
    let location = snapshot.skills().catalog_entries()[0].location.clone();
    assert_eq!(
        location,
        skill_location(conversation.workspace.root(), "guide")
    );
    let body = std::fs::read_to_string(&location).unwrap();
    assert!(body.contains("body"));
    std::fs::write(&location, body.replace("body", "current bytes")).unwrap();
    assert!(
        std::fs::read_to_string(&location)
            .unwrap()
            .contains("current bytes")
    );
    let again = prepare_and_commit(&conversation.coordinator).await;
    assert_eq!(again.revision(), snapshot.revision());
    assert_eq!(again.skills().bindings(), snapshot.skills().bindings());
    assert_no_skill_environment(conversation.dir.path());
}
