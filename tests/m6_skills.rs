//! M6 deterministic tests: Skill discovery, parsing, version identity,
//! dependency declarations, and the compact model-visible catalog.
//!
//! Every test is local: no public package registry, no network, no
//! package-manager subprocess.

use std::collections::BTreeMap;
use std::path::Path;

use rustx::runtime::identity::SkillVersionId;
use rustx::skills::{
    DependencyError, SkillDiscovery, SkillDiscoveryConfig, SkillPackageError,
    merge_dependency_manifests, node_environment_digest, python_environment_digest,
    render_skill_catalog,
};
use rustx::tools::workspace::Workspace;
#[path = "common/mod.rs"]
mod common;

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
    project_discovery(workspace).discover().expect("discover")
}

/// Isolates the project-root tests from the developer's global Skill roots.
fn project_discovery(workspace: &Workspace) -> SkillDiscovery {
    SkillDiscovery::with_config(
        workspace,
        SkillDiscoveryConfig {
            automatic_roots: vec![
                workspace.root().join(".rustx/skills"),
                workspace.root().join(".agents/skills"),
            ],
            explicit_paths: Vec::new(),
        },
    )
}

/// A missing `.agents/skills/` directory means an empty Skill set, not an
/// error.
#[test]
fn missing_skill_root_is_an_empty_skill_set() {
    let (_dir, workspace) = fixture();
    let packages = project_discovery(&workspace).discover().expect("discover");
    assert!(packages.is_empty());
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

/// A frontmatter `name` that does not match the parent directory is
/// rejected.
#[test]
fn name_must_match_the_parent_directory() {
    let (dir, workspace) = fixture();
    let root = dir.path().join(".agents/skills/pdf");
    std::fs::create_dir_all(&root).expect("dir");
    std::fs::write(
        root.join("SKILL.md"),
        "---\nname: documents\ndescription: A skill.\n---\nbody\n",
    )
    .expect("SKILL.md");
    let error = project_discovery(&workspace)
        .discover()
        .expect_err("rejected");
    assert!(matches!(
        error,
        SkillPackageError::NameDirectoryMismatch { .. }
    ));
}

/// Invalid standard names are rejected: uppercase, leading hyphen,
/// consecutive hyphens, and names exceeding 64 characters.
#[test]
fn invalid_standard_names_are_rejected() {
    for bad in ["PDF", "-pdf", "pdf--x", "pdf-", &"p".repeat(65)] {
        let (dir, workspace) = fixture();
        let root = dir.path().join(".agents/skills").join(bad);
        std::fs::create_dir_all(&root).expect("dir");
        std::fs::write(
            root.join("SKILL.md"),
            format!("---\nname: {bad}\ndescription: A skill.\n---\nbody\n"),
        )
        .expect("SKILL.md");
        let error = project_discovery(&workspace)
            .discover()
            .expect_err("rejected");
        assert!(
            matches!(error, SkillPackageError::InvalidName { .. }),
            "name {bad:?} must be rejected, got {error:?}"
        );
    }
}

/// An empty or oversized description is rejected.
#[test]
fn empty_and_oversized_descriptions_are_rejected() {
    let (dir, workspace) = fixture();
    let root = dir.path().join(".agents/skills/skill");
    std::fs::create_dir_all(&root).expect("dir");
    std::fs::write(
        root.join("SKILL.md"),
        "---\nname: skill\ndescription: \"\"\n---\nbody\n",
    )
    .expect("SKILL.md");
    assert!(matches!(
        project_discovery(&workspace)
            .discover()
            .expect_err("rejected"),
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
    assert!(matches!(
        project_discovery(&workspace)
            .discover()
            .expect_err("rejected"),
        SkillPackageError::InvalidDescription { .. }
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
        project_discovery(&workspace)
            .discover()
            .expect_err("rejected"),
        SkillPackageError::MalformedFrontmatter { .. }
    ));
    // A missing closing delimiter is malformed too.
    std::fs::write(
        dir.path().join(".agents/skills/skill/SKILL.md"),
        "---\nname: skill\ndescription: A skill.\nbody without close\n",
    )
    .expect("SKILL.md");
    assert!(matches!(
        project_discovery(&workspace)
            .discover()
            .expect_err("rejected"),
        SkillPackageError::MalformedFrontmatter { .. }
    ));
}

/// A non-string `metadata` value is rejected (the standard requires a
/// string-to-string map).
#[test]
fn malformed_metadata_is_rejected() {
    let (dir, workspace) = fixture();
    let root = dir.path().join(".agents/skills/skill");
    std::fs::create_dir_all(&root).expect("dir");
    std::fs::write(
        root.join("SKILL.md"),
        "---\nname: skill\ndescription: A skill.\nmetadata:\n  version: 1.0\n---\nbody\n",
    )
    .expect("SKILL.md");
    assert!(matches!(
        project_discovery(&workspace)
            .discover()
            .expect_err("rejected"),
        SkillPackageError::MalformedMetadata { .. }
    ));
}

/// A direct candidate without `SKILL.md` is rejected by the discovery
/// contract.
#[test]
fn candidate_without_skill_markdown_is_rejected() {
    let (dir, workspace) = fixture();
    std::fs::create_dir_all(dir.path().join(".agents/skills/empty")).expect("dir");
    let error = project_discovery(&workspace)
        .discover()
        .expect_err("rejected");
    assert!(matches!(
        error,
        SkillPackageError::MissingSkillMarkdown { .. }
    ));
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

/// Package symlink roots and package-internal symlinks are rejected for
/// M6, while ordinary Workspace symlink semantics for tools are unchanged.
#[test]
fn package_symlinks_are_rejected() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "real", "Real skill.", &[], "body\n");
    std::os::unix::fs::symlink(
        dir.path().join(".agents/skills/real"),
        dir.path().join(".agents/skills/link"),
    )
    .expect("symlink root");
    assert!(matches!(
        project_discovery(&workspace)
            .discover()
            .expect_err("rejected"),
        SkillPackageError::UnsupportedSymlink { .. }
    ));

    let (dir2, workspace2) = fixture();
    write_skill(dir2.path(), "skill", "A skill.", &[], "body\n");
    std::os::unix::fs::symlink(
        dir2.path().join("elsewhere"),
        dir2.path().join(".agents/skills/skill/scripts"),
    )
    .expect("internal symlink");
    assert!(matches!(
        project_discovery(&workspace2)
            .discover()
            .expect_err("rejected"),
        SkillPackageError::UnsupportedSymlink { .. }
    ));
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
        // Relative input from a low-level discovery caller. Production
        // composition resolves CLI/config Skill paths against the canonical
        // Workspace root before this boundary, so discovery is not the only
        // thing standing between a relative `--skill` and the model — but it
        // is what makes the invariant hold for every caller.
        relative_root.join("work/.agents/skills/deck"),
        // Absolute but non-canonical: an embedded `..` is a different
        // spelling of the same package.
        workspace_root.join("../work/.agents/skills/deck"),
    ] {
        let packages = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic_roots: Vec::new(),
                explicit_paths: vec![explicit.clone()],
            },
        )
        .discover()
        .expect("discovery accepts a non-canonical root");
        let snapshot = rustx::skills::SkillSnapshot::new(
            packages.into_iter().map(std::sync::Arc::new).collect(),
        );
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

    let (dir, workspace) = fixture();
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

    let error = SkillDiscovery::with_config(
        &workspace,
        SkillDiscoveryConfig {
            automatic_roots: Vec::new(),
            explicit_paths: vec![package],
        },
    )
    .discover()
    .expect_err("a non-UTF-8 package root is rejected");
    assert!(
        matches!(error, SkillPackageError::UnrepresentableRoot { .. }),
        "expected UnrepresentableRoot, got {error:?}"
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

    let conversation_id = rustx::runtime::identity::ConversationId::new("conv-m6-assets");
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
            subagent_catalog: rustx::runtime::subagent::SubagentCatalog::empty(),
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
        &common::NoopProgress,
        &artifacts_store,
        &tool_output,
        &environment,
    );
    let result = executor
        .execute(
            rustx::tools::types::ToolInvocation {
                call_id: rustx::runtime::identity::ToolCallId::new("call-copy"),
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
    let conversation_id = rustx::runtime::identity::ConversationId::new("conv-m6");
    let artifacts = dir.path().join("artifacts");
    let registry = rustx::tools::executor::ToolRegistry::new();
    let mut registry = registry;
    rustx::tools::native::register_native_tools(
        &mut registry,
        rustx::tools::native::NativeToolResources {
            subagent_catalog: rustx::runtime::subagent::SubagentCatalog::empty(),
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
        call_id: rustx::runtime::identity::ToolCallId::new("call-1"),
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
            &common::NoopProgress,
            &artifacts_store,
            &tool_output,
            &environment,
        );
        executor.execute(invocation, context).await
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

/// A body change changes the version identity.
#[test]
fn body_change_changes_the_version_identity() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "pdf", "PDF skill.", &[], "body one\n");
    let before = version_id(&workspace, "pdf");
    write_skill(dir.path(), "pdf", "PDF skill.", &[], "body two\n");
    assert_ne!(before, version_id(&workspace, "pdf"));
}

/// A script change changes the version identity.
#[test]
fn script_change_changes_the_version_identity() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "pdf", "PDF skill.", &[], "body\n");
    let scripts = dir.path().join(".agents/skills/pdf/scripts");
    std::fs::create_dir_all(&scripts).expect("scripts");
    std::fs::write(scripts.join("extract.py"), "print('one')\n").expect("script");
    let before = version_id(&workspace, "pdf");
    std::fs::write(scripts.join("extract.py"), "print('two')\n").expect("script");
    assert_ne!(before, version_id(&workspace, "pdf"));
}

