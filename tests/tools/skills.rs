//! Deterministic tests: Skill discovery, parsing, version identity, and the
//! compact model-visible catalog.
//!
//! Every test is local: no public package registry, no network, and no
//! package-manager subprocess. Dependency-looking metadata is ordinary text.

use std::path::Path;

use rustx::runtime::identity::SkillVersionId;
use rustx::skills::{
    SkillDiagnostic, SkillDiscovery, SkillDiscoveryConfig, SkillPackageError, render_skill_catalog,
};
use rustx::tools::workspace::Workspace;
/// Writes one valid Skill package into the workspace.
fn write_skill(
    workspace: &Path,
    name: &str,
    description: &str,
    metadata: &[(&str, &str)],
    body: &str,
) {
    let root = workspace.join(".agents").join("skills").join(name);
    std::fs::create_dir_all(&root).expect("skill dir");
    let mut frontmatter = format!("---\nname: {name}\ndescription: \"{description}\"\n");
    if !metadata.is_empty() {
        frontmatter.push_str("metadata:\n");
        for (key, value) in metadata {
            use std::fmt::Write as _;
            let _ = writeln!(frontmatter, "  {key}: '{value}'");
        }
    }
    frontmatter.push_str("---\n");
    std::fs::write(root.join("SKILL.md"), format!("{frontmatter}{body}")).expect("SKILL.md");
}

/// A workspace fixture: canonical root plus the skills directory.
fn fixture() -> (tempfile::TempDir, Workspace) {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace = Workspace::new(dir.path()).expect("workspace");
    (dir, workspace)
}

fn discover(workspace: &Workspace) -> Vec<rustx::skills::SkillPackage> {
    project_discovery(workspace).discover().packages
}

/// The single typed exclusion cause of a workspace root holding exactly one
/// malformed candidate.
///
/// A malformed package is never a discovery failure (#280): it is excluded
/// with a typed generation diagnostic at the package boundary that owns it.
fn invalid_cause(workspace: &Workspace) -> SkillPackageError {
    let outcome = project_discovery(workspace).discover();
    assert!(
        outcome.packages.is_empty(),
        "the malformed candidate must not publish, got {:?}",
        outcome
            .packages
            .iter()
            .map(rustx::skills::SkillPackage::name)
            .collect::<Vec<_>>()
    );
    let [SkillDiagnostic::PackageInvalid { cause, .. }] = outcome.diagnostics.as_slice() else {
        panic!(
            "expected exactly one package-invalid fact, got {:?}",
            outcome.diagnostics
        );
    };
    cause.clone()
}

/// Isolates the project-root tests from the developer's User Skill roots.
fn project_discovery(workspace: &Workspace) -> SkillDiscovery {
    SkillDiscovery::with_config(SkillDiscoveryConfig::workspace_root(
        workspace.root().join(".agents/skills"),
    ))
}

/// A missing `.agents/skills/` directory means an empty Skill set, not an
/// error.
#[test]
fn missing_skill_root_is_an_empty_skill_set() {
    let (_dir, workspace) = fixture();
    let outcome = project_discovery(&workspace).discover();
    assert!(outcome.packages.is_empty());
}

/// Valid Skills are discovered with deterministic ordering independent of
/// filesystem enumeration order.
#[test]
fn discovery_is_deterministic_and_sorted() {
    let (dir, workspace) = fixture();
    // Create in reverse order; discovery must still sort by name.
    for name in ["zebra", "alpha", "mid"] {
        write_skill(dir.path(), name, &format!("Does {name}."), &[], "body\n");
    }
    let packages = discover(&workspace);
    let names: Vec<&str> = packages
        .iter()
        .map(rustx::skills::SkillPackage::name)
        .collect();
    assert_eq!(names, ["alpha", "mid", "zebra"]);
}

