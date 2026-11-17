use super::*;
use crate::local_runtime::launch::{HostEnvironment, LaunchRequest};
use crate::runtime::agent_profile::AgentSkillSelection;

fn fixture() -> (tempfile::TempDir, UserConfigManager, SourceTarget) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let home = root.join("home");
    let workspace = root.join("workspace");
    fs::create_dir_all(home.join("rustx")).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::write(
        home.join("rustx/rustx.toml"),
        r#"
[providers.local]
protocol = "openai_responses"
base_url = "http://127.0.0.1:9/v1"
api_key = "$RUSTX_SKILL_TRANSFER_UNUSED"
[models.local]
provider = "local"
id = "local"
context_window = 128000
max_output_tokens = 8192
[models.local.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
[agent.model]
model = "local"
"#,
    )
    .unwrap();
    let host = HostEnvironment::from_paths(workspace.clone(), home).unwrap();
    let request = LaunchRequest {
        workspace: Some(workspace.clone()),
        ..LaunchRequest::default()
    };
    let manager = request.session_input(&host).unwrap().0;
    assert!(
        manager
            .capture_authored_settings(&SourceTarget::User, None)
            .unwrap()
            .0
            .is_some(),
        "the transfer fixture must have a valid authored model catalog"
    );
    (
        temporary,
        manager,
        SourceTarget::Workspace {
            directory: workspace,
        },
    )
}