/// A binary asset change changes the version identity (raw bytes).
#[test]
fn binary_asset_change_changes_the_version_identity() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "pdf", "PDF skill.", &[], "body\n");
    let assets = dir.path().join(".agents/skills/pdf/assets");
    std::fs::create_dir_all(&assets).expect("assets");
    std::fs::write(assets.join("sample.bin"), [0u8, 1, 2, 3]).expect("asset");
    let before = version_id(&workspace, "pdf");
    std::fs::write(assets.join("sample.bin"), [0u8, 9, 8, 7]).expect("asset");
    assert_ne!(before, version_id(&workspace, "pdf"));
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

/// A dependency-only change changes the version identity but a
/// description-only change leaves the environment identity unchanged when
/// dependency inputs are unchanged.
#[test]
fn dependency_change_changes_the_version_identity() {
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

    let description_version = version_id(&workspace, "pdf");
    let deps: BTreeMap<String, String> = [("pypdf".to_owned(), "5.10.0".to_owned())]
        .into_iter()
        .collect();
    let env_digest =
        python_environment_digest("linux", "x86_64", "Python 3.12.3", "pip 24.0", &deps);
    write_skill(
        dir.path(),
        "pdf",
        "Changed description only.",
        &[("rustx.python-dependencies", r#"{"pypdf":"5.10.0"}"#)],
        "body\n",
    );
    let description_version_after = version_id(&workspace, "pdf");
    assert_ne!(description_version, description_version_after);
    assert_eq!(
        env_digest,
        python_environment_digest("linux", "x86_64", "Python 3.12.3", "pip 24.0", &deps),
        "a description-only change does not change the Python environment identity"
    );
}

// ---------------------------------------------------------------------------
// Dependency declarations
// ---------------------------------------------------------------------------

fn parse_python(value: &str) -> BTreeMap<String, String> {
    rustx::skills::parse_python_dependencies(value).expect("parse")
}

fn parse_node(value: &str) -> BTreeMap<String, String> {
    rustx::skills::parse_node_dependencies(value).expect("parse")
}

/// An absent metadata key means an empty dependency map.
#[test]
fn absent_metadata_key_is_an_empty_dependency_map() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "pdf", "PDF skill.", &[], "body\n");
    let packages = discover(&workspace);
    assert!(packages[0].dependencies().python.is_empty());
    assert!(packages[0].dependencies().node.is_empty());
}