/// Hidden root entries never become Skills; ordinary files directly under
/// the Skill root are ignored.
#[test]
fn hidden_entries_and_unrelated_files_are_ignored() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "visible", "A visible skill.", &[], "body\n");
    std::fs::create_dir_all(dir.path().join(".agents/skills/.hidden")).expect("hidden dir");
    std::fs::write(
        dir.path().join(".agents/skills/.hidden/SKILL.md"),
        "---\nname: hidden\ndescription: hidden\n---\n",
    )
    .expect("hidden SKILL.md");
    std::fs::write(dir.path().join(".agents/skills/notes.txt"), "unrelated").expect("notes");
    let packages = discover(&workspace);
    let names: Vec<&str> = packages
        .iter()
        .map(rustx::skills::SkillPackage::name)
        .collect();
    assert_eq!(names, ["visible"]);
}

/// The declared name is authoritative even if its folder has another name.
#[test]
fn declared_name_can_differ_from_the_parent_directory() {
    let (dir, workspace) = fixture();
    let root = dir.path().join(".agents/skills/pdf");
    std::fs::create_dir_all(&root).expect("dir");
    std::fs::write(
        root.join("SKILL.md"),
        "---\nname: documents\ndescription: A skill.\n---\nbody\n",
    )
    .expect("SKILL.md");
    assert_eq!(discover(&workspace)[0].name(), "documents");
}

/// Nonstandard names load with a warning, as in Pi.
#[test]
fn invalid_standard_names_warn_without_hiding_the_skill() {
    for bad in ["PDF", "-pdf", "pdf--x", "pdf-", &"p".repeat(65)] {
        let (dir, workspace) = fixture();
        let root = dir.path().join(".agents/skills").join(bad);
        std::fs::create_dir_all(&root).expect("dir");
        std::fs::write(
            root.join("SKILL.md"),
            format!("---\nname: {bad}\ndescription: A skill.\n---\nbody\n"),
        )
        .expect("SKILL.md");
        let outcome = project_discovery(&workspace).discover();
        assert_eq!(outcome.packages[0].name(), bad);
        assert!(matches!(
            outcome.diagnostics.as_slice(),
            [SkillDiagnostic::PackageWarning { .. }]
        ));
    }
}

/// A description is required. The length recommendation only produces a warning.
#[test]
fn empty_descriptions_are_rejected_and_oversized_ones_warn() {
    let (dir, workspace) = fixture();
    let root = dir.path().join(".agents/skills/skill");
    std::fs::create_dir_all(&root).expect("dir");
    std::fs::write(
        root.join("SKILL.md"),
        "---\nname: skill\ndescription: \"\"\n---\nbody\n",
    )
    .expect("SKILL.md");
    assert!(matches!(
        invalid_cause(&workspace),
        SkillPackageError::InvalidDescription { .. }
    ));
    std::fs::write(
        root.join("SKILL.md"),
        format!(
            "---\nname: skill\ndescription: \"{}\"\n---\nbody\n",
            "x".repeat(1025)
        ),
    )
    .expect("SKILL.md");
    let outcome = project_discovery(&workspace).discover();
    assert_eq!(outcome.packages[0].description().len(), 1025);
    assert!(matches!(
        outcome.diagnostics.as_slice(),
        [SkillDiagnostic::PackageWarning { .. }]
    ));
}

/// Malformed YAML frontmatter is rejected.
#[test]
fn malformed_yaml_is_rejected() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "skill", "A skill.", &[], "body\n");
    std::fs::write(
        dir.path().join(".agents/skills/skill/SKILL.md"),
        "---\nname: skill\ndescription: [unclosed\n---\nbody\n",
    )
    .expect("SKILL.md");
    assert!(matches!(
        invalid_cause(&workspace),
        SkillPackageError::MalformedFrontmatter { .. }
    ));
    // A missing closing delimiter is malformed too.
    std::fs::write(
        dir.path().join(".agents/skills/skill/SKILL.md"),
        "---\nname: skill\ndescription: A skill.\nbody without close\n",
    )
    .expect("SKILL.md");
    assert!(matches!(
        invalid_cause(&workspace),
        SkillPackageError::MalformedFrontmatter { .. }
    ));
}