#[test]
fn skill_copy_rejects_names_already_selected_by_a_named_agent() {
    for from_user in [true, false] {
        for global_profile in [true, false] {
            let (_temporary, manager, workspace) = fixture();
            let user = SourceTarget::User;
            let (source, destination) = if from_user {
                (&user, &workspace)
            } else {
                (&workspace, &user)
            };
            let collection = manager.skill_collection(source);
            let original = collection.join("original");
            fs::create_dir_all(&original).unwrap();
            fs::write(
                original.join("SKILL.md"),
                "---\nname: original\ndescription: Original skill\n---\nInstructions\n",
            )
            .unwrap();
            let package = manager
                .discover_skill_scope(source, &collection)
                .packages
                .pop()
                .unwrap();
            let revision = operation_revision(&package, &collection).unwrap();
            let profile_root = manager
                .resource_root(if global_profile { &user } else { &workspace })
                .join("agents");
            fs::create_dir_all(&profile_root).unwrap();
            let profile = profile_root.join("reviewer.toml");
            fs::write(
                &profile,
                "description = 'Review'\ninstructions = 'Review input'\nskills = ['copied']\n",
            )
            .unwrap();

            let result = manager.transfer_skill(
                source,
                destination,
                TransferOperation::Copy,
                "original",
                "copied",
                &revision,
            );
            assert!(
                matches!(
                    &result,
                    Err(SettingsError::TransferRejected(
                        TransferRejection::NameEnabled
                    ))
                ),
                "from_user={from_user}, global_profile={global_profile}: {result:?}"
            );
            assert!(original.join("SKILL.md").is_file());
            assert!(
                !manager
                    .skill_collection(destination)
                    .join("copied")
                    .exists()
            );

            fs::write(
                &profile,
                "description = 'Review'\ninstructions = 'Review input'\nskills = []\n",
            )
            .unwrap();
            manager
                .transfer_skill(
                    source,
                    destination,
                    TransferOperation::Copy,
                    "original",
                    "copied",
                    &revision,
                )
                .unwrap();
            let copied = manager
                .discover_skill_scope(destination, &manager.skill_collection(destination))
                .packages
                .into_iter()
                .find(|package| package.name() == "copied")
                .unwrap();
            assert!(!copied.enabled_by_default());
            assert!(!AgentSkillSelection::All.selects_package(&copied));
            assert!(
                AgentSkillSelection::Exact(vec!["copied".into()]).selects_package(&copied),
                "explicit enabling still overrides the default"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn skill_copy_and_move_preserve_root_and_subdirectory_modes() {
    use std::os::unix::fs::PermissionsExt;
    for mode in [0o700, 0o750, 0o500, 0o550, 0o555] {
        for operation in [TransferOperation::Copy, TransferOperation::Move] {
            let (_temporary, manager, workspace) = fixture();
            let source = &SourceTarget::User;
            let destination = &workspace;
            let root = manager.skill_collection(source).join("private-skill");
            let nested = root.join("references").join("private");
            fs::create_dir_all(&nested).unwrap();
            fs::write(
                root.join("SKILL.md"),
                "---\nname: private-skill\ndescription: Private\n---\nInstructions\n",
            )
            .unwrap();
            fs::write(nested.join("note.txt"), "secret").unwrap();
            let allow = |path: &std::path::Path, mode| {
                fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
            };
            allow(&root, mode);
            allow(&root.join("references"), mode);
            allow(&nested, mode);
            allow(&nested.join("note.txt"), 0o644);
            allow(&root.join("SKILL.md"), 0o640);
            let package = manager
                .discover_skill_scope(source, &manager.skill_collection(source))
                .packages
                .pop()
                .unwrap();
            let revision = operation_revision(&package, &manager.skill_collection(source)).unwrap();
            let destination_name = if operation == TransferOperation::Copy {
                "copied-skill"
            } else {
                "private-skill"
            };
            manager
                .transfer_skill(
                    source,
                    destination,
                    operation,
                    "private-skill",
                    destination_name,
                    &revision,
                )
                .unwrap_or_else(|error| panic!("{operation:?} mode {mode:o}: {error:?}"));
            let copied = manager.skill_collection(destination).join(destination_name);
            let copied_nested = copied.join("references").join("private");
            let mode_of =
                |path: &std::path::Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode_of(&copied), mode, "{operation:?} root");
            assert_eq!(
                mode_of(&copied.join("references")),
                mode,
                "{operation:?} references"
            );
            assert_eq!(mode_of(&copied_nested), mode, "{operation:?} private");
            assert_eq!(mode_of(&copied.join("SKILL.md")), 0o640);
            assert_eq!(mode_of(&copied_nested.join("note.txt")), 0o644);
            assert_eq!(
                fs::read_to_string(copied_nested.join("note.txt")).unwrap(),
                "secret"
            );
            assert_eq!(root.exists(), operation == TransferOperation::Copy);
            assert_no_skill_stages(&manager.skill_collection(source));
            assert_no_skill_stages(&manager.skill_collection(destination));
        }
    }
}

fn assert_no_skill_stages(collection: &std::path::Path) {
    let entries = fs::read_dir(collection).unwrap();
    for entry in entries.flatten() {
        let name = entry.file_name();
        assert!(
            !name.to_string_lossy().starts_with(".rustx-skill-"),
            "unpublished stage leaked at {}",
            entry.path().display()
        );
    }
}

fn make_writable(path: &std::path::Path) {
    if path.exists() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn install_private_skill(manager: &UserConfigManager, mode: u32) -> String {
    let source = &SourceTarget::User;
    let root = manager.skill_collection(source).join("private-skill");
    let nested = root.join("references").join("private");
    if root.exists() {
        make_writable(&root);
        make_writable(&root.join("references"));
        make_writable(&nested);
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        root.join("SKILL.md"),
        "---\nname: private-skill\ndescription: Private\n---\nInstructions\n",
    )
    .unwrap();
    fs::write(nested.join("note.txt"), "secret").unwrap();
    for path in [&root, &root.join("references"), &nested] {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
    let package = manager
        .discover_skill_scope(source, &manager.skill_collection(source))
        .packages
        .into_iter()
        .find(|package| package.name() == "private-skill")
        .unwrap();
    operation_revision(&package, &manager.skill_collection(source)).unwrap()
}

#[cfg(unix)]
#[test]
fn nonwritable_skill_delete_and_aborted_stage_leave_no_retired_directory() {
    use std::os::unix::fs::PermissionsExt;
    let (_temporary, manager, workspace) = fixture();
    let sibling = manager
        .skill_collection(&SourceTarget::User)
        .join("sibling");
    fs::create_dir_all(sibling.join("nested")).unwrap();
    fs::write(
        sibling.join("SKILL.md"),
        "---\nname: sibling\ndescription: Keep\n---\nStay\n",
    )
    .unwrap();
    fs::set_permissions(&sibling, fs::Permissions::from_mode(0o500)).unwrap();
    let revision = install_private_skill(&manager, 0o500);
    manager
        .delete_skill(&SourceTarget::User, "private-skill", &revision)
        .unwrap();
    assert!(
        !manager
            .skill_collection(&SourceTarget::User)
            .join("private-skill")
            .exists()
    );
    assert_eq!(
        fs::metadata(&sibling).unwrap().permissions().mode() & 0o777,
        0o500
    );
    assert_no_skill_stages(&manager.skill_collection(&SourceTarget::User));

    let revision = install_private_skill(&manager, 0o550);
    let prepared = manager
        .prepare_skill_transfer(
            &SourceTarget::User,
            &workspace,
            TransferOperation::Copy,
            "private-skill",
            "copied-skill",
            &revision,
        )
        .unwrap();
    drop(prepared);
    assert_eq!(
        fs::metadata(
            manager
                .skill_collection(&SourceTarget::User)
                .join("private-skill")
        )
        .unwrap()
        .permissions()
        .mode()
            & 0o777,
        0o550
    );
    assert_no_skill_stages(&manager.skill_collection(&workspace));
}

#[cfg(unix)]
#[test]
fn staging_reports_source_conflict_invalid_and_destination_rejection_without_leaks() {
    let (_temporary, manager, workspace) = fixture();
    let source = SourceTarget::User;
    let revision = install_private_skill(&manager, 0o500);
    let root = manager.skill_collection(&source).join("private-skill");

    let prepared = manager
        .prepare_skill_transfer(
            &source,
            &workspace,
            TransferOperation::Copy,
            "private-skill",
            "copied-skill",
            &revision,
        )
        .unwrap();
    make_writable(&root);
    fs::write(
        root.join("SKILL.md"),
        "---\nname: private-skill\ndescription: Edited\n---\nChanged\n",
    )
    .unwrap();
    let conflict = manager.commit_skill_transfer(prepared);
    assert!(
        matches!(conflict, Err(SettingsError::Conflict { .. })),
        "revision change: {conflict:?}"
    );
    assert!(!workspace_skill(&manager, &workspace, "copied-skill").exists());
    assert_no_skill_stages(&manager.skill_collection(&workspace));

    let revision = install_private_skill(&manager, 0o500);
    let prepared = manager
        .prepare_skill_transfer(
            &source,
            &workspace,
            TransferOperation::Copy,
            "private-skill",
            "copied-skill",
            &revision,
        )
        .unwrap();
    make_writable(&root);
    make_writable(&root.join("references"));
    make_writable(&root.join("references").join("private"));
    fs::write(
        root.join("references").join("private").join("note.txt"),
        "changed",
    )
    .unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).unwrap();
    fs::set_permissions(root.join("references"), fs::Permissions::from_mode(0o500)).unwrap();
    fs::set_permissions(
        root.join("references").join("private"),
        fs::Permissions::from_mode(0o500),
    )
    .unwrap();
    let invalid = manager.commit_skill_transfer(prepared);
    assert!(
        matches!(invalid, Err(SettingsError::Invalid)),
        "supporting file change: {invalid:?}"
    );
    assert_no_skill_stages(&manager.skill_collection(&workspace));
}

#[cfg(unix)]
#[test]
fn staging_rejects_mode_and_destination_conflicts_without_leaks() {
    let (_temporary, manager, workspace) = fixture();
    let source = SourceTarget::User;
    let root = manager.skill_collection(&source).join("private-skill");
    let revision = install_private_skill(&manager, 0o550);
    let prepared = manager
        .prepare_skill_transfer(
            &source,
            &workspace,
            TransferOperation::Move,
            "private-skill",
            "private-skill",
            &revision,
        )
        .unwrap();
    fs::set_permissions(root.join("references"), fs::Permissions::from_mode(0o500)).unwrap();
    let mode_change = manager.commit_skill_transfer(prepared);
    assert!(
        matches!(mode_change, Err(SettingsError::Invalid)),
        "mode change: {mode_change:?}"
    );
    assert!(root.join("SKILL.md").is_file());
    assert_no_skill_stages(&manager.skill_collection(&workspace));

    let revision = install_private_skill(&manager, 0o500);
    let prepared = manager
        .prepare_skill_transfer(
            &source,
            &workspace,
            TransferOperation::Copy,
            "private-skill",
            "copied-skill",
            &revision,
        )
        .unwrap();
    let taken = manager.skill_collection(&workspace).join("copied-skill");
    fs::create_dir_all(&taken).unwrap();
    fs::write(
        taken.join("SKILL.md"),
        "---\nname: copied-skill\ndescription: There\n---\nAlready\n",
    )
    .unwrap();
    let name_taken = manager.commit_skill_transfer(prepared);
    assert!(
        matches!(
            name_taken,
            Err(SettingsError::TransferRejected(
                TransferRejection::NameTaken
            ))
        ),
        "destination identity: {name_taken:?}"
    );
    assert_no_skill_stages(&manager.skill_collection(&workspace));
    fs::remove_dir_all(&taken).unwrap();

    let revision = install_private_skill(&manager, 0o555);
    let prepared = manager
        .prepare_skill_transfer(
            &source,
            &workspace,
            TransferOperation::Copy,
            "private-skill",
            "copied-skill",
            &revision,
        )
        .unwrap();
    let profile = manager.resource_root(&source).join("agents");
    fs::create_dir_all(&profile).unwrap();
    fs::write(
        profile.join("reviewer.toml"),
        "description = 'Review'\ninstructions = 'Review input'\nskills = ['copied-skill']\n",
    )
    .unwrap();
    let enabled = manager.commit_skill_transfer(prepared);
    assert!(
        matches!(
            enabled,
            Err(SettingsError::TransferRejected(
                TransferRejection::NameEnabled
            ))
        ),
        "destination selection: {enabled:?}"
    );
    assert!(!workspace_skill(&manager, &workspace, "copied-skill").exists());
    assert_no_skill_stages(&manager.skill_collection(&workspace));
}

#[cfg(unix)]
#[test]
fn copy_failure_classifies_source_mutation_and_preserves_unchanged_io_errors() {
    let (_temporary, manager, workspace) = fixture();
    let source = SourceTarget::User;
    let revision = install_private_skill(&manager, 0o700);
    let root = manager.skill_collection(&source).join("private-skill");
    let skill_md = root.join("SKILL.md");
    manager.test_hooks.insert("skill_content_preparing", {
        let skill_md = skill_md.clone();
        move || {
            fs::write(
                &skill_md,
                "---\nname: private-skill\ndescription: Edited\n---\nChanged\n",
            )
            .unwrap();
        }
    });
    let conflict = manager.prepare_skill_transfer(
        &source,
        &workspace,
        TransferOperation::Copy,
        "private-skill",
        "copied-skill",
        &revision,
    );
    assert!(
        matches!(conflict, Err(SettingsError::Conflict { .. })),
        "in-copy revision change: {}",
        conflict.as_ref().err().map_or("ok", settings_error_name)
    );
    assert_no_skill_stages(&manager.skill_collection(&workspace));

    let revision = install_private_skill(&manager, 0o700);
    let note = root.join("references").join("private").join("note.txt");
    manager.test_hooks.insert("skill_content_preparing", {
        let note = note.clone();
        move || {
            fs::write(&note, "changed during copy").unwrap();
        }
    });
    let invalid = manager.prepare_skill_transfer(
        &source,
        &workspace,
        TransferOperation::Copy,
        "private-skill",
        "copied-skill",
        &revision,
    );
    assert!(
        matches!(invalid, Err(SettingsError::Invalid)),
        "in-copy supporting change: {}",
        invalid.as_ref().err().map_or("ok", settings_error_name)
    );
    assert_no_skill_stages(&manager.skill_collection(&workspace));
}

#[cfg(unix)]
#[test]
fn unchanged_source_copy_failure_stays_io_and_removes_the_stage() {
    let (_temporary, manager, workspace) = fixture();
    let source = SourceTarget::User;
    let revision = install_private_skill(&manager, 0o700);
    let root = manager.skill_collection(&source).join("private-skill");
    let destination_collection = manager.skill_collection(&workspace);
    manager.test_hooks.insert("skill_content_preparing", {
        let destination_collection = destination_collection.clone();
        move || {
            let stage = fs::read_dir(&destination_collection)
                .unwrap()
                .flatten()
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".rustx-skill-")
                })
                .unwrap()
                .path();
            fs::set_permissions(&stage, fs::Permissions::from_mode(0o500)).unwrap();
        }
    });
    let io_error = manager.prepare_skill_transfer(
        &source,
        &workspace,
        TransferOperation::Copy,
        "private-skill",
        "copied-skill",
        &revision,
    );
    assert!(
        matches!(io_error, Err(SettingsError::Io)),
        "unchanged source must stay an I/O failure: {}",
        io_error.as_ref().err().map_or("ok", settings_error_name)
    );
    assert!(root.join("SKILL.md").is_file());
    assert_no_skill_stages(&destination_collection);
}

#[cfg(unix)]
#[test]
fn owned_stage_cleanup_does_not_follow_or_chmod_a_replacement() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = tempfile::tempdir().unwrap();
    // PackageDirectory::open walks every ancestor with O_DIRECTORY|O_NOFOLLOW.
    // macOS TMPDIR is under /var, a symlink to /private/var; that open returns
    // ENOTDIR. Other fixtures canonicalize first. Confinement stays unchanged.
    let canonical = temporary.path().canonicalize().unwrap();
    let alias = canonical.join("alias");
    std::os::unix::fs::symlink(&canonical, &alias).unwrap();
    let through_alias = alias.join("owned");
    fs::create_dir(canonical.join("owned")).unwrap();
    match crate::package_io::PackageDirectory::open(&through_alias) {
        Err(error) => {
            let confined = error.kind() == std::io::ErrorKind::NotADirectory
                || error.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error());
            assert!(confined, "symlink ancestor must stay confined: {error}");
        }
        Ok(_) => panic!("symlink ancestor must stay confined, not followed"),
    }
    let root = canonical.join("owned");
    let secret = canonical.join("secret");
    fs::create_dir_all(secret.join("nested")).unwrap();
    fs::write(secret.join("nested").join("keep"), "keep").unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o500)).unwrap();
    fs::create_dir_all(root.join("child")).unwrap();
    fs::write(root.join("child").join("note"), "note").unwrap();
    std::os::unix::fs::symlink(&secret, root.join("link")).unwrap();
    fs::set_permissions(root.join("child"), fs::Permissions::from_mode(0o500)).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).unwrap();
    let identity = {
        let metadata = fs::metadata(&root).unwrap();
        (metadata.dev(), metadata.ino())
    };
    super::remove_owned_tree(&root, identity).unwrap();
    assert!(!root.exists());
    assert_eq!(
        fs::metadata(&secret).unwrap().permissions().mode() & 0o777,
        0o500
    );
    assert_eq!(
        fs::read_to_string(secret.join("nested").join("keep")).unwrap(),
        "keep"
    );

    let replaced = canonical.join("replaced");
    fs::create_dir(&replaced).unwrap();
    let old_identity = {
        let metadata = fs::metadata(&replaced).unwrap();
        (metadata.dev(), metadata.ino())
    };
    fs::rename(&replaced, canonical.join("moved-aside")).unwrap();
    fs::create_dir(&replaced).unwrap();
    fs::write(replaced.join("keep"), "foreign").unwrap();
    fs::set_permissions(&replaced, fs::Permissions::from_mode(0o500)).unwrap();
    let cleanup = super::remove_owned_tree(&replaced, old_identity);
    assert!(cleanup.is_err(), "a replaced directory must not be cleaned");
    assert_eq!(
        fs::metadata(&replaced).unwrap().permissions().mode() & 0o777,
        0o500
    );
    assert_eq!(
        fs::read_to_string(replaced.join("keep")).unwrap(),
        "foreign"
    );
}

fn settings_error_name(error: &SettingsError) -> &'static str {
    match error {
        SettingsError::Conflict { .. } => "conflict",
        SettingsError::Invalid => "invalid",
        SettingsError::Io => "io",
        SettingsError::Committed => "committed",
        SettingsError::TransferRejected(_) => "rejected",
        SettingsError::InvalidWorkflow(_) => "workflow",
    }
}

fn workspace_skill(
    manager: &UserConfigManager,
    workspace: &SourceTarget,
    name: &str,
) -> std::path::PathBuf {
    manager.skill_collection(workspace).join(name)
}