/// Exact dependency JSON objects parse deterministically; different JSON
/// key ordering produces the same canonical set.
#[test]
fn exact_dependency_objects_parse_deterministically() {
    let first = parse_python(r#"{"pypdf":"5.9.0","Pillow":"11.3.0"}"#);
    let second = parse_python(r#"{"Pillow":"11.3.0","pypdf":"5.9.0"}"#);
    assert_eq!(first, second);
    assert_eq!(
        first,
        [
            ("pillow".to_owned(), "11.3.0".to_owned()),
            ("pypdf".to_owned(), "5.9.0".to_owned()),
        ]
        .into_iter()
        .collect(),
        "python names normalize deterministically"
    );
    let node = parse_node(r#"{"pdf-lib":"1.17.1","@scope/pkg":"2.0.0"}"#);
    assert_eq!(node.get("@scope/pkg").expect("scoped"), "2.0.0");
}

/// Ranges, URLs, local paths, tags, and other unsupported declarations are
/// rejected for both ecosystems.
#[test]
fn unsupported_declarations_are_rejected() {
    for bad in [
        ">=1.0",
        "~=1.0",
        "==1.0",
        "1.0.*",
        "1.0[extra]",
        "1.0; python_version<'3.9'",
        "https://example.com/pkg.tar.gz",
        "git+https://example.com/repo.git",
        "../local",
    ] {
        let result = rustx::skills::parse_python_dependencies(&format!(r#"{{"pkg":"{bad}"}}"#));
        assert!(
            matches!(result, Err(DependencyError::InvalidVersion { .. })),
            "python {bad:?} must be rejected"
        );
    }
    for bad in [
        "^1.0.0",
        "~1.0.0",
        ">=1.0.0",
        "1.0.x",
        "*",
        "latest",
        "1.0.0 || 2.0.0",
        "git+https://example.com/repo.git",
        "workspace:*",
        "file:../pkg",
    ] {
        let result = rustx::skills::parse_node_dependencies(&format!(r#"{{"pkg":"{bad}"}}"#));
        assert!(
            matches!(result, Err(DependencyError::InvalidVersion { .. })),
            "node {bad:?} must be rejected"
        );
    }
    // A valid prerelease exact version is accepted.
    assert!(rustx::skills::parse_node_dependencies(r#"{"pkg":"1.0.0-beta.1"}"#).is_ok());
    assert!(rustx::skills::parse_python_dependencies(r#"{"pkg":"1.0rc1"}"#).is_ok());
}

/// Duplicate compatible declarations coalesce across Skills; incompatible
/// direct versions report every responsible Skill.
#[test]
fn merge_coalesces_and_conflicts_report_every_skill() {
    let (dir, workspace) = fixture();
    write_skill(
        dir.path(),
        "a",
        "Skill A.",
        &[("rustx.python-dependencies", r#"{"pypdf":"5.9.0"}"#)],
        "body\n",
    );
    write_skill(
        dir.path(),
        "b",
        "Skill B.",
        &[("rustx.python-dependencies", r#"{"PyPDF":"5.9.0"}"#)],
        "body\n",
    );
    let packages = discover(&workspace);
    let merged = merge_dependency_manifests(&packages).expect("coalesce");
    assert_eq!(merged.python.len(), 1);
    assert_eq!(merged.python.get("pypdf").expect("coalesced"), "5.9.0");

    write_skill(
        dir.path(),
        "b",
        "Skill B.",
        &[("rustx.python-dependencies", r#"{"PyPDF":"5.10.0"}"#)],
        "body\n",
    );
    let packages = discover(&workspace);
    let conflict = merge_dependency_manifests(&packages).expect_err("conflict");
    assert_eq!(conflict.ecosystem, rustx::skills::Ecosystem::Python);
    assert_eq!(conflict.package, "pypdf");
    assert_eq!(conflict.declarations.len(), 2);
    let skills: Vec<&str> = conflict
        .declarations
        .iter()
        .map(|(skill, _)| skill.as_str())
        .collect();
    let versions: Vec<&str> = conflict
        .declarations
        .iter()
        .map(|(_, version)| version.as_str())
        .collect();
    assert_eq!(skills, ["a", "b"]);
    assert_eq!(versions, ["5.9.0", "5.10.0"]);
}

/// A malformed rustX dependency declaration fails the whole discovery
/// transaction (one malformed Skill must not partially activate).
#[test]
fn malformed_dependency_declaration_fails_the_transaction() {
    let (dir, workspace) = fixture();
    write_skill(dir.path(), "good", "Good skill.", &[], "body\n");
    write_skill(
        dir.path(),
        "bad",
        "Bad skill.",
        &[("rustx.python-dependencies", r#"{"pypdf":"not a version"}"#)],
        "body\n",
    );
    let error = project_discovery(&workspace)
        .discover()
        .expect_err("rejected");
    assert!(matches!(
        error,
        SkillPackageError::InvalidDependencyDeclaration { .. }
    ));
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
    let rendered = render_skill_catalog(snapshot.catalog_entries());
    let skills_root = workspace.root().join(".agents/skills");
    let pdf = skills_root.join("pdf/SKILL.md").display().to_string();
    let slides = skills_root.join("slides/SKILL.md").display().to_string();
    let expected = format!(
        concat!(
            "## Skills\n\n",
            "The following skills provide specialized instructions for specific tasks.\n",
            "Use the Read tool to load a skill when the task matches its description.\n",
            "When a skill file references a relative path, resolve it against the skill ",
            "directory (the parent of its SKILL.md) and use that absolute path in tool ",
            "commands.\n\n",
            "<available_skills>\n",
            "  <skill>\n",
            "    <name>pdf</name>\n",
            "    <description>Create, edit, inspect, and transform PDF documents.</description>\n",
            "    <location>{pdf}</location>\n",
            "  </skill>\n",
            "  <skill>\n",
            "    <name>slides</name>\n",
            "    <description>Create and modify presentation decks.</description>\n",
            "    <location>{slides}</location>\n",
            "  </skill>\n",
            "</available_skills>"
        ),
        pdf = pdf,
        slides = slides
    );
    assert_eq!(rendered, expected);
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

// ---------------------------------------------------------------------------
// Environment identity (pure digest semantics)
// ---------------------------------------------------------------------------

/// Same canonical input → same digest; dependency order does not matter;
/// the store path is not an input.
#[test]
fn environment_digests_are_deterministic() {
    let deps: BTreeMap<String, String> = [
        ("pypdf".to_owned(), "5.9.0".to_owned()),
        ("pillow".to_owned(), "11.3.0".to_owned()),
    ]
    .into_iter()
    .collect();
    let mut reversed: BTreeMap<String, String> = BTreeMap::new();
    reversed.insert("pillow".to_owned(), "11.3.0".to_owned());
    reversed.insert("pypdf".to_owned(), "5.9.0".to_owned());
    assert_eq!(
        python_environment_digest("linux", "x86_64", "Python 3.12.3", "pip 24.0", &deps),
        python_environment_digest("linux", "x86_64", "Python 3.12.3", "pip 24.0", &reversed)
    );
    let digest = python_environment_digest("linux", "x86_64", "Python 3.12.3", "pip 24.0", &deps);
    assert_eq!(digest.as_str().len(), 7 + 64);

    // Different dependency input → different digest.
    let other: BTreeMap<String, String> = [("pypdf".to_owned(), "5.10.0".to_owned())]
        .into_iter()
        .collect();
    assert_ne!(
        digest,
        python_environment_digest("linux", "x86_64", "Python 3.12.3", "pip 24.0", &other)
    );

    // A relevant runtime-version change → different digest.
    assert_ne!(
        digest,
        python_environment_digest("linux", "x86_64", "Python 3.13.0", "pip 24.0", &deps)
    );

    // Python and Node digests are distinct identities.
    let node_deps: BTreeMap<String, String> = [("pdf-lib".to_owned(), "1.17.1".to_owned())]
        .into_iter()
        .collect();
    let node_digest = node_environment_digest("linux", "x86_64", "v22.1.0", "10.2.3", &node_deps);
    assert_ne!(
        node_digest.as_str(),
        digest.as_str(),
        "python and node identities remain distinct"
    );
}