/// Optional fields are accepted without strict shape validation.
#[test]
fn optional_frontmatter_fields_do_not_gate_discovery() {
    let (dir, workspace) = fixture();
    let root = dir.path().join(".agents/skills/skill");
    std::fs::create_dir_all(&root).expect("dir");
    std::fs::write(
        root.join("SKILL.md"),
        format!("---\nname: skill\ndescription: A skill.\ncompatibility: {}\nallowed-tools: [Read]\nmetadata:\n  version: 1.0\n---\nbody\n", "x".repeat(501)),
    )
    .expect("SKILL.md");
    let outcome = project_discovery(&workspace).discover();
    assert!(outcome.diagnostics.is_empty());
    assert_eq!(outcome.packages[0].compatibility().unwrap().len(), 501);
    assert_eq!(outcome.packages[0].metadata()["version"], 1.0);
}

/// A grouping directory without any definitions is an empty branch.
#[test]
fn empty_grouping_directory_is_ignored() {
    let (dir, workspace) = fixture();
    std::fs::create_dir_all(dir.path().join(".agents/skills/empty")).expect("dir");
    let outcome = project_discovery(&workspace).discover();
    assert!(outcome.packages.is_empty());
    assert!(outcome.diagnostics.is_empty());
}

/// Nested packages are never discovered recursively: a subdirectory of a
/// package does not become a Skill.
#[test]
fn nested_packages_are_not_recursively_discovered() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "outer", "Outer skill.", &[], "body\n");
    write_skill(dir.path(), "outer", "Outer skill.", &[], "body\n");
    // A nested package-looking directory inside the outer package.
    let nested = dir.path().join(".agents/skills/outer/nested");
    std::fs::create_dir_all(&nested).expect("nested dir");
    std::fs::write(
        nested.join("SKILL.md"),
        "---\nname: nested\ndescription: Nested.\n---\n",
    )
    .expect("nested SKILL.md");
    let packages = discover(&workspace);
    let names: Vec<&str> = packages
        .iter()
        .map(rustx::skills::SkillPackage::name)
        .collect();
    assert_eq!(names, ["outer"], "nested packages are never discovered");
}

/// Contained package aliases are deduplicated; internal resources belong to Tools.
#[test]
fn package_symlinks_are_deduplicated() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "real", "Real skill.", &[], "body\n");
    std::os::unix::fs::symlink(
        dir.path().join(".agents/skills/real"),
        dir.path().join(".agents/skills/link"),
    )
    .expect("symlink root");
    let outcome = project_discovery(&workspace).discover();
    // Both spellings name the same physical file.
    assert_eq!(
        outcome
            .packages
            .iter()
            .map(rustx::skills::SkillPackage::name)
            .collect::<Vec<_>>(),
        ["real"]
    );
    assert!(outcome.diagnostics.is_empty());

    let (dir2, workspace2) = fixture();
    write_skill(dir2.path(), "skill", "A skill.", &[], "body\n");
    std::os::unix::fs::symlink(
        dir2.path().join("elsewhere"),
        dir2.path().join(".agents/skills/skill/scripts"),
    )
    .expect("internal symlink");
    let internal = project_discovery(&workspace2).discover();
    assert_eq!(internal.packages.len(), 1);
    assert!(
        internal.diagnostics.is_empty(),
        "runtime resource links are not discovery inputs"
    );
}

/// Discovery accepts a non-canonical package root but publishes a canonical
/// absolute one.
///
/// This boundary owns the invariant on its own, for any caller. Publishing a
/// non-canonical spelling verbatim would break the whole point of a host
/// path: Read resolves a relative model path against the canonical Workspace
/// root and Bash runs with that root as its cwd, so both would re-prefix it
/// and open the wrong file (or nothing).
#[test]
fn a_non_canonical_package_root_is_published_canonically() {
    // Created under the process cwd so a genuinely relative candidate path
    // exists without any test mutating the shared process cwd.
    let dir = tempfile::tempdir_in(".").expect("temporary root under cwd");
    let relative_root = std::path::PathBuf::from(dir.path().file_name().expect("temporary name"));
    let workspace_root = dir.path().join("work");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    write_skill(&workspace_root, "deck", "Deck skill.", &[], "body\n");
    let workspace = Workspace::new(&workspace_root).expect("workspace");

    for explicit in [
        // Relative input from a low-level discovery caller. Production binds
        // the fixed User/Workspace collections before this boundary. Discovery
        // preserves canonical package identity for every caller.
        relative_root.join("work/.agents/skills/deck"),
        // Absolute but non-canonical: an embedded `..` is a different
        // spelling of the same package.
        workspace_root.join("../work/.agents/skills/deck"),
    ] {
        let outcome = SkillDiscovery::with_config(SkillDiscoveryConfig::workspace_root(
            explicit.parent().unwrap().to_path_buf(),
        ))
        .discover();
        let snapshot = rustx::skills::SkillSnapshot::from_discovery(outcome);
        let location = snapshot.catalog_entries()[0].location.clone();
        let published = std::path::Path::new(&location);

        assert!(
            published.is_absolute(),
            "published location must be absolute for {explicit:?}, got {location:?}"
        );
        assert_eq!(
            published,
            workspace
                .root()
                .join(".agents/skills/deck/SKILL.md")
                .as_path(),
            "every spelling of one package publishes the same canonical location"
        );
        // The decisive property: re-resolving the published path against the
        // execution cwd — what Read and Bash both do — reaches the same file.
        assert_eq!(
            workspace.root().join(published),
            published.to_path_buf(),
            "an absolute published location is cwd-independent"
        );
        assert!(published.is_file(), "the published location exists");
    }
}

/// A package root that is not valid UTF-8 cannot be published as a
/// model-visible location, so discovery rejects it instead of handing the
/// model a lossy path that names no file.
#[cfg(unix)]
#[test]
fn a_non_utf8_package_root_is_rejected_rather_than_published_lossily() {
    use std::os::unix::ffi::OsStrExt as _;

    let (dir, _) = fixture();
    // A lone 0xFF byte is never valid UTF-8. Linux filesystems store it
    // verbatim, which is what makes the discovery-level rejection reachable
    // and worth asserting.
    let invalid = std::ffi::OsStr::from_bytes(b"skills-\xff");
    let root = dir.path().join(invalid);
    let package = root.join("deck");
    // macOS (APFS/HFS+) enforces UTF-8 filenames and refuses the name with
    // EILSEQ, so the same invariant already holds one layer lower and no
    // such package can reach discovery at all. Skip rather than assert a
    // rejection the platform makes unreachable.
    if let Err(error) = std::fs::create_dir_all(&package) {
        eprintln!(
            "the filesystem refuses non-UTF-8 names ({error}); the \
             discovery-level rejection is unreachable here and was not \
             exercised"
        );
        return;
    }
    std::fs::write(
        package.join("SKILL.md"),
        "---\nname: deck\ndescription: Deck skill.\n---\nbody\n",
    )
    .expect("SKILL.md");

    let outcome = SkillDiscovery::with_config(SkillDiscoveryConfig::workspace_root(
        package.parent().unwrap().to_path_buf(),
    ))
    .discover();
    assert!(outcome.packages.is_empty());
    assert!(
        matches!(
            outcome.diagnostics.as_slice(),
            [SkillDiagnostic::PackageInvalid {
                cause: SkillPackageError::UnrepresentableRoot { .. },
                ..
            }]
        ),
        "expected UnrepresentableRoot, got {:?}",
        outcome.diagnostics
    );
}

/// A Skill's supporting files are reachable from Bash, not just Read.
///
/// `SKILL.md` refers to its own assets relatively; the model resolves those
/// against the published package directory and runs an ordinary shell
/// command. This is the whole reason the catalog publishes a host path: a
/// namespace only Read understood would fail every Bash-executed Skill step.
#[cfg(unix)]
#[tokio::test]
async fn bash_reaches_skill_assets_through_the_published_location() {
    let (dir, workspace) = fixture();
    write_skill(
        dir.path(),
        "deck",
        "Builds decks.",
        &[],
        "Copy assets/template.html into the workspace.\n",
    );
    let assets = dir.path().join(".agents/skills/deck/assets");
    std::fs::create_dir_all(&assets).expect("assets dir");
    std::fs::write(assets.join("template.html"), "<!-- deck template -->\n").expect("template");

    let packages = discover(&workspace);
    let snapshot =
        rustx::skills::SkillSnapshot::new(packages.into_iter().map(std::sync::Arc::new).collect());
    let location = snapshot.catalog_entries()[0].location.clone();
    let skill_dir = std::path::Path::new(&location)
        .parent()
        .expect("skill directory")
        .to_path_buf();

    let conversation_id =
        rustx::runtime::identity::ConversationId::new("conv_8ecf8c4f-141c-7493-8f52-692cc55da299");
    let artifacts = dir.path().join("artifacts");
    let artifacts_store =
        rustx::tools::artifacts::ArtifactStore::new(conversation_id.clone(), &artifacts)
            .expect("artifacts");
    let tool_output = rustx::tools::managed_output::ManagedToolOutput::new(
        conversation_id.clone(),
        artifacts.join("tool-output"),
    )
    .expect("managed tool output");
    let environment = rustx::tools::environment::ToolEnvironment::new();
    let mut registry = rustx::tools::executor::ToolRegistry::new();
    rustx::tools::native::register_native_tools(
        &mut registry,
        rustx::tools::native::NativeToolResources {
            subagent_catalog: rustx::runtime::subagent::AgentCatalog::empty(),
            background: rustx::tools::background::ConversationBackgroundRegistry::new(
                conversation_id.clone(),
                rustx::tools::background::BackgroundResources {
                    mailbox: rustx::runtime::inbound::ConversationInboundMailbox::new(
                        conversation_id.clone(),
                    ),
                    workspace: workspace.clone(),
                    artifacts: artifacts_store.clone(),
                    tool_output: tool_output.clone(),
                    clock: std::sync::Arc::new(rustx::runtime::SystemClock),
                    event_sink: None,
                },
            ),
            subagents: None,
        },
        rustx::tools::native::NativeToolPolicies::default(),
    )
    .expect("native tools");
    let executor = registry.executor(&rustx::runtime::identity::ToolId::new("tool-bash"));
    let context = rustx::tools::executor::ToolExecutionContext::new(
        &conversation_id,
        None,
        rustx::runtime::ExecutionCancellation::detached(
            rustx::runtime::CancellationSignal::new(),
            rustx::runtime::types::CancellationReason::UserRequested,
        ),
        &workspace,
        &crate::common::NoopProgress,
        &artifacts_store,
        &tool_output,
        &environment,
    );
    let result = executor
        .start(
            rustx::tools::types::ToolInvocation {
                id: rustx::tools::types::ToolInvocationId::Agent {
                    call_id: rustx::runtime::identity::ToolCallId::new("call-copy"),
                },
                tool_id: rustx::runtime::identity::ToolId::new("tool-bash"),
                tool_name: "bash".to_owned(),
                mode: rustx::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({
                    "command": format!(
                        "cp {}/assets/template.html ./index.html && cat ./index.html",
                        skill_dir.display()
                    )
                }),
            },
            context,
        )
        .completion
        .await;

    assert_eq!(
        result.status,
        rustx::tools::types::ToolExecutionStatus::Success,
        "Bash must reach the Skill's own assets: {result:?}"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.root().join("index.html")).expect("copied template"),
        "<!-- deck template -->\n"
    );
}

/// Bash `cd` cannot redefine the canonical Skill root: the root is
/// anchored to the Workspace, and Bash has no persistent cwd.
#[cfg(unix)]
#[tokio::test]
async fn bash_cd_cannot_redefine_the_skill_root() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "pdf", "PDF skill.", &[], "body\n");
    let conversation_id =
        rustx::runtime::identity::ConversationId::new("conv_6c966d7f-bdde-792f-8692-51542a5c1834");
    let artifacts = dir.path().join("artifacts");
    let registry = rustx::tools::executor::ToolRegistry::new();
    let mut registry = registry;
    rustx::tools::native::register_native_tools(
        &mut registry,
        rustx::tools::native::NativeToolResources {
            subagent_catalog: rustx::runtime::subagent::AgentCatalog::empty(),
            background: rustx::tools::background::ConversationBackgroundRegistry::new(
                conversation_id.clone(),
                rustx::tools::background::BackgroundResources {
                    mailbox: rustx::runtime::inbound::ConversationInboundMailbox::new(
                        conversation_id.clone(),
                    ),
                    workspace: workspace.clone(),
                    artifacts: rustx::tools::artifacts::ArtifactStore::new(
                        conversation_id.clone(),
                        &artifacts,
                    )
                    .expect("artifacts"),
                    tool_output: rustx::tools::managed_output::ManagedToolOutput::new(
                        conversation_id.clone(),
                        artifacts.join("tool-output"),
                    )
                    .expect("managed tool output"),
                    clock: std::sync::Arc::new(rustx::runtime::SystemClock),
                    event_sink: None,
                },
            ),
            subagents: None,
        },
        rustx::tools::native::NativeToolPolicies::default(),
    )
    .expect("native tools");
    let executor = registry.executor(&rustx::runtime::identity::ToolId::new("tool-bash"));
    let invocation = rustx::tools::types::ToolInvocation {
        id: rustx::tools::types::ToolInvocationId::Agent {
            call_id: rustx::runtime::identity::ToolCallId::new("call-1"),
        },
        tool_id: rustx::runtime::identity::ToolId::new("tool-bash"),
        tool_name: "bash".to_owned(),
        mode: rustx::tools::types::ToolInvocationMode::Foreground,
        arguments: serde_json::json!({"command": "cd /tmp && pwd"}),
    };
    let result = {
        let artifacts_store =
            rustx::tools::artifacts::ArtifactStore::new(conversation_id.clone(), &artifacts)
                .expect("artifacts");
        let tool_output = rustx::tools::managed_output::ManagedToolOutput::new(
            conversation_id.clone(),
            artifacts.join("tool-output"),
        )
        .expect("managed tool output");
        let environment = rustx::tools::environment::ToolEnvironment::new();
        let context = rustx::tools::executor::ToolExecutionContext::new(
            &conversation_id,
            None,
            rustx::runtime::ExecutionCancellation::detached(
                rustx::runtime::CancellationSignal::new(),
                rustx::runtime::types::CancellationReason::UserRequested,
            ),
            &workspace,
            &crate::common::NoopProgress,
            &artifacts_store,
            &tool_output,
            &environment,
        );
        executor.start(invocation, context).completion.await
    };
    assert!(matches!(
        result.status,
        rustx::tools::types::ToolExecutionStatus::Success
    ));
    // Discovery after the `cd` still resolves the canonical root.
    let packages = discover(&workspace);
    let names: Vec<&str> = packages
        .iter()
        .map(rustx::skills::SkillPackage::name)
        .collect();
    assert_eq!(names, ["pdf"]);
}

// ---------------------------------------------------------------------------
// Package/version identity
// ---------------------------------------------------------------------------

fn version_id(workspace: &Workspace, name: &str) -> SkillVersionId {
    discover(workspace)
        .into_iter()
        .find(|package| package.name() == name)
        .expect("skill present")
        .version_id()
        .clone()
}

/// Same package bytes produce the same version identity; host workspace
/// location does not affect it.
#[test]
fn same_bytes_same_version_identity_across_workspaces() {
    let (dir_a, workspace_a) = fixture();
    write_skill(dir_a.path(), "pdf", "PDF skill.", &[], "body\n");
    let (dir_b, workspace_b) = fixture();
    write_skill(dir_b.path(), "pdf", "PDF skill.", &[], "body\n");
    assert_eq!(
        version_id(&workspace_a, "pdf"),
        version_id(&workspace_b, "pdf")
    );
    assert_eq!(version_id(&workspace_a, "pdf").as_str().len(), 7 + 64);
}

/// Filesystem enumeration order never enters the digest: two packages
/// with identical content created in different orders produce the same
/// identity.
#[test]
fn filesystem_enumeration_order_does_not_affect_the_digest() {
    let (dir_a, workspace_a) = fixture();
    write_skill(dir_a.path(), "pdf", "PDF skill.", &[], "body\n");
    let scripts = dir_a.path().join(".agents/skills/pdf/scripts");
    std::fs::create_dir_all(&scripts).expect("scripts");
    std::fs::write(scripts.join("z.py"), "print('z')\n").expect("z");
    std::fs::write(scripts.join("a.py"), "print('a')\n").expect("a");

    let (dir_b, workspace_b) = fixture();
    write_skill(dir_b.path(), "pdf", "PDF skill.", &[], "body\n");
    let scripts_b = dir_b.path().join(".agents/skills/pdf/scripts");
    std::fs::create_dir_all(&scripts_b).expect("scripts");
    std::fs::write(scripts_b.join("a.py"), "print('a')\n").expect("a");
    std::fs::write(scripts_b.join("z.py"), "print('z')\n").expect("z");

    assert_eq!(
        version_id(&workspace_a, "pdf"),
        version_id(&workspace_b, "pdf")
    );
}

/// File mtimes never enter the digest.
#[test]
fn file_mtime_does_not_affect_the_digest() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "pdf", "PDF skill.", &[], "body\n");
    let before = version_id(&workspace, "pdf");
    let path = dir.path().join(".agents/skills/pdf/SKILL.md");
    let past = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    let file = std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("open");
    file.set_modified(past).expect("set mtime");
    drop(file);
    let after = version_id(&workspace, "pdf");
    assert_eq!(before, after);
}

/// Runtime body edits preserve the definition version.
#[test]
fn body_change_preserves_the_version_identity() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "pdf", "PDF skill.", &[], "body one\n");
    let before = version_id(&workspace, "pdf");
    write_skill(dir.path(), "pdf", "PDF skill.", &[], "body two\n");
    assert_eq!(before, version_id(&workspace, "pdf"));
}

/// Runtime script edits preserve the definition version.
#[test]
fn script_change_preserves_the_version_identity() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "pdf", "PDF skill.", &[], "body\n");
    let scripts = dir.path().join(".agents/skills/pdf/scripts");
    std::fs::create_dir_all(&scripts).expect("scripts");
    std::fs::write(scripts.join("extract.py"), "print('one')\n").expect("script");
    let before = version_id(&workspace, "pdf");
    std::fs::write(scripts.join("extract.py"), "print('two')\n").expect("script");
    assert_eq!(before, version_id(&workspace, "pdf"));
}

/// Binary assets do not participate in discovery identity.
#[test]
fn binary_asset_change_preserves_the_version_identity() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "pdf", "PDF skill.", &[], "body\n");
    let assets = dir.path().join(".agents/skills/pdf/assets");
    std::fs::create_dir_all(&assets).expect("assets");
    std::fs::write(assets.join("sample.bin"), [0u8, 1, 2, 3]).expect("asset");
    let before = version_id(&workspace, "pdf");
    std::fs::write(assets.join("sample.bin"), [0u8, 9, 8, 7]).expect("asset");
    assert_eq!(before, version_id(&workspace, "pdf"));
}

/// A description-only change changes the version identity.
#[test]
fn description_change_changes_the_version_identity() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "pdf", "First description.", &[], "body\n");
    let before = version_id(&workspace, "pdf");
    write_skill(dir.path(), "pdf", "Second description.", &[], "body\n");
    assert_ne!(before, version_id(&workspace, "pdf"));
}

/// Literal dependency metadata participates in the definition identity and is
/// not normalized into pins.
#[test]
fn dependency_metadata_text_changes_the_version_identity() {
    let (dir, workspace) = fixture();
    write_skill(
        dir.path(),
        "pdf",
        "PDF skill.",
        &[("rustx.python-dependencies", r#"{"pypdf":"5.9.0"}"#)],
        "body\n",
    );
    let before = version_id(&workspace, "pdf");
    write_skill(
        dir.path(),
        "pdf",
        "PDF skill.",
        &[("rustx.python-dependencies", r#"{"pypdf":"5.10.0"}"#)],
        "body\n",
    );
    assert_ne!(before, version_id(&workspace, "pdf"));
    let packages = discover(&workspace);
    assert_eq!(
        packages[0].metadata()["rustx.python-dependencies"].as_str(),
        Some(r#"{"pypdf":"5.10.0"}"#)
    );
}

// ---------------------------------------------------------------------------
// Uninterpreted dependency metadata
// ---------------------------------------------------------------------------

/// Malformed dependency-looking metadata does not hide the Skill or warn.
#[test]
fn malformed_dependency_metadata_stays_on_the_skill() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "good", "Good skill.", &[], "body\n");
    write_skill(
        dir.path(),
        "bad",
        "Bad skill.",
        &[
            ("rustx.python-dependencies", r#"{"pypdf":"not a version"}"#),
            ("rustx.node-dependencies", r#"{"typescript":"5.8.0"}"#),
        ],
        "body\n",
    );
    let outcome = project_discovery(&workspace).discover();
    assert_eq!(
        outcome
            .packages
            .iter()
            .map(rustx::skills::SkillPackage::name)
            .collect::<Vec<_>>(),
        ["bad", "good"]
    );
    assert_eq!(
        outcome.packages[0].metadata()["rustx.python-dependencies"].as_str(),
        Some(r#"{"pypdf":"not a version"}"#)
    );
    assert_eq!(
        outcome.packages[0].metadata()["rustx.node-dependencies"].as_str(),
        Some(r#"{"typescript":"5.8.0"}"#)
    );
    assert!(
        outcome
            .diagnostics
            .iter()
            .all(|fact| !matches!(fact, SkillDiagnostic::PackageWarning { .. })),
        "{:?}",
        outcome.diagnostics
    );
}

// ---------------------------------------------------------------------------
// Model-visible catalog
// ---------------------------------------------------------------------------

/// The exact compact `## Skills` catalog form: deterministic ordering, name +
/// description + the host `SKILL.md` location; no SKILL.md body and no
/// dependency metadata.
#[test]
fn catalog_rendering_is_exact_and_publishes_host_skill_locations() {
    let (dir, workspace) = fixture();
    write_skill(
        dir.path(),
        "pdf",
        "Create, edit, inspect, and transform PDF documents.",
        &[],
        "body\n",
    );
    write_skill(
        dir.path(),
        "slides",
        "Create and modify presentation decks.",
        &[],
        "body\n",
    );
    let packages = discover(&workspace);
    let snapshot =
        rustx::skills::SkillSnapshot::new(packages.into_iter().map(std::sync::Arc::new).collect());
    let rendered = render_skill_catalog(
        snapshot.catalog_entries(),
        rustx::skills::SkillReadTool::Read,
    );
    let skills_root = workspace.root().join(".agents/skills");
    let pdf = skills_root.join("pdf/SKILL.md").display().to_string();
    let slides = skills_root.join("slides/SKILL.md").display().to_string();
    assert!(rendered.contains(&skills_root.display().to_string()));
    assert!(rendered.contains("<name>pdf</name>") && rendered.contains("<name>slides</name>"));
    assert!(rendered.contains("Use the read tool to load"));
    assert!(
        rendered.contains(&pdf) && rendered.contains(&slides),
        "each instruction path enters the prompt"
    );
    assert!(!rendered.contains("body"), "SKILL.md bodies never appear");
    assert!(
        !rendered.contains("rustx.python-dependencies"),
        "dependency metadata never appears"
    );
    assert_eq!(snapshot.catalog_entries()[0].location, pdf);
    assert_eq!(snapshot.catalog_entries()[1].location, slides);

    // The empty snapshot has no entries; CapabilitySnapshot consequently
    // omits the entire system section.
    let empty = rustx::skills::SkillSnapshot::new(Vec::new());
    assert!(empty.catalog_entries().is_empty());
}
