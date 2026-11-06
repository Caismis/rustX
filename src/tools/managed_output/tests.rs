use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

const CONVERSATION: &str = "conv_01900000-0000-7000-8000-000000000001";

fn store(root: &Path) -> ManagedToolOutput {
    std::fs::create_dir_all(root.join("temporary root with spaces")).unwrap();
    ManagedToolOutput::in_temp_root(
        ConversationId::new(CONVERSATION),
        &root.join("private"),
        &root.join("temporary root with spaces"),
    )
    .unwrap()
}

#[test]
fn short_flat_private_files_retain_streamed_utf8_after_drop() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let namespace = store.root().unwrap();
    assert_eq!(namespace.file_name().unwrap().len(), 19);
    assert_eq!(
        namespace.parent().unwrap(),
        dir.path()
            .join("temporary root with spaces")
            .canonicalize()
            .unwrap()
    );
    assert_eq!(
        std::fs::metadata(&namespace).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let mut spill = store.open_spill().unwrap();
    let path = spill.path().to_owned();
    assert_eq!(path.parent().unwrap(), namespace);
    assert_eq!(path.file_name().unwrap().len(), 22);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    spill.write_all("prefix\n").unwrap();
    spill.write_all("😀suffix").unwrap();
    drop(spill);
    drop(store);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "prefix\n😀suffix");
    assert!(!dir.path().join("private/tool-output").exists());
}

#[test]
fn bounded_file_collisions_and_deleted_names_never_overwrite_or_alias() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let a = "0000000000000001".to_owned();
    let b = "0000000000000002".to_owned();
    store
        .state
        .lock()
        .unwrap()
        .namespaces
        .set_names([a.clone()]);
    let mut first = store.open_spill().unwrap();
    first.write_all("original").unwrap();
    store
        .state
        .lock()
        .unwrap()
        .namespaces
        .set_names(std::iter::repeat_n(a.clone(), storage::RETRIES));
    assert!(matches!(
        store.open_spill(),
        Err(ManagedOutputError::IdentityCollision)
    ));
    assert_eq!(std::fs::read_to_string(first.path()).unwrap(), "original");
    std::fs::remove_file(first.path()).unwrap();
    store.state.lock().unwrap().namespaces.set_names([a, b]);
    let second = store.open_spill().unwrap();
    assert_ne!(first.path(), second.path());
    assert!(!first.path().exists());
}

#[test]
fn planted_leaf_symlink_is_a_collision_and_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let target = dir.path().join("user.txt");
    std::fs::write(&target, "user content").unwrap();
    symlink(
        &target,
        store.root().unwrap().join("r-0000000000000001.txt"),
    )
    .unwrap();
    store
        .state
        .lock()
        .unwrap()
        .namespaces
        .set_names(["0000000000000001".into(), "0000000000000002".into()]);
    let mut spill = store.open_spill().unwrap();
    spill.write_all("output").unwrap();
    assert_eq!(std::fs::read_to_string(target).unwrap(), "user content");
    assert!(spill.path().ends_with("r-0000000000000002.txt"));
}

#[test]
fn background_allocation_binds_full_identity_and_preserves_live_locator() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let execution = store.execution_identity().unwrap();
    let path = store.allocate_background_output(&execution).unwrap();
    assert_eq!(path.file_name().unwrap().len(), 25);
    assert!(!path.to_string_lossy().contains(execution.as_str()));
    assert_eq!(store.background_output_path(&execution), path);
    assert_eq!(std::fs::read(&path).unwrap(), b"");
    let mut sink = store.open_background_output_sink(&execution).unwrap();
    sink.append("live").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "live");
    assert!(store.allocate_background_output(&execution).is_err());
    std::fs::remove_file(&path).unwrap();
    symlink(dir.path().join("user.txt"), &path).unwrap();
    assert!(store.open_background_output_sink(&execution).is_err());
    drop(sink);
    store.discard_background_output(&execution);
    assert!(!path.exists());
}

#[test]
fn rollback_and_write_failures_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    store.set_force_open_failures(true);
    let execution = store.execution_identity().unwrap();
    assert!(store.allocate_background_output(&execution).is_err());
    assert!(store.open_spill().is_err());
    store.set_force_open_failures(false);
    let path = store.allocate_background_output(&execution).unwrap();
    store.discard_background_output(&execution);
    assert!(!path.exists());
    store.discard_background_output(&execution);
    store.fail_writes_after(4);
    let mut spill = store.open_spill().unwrap();
    assert!(spill.write_all("abcdef").is_err());
    assert_eq!(std::fs::read(spill.path()).unwrap(), b"abcd");
    assert!(spill.write_all("x").is_err());
}

#[test]
fn external_removal_rotates_namespace_without_repairing_old_locators() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let old = store.open_spill().unwrap().path().to_owned();
    let old_root = store.root().unwrap();
    std::fs::remove_dir_all(&old_root).unwrap();
    let mut next = store.open_spill().unwrap();
    next.write_all("new output").unwrap();
    assert_ne!(store.root().unwrap(), old_root);
    assert!(!old.exists());
    assert!(!old_root.exists());
    assert!(store.ensure_model_mutation_allowed(&old).is_err());
    assert!(store.ensure_model_mutation_allowed(dir.path()).is_ok());
    assert!(store.validate_locator(&old).is_err());
}

#[test]
fn cold_reconstruction_retains_valid_references_and_changes_allocation_namespace() {
    let dir = tempfile::tempdir().unwrap();
    let first = store(dir.path());
    let mut spill = first.open_spill().unwrap();
    spill.write_all("retained").unwrap();
    let old = spill.path().to_owned();
    drop(spill);
    drop(first);
    let second = store(dir.path());
    second.validate_locator(&old).unwrap();
    assert_eq!(std::fs::read_to_string(&old).unwrap(), "retained");
    assert_ne!(old.parent().unwrap(), second.root().unwrap());
    std::fs::remove_dir_all(old.parent().unwrap()).unwrap();
    drop(second);
    let third = store(dir.path());
    assert!(third.validate_locator(&old).is_err());
    assert!(third.open_spill().is_ok());
    assert!(!old.exists());
}

#[test]
fn concurrent_conversations_and_products_do_not_mix_or_delete_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let temporary = dir.path().join("shared temporary root with spaces");
    std::fs::create_dir(&temporary).unwrap();
    let first = ManagedToolOutput::new_in_temp_root(
        ConversationId::new(CONVERSATION),
        &dir.path().join("product-a/private"),
        &temporary,
    )
    .unwrap();
    let second = ManagedToolOutput::new_in_temp_root(
        ConversationId::generate(),
        &dir.path().join("product-b/private"),
        &temporary,
    )
    .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(9));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let store = if i % 2 == 0 {
                first.clone()
            } else {
                second.clone()
            };
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let mut output = store.open_spill().unwrap();
                output.write_all(&i.to_string()).unwrap();
                (output.path().to_owned(), i.to_string())
            })
        })
        .collect();
    barrier.wait();
    let outputs: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let first_root = first.root().unwrap();
    drop(first);
    storage::cleanup(
        &dir.path().join("product-a"),
        &dir.path().join("product-a/private").canonicalize().unwrap(),
        &ConversationId::new(CONVERSATION),
    )
    .unwrap();
    for (path, expected) in outputs {
        if path.starts_with(&first_root) {
            assert!(!path.exists());
        } else {
            assert_eq!(std::fs::read_to_string(path).unwrap(), expected);
        }
    }
}

#[test]
fn cleanup_rejects_replaced_directory_and_unexpected_subdirectory() {
    let dir = tempfile::tempdir().unwrap();
    let output = store(dir.path());
    let root = output.root().unwrap();
    let original = root.with_extension("moved");
    std::fs::rename(&root, &original).unwrap();
    symlink(dir.path(), &root).unwrap();
    let owner = dir.path().join("private").canonicalize().unwrap();
    assert!(storage::cleanup(dir.path(), &owner, &ConversationId::new(CONVERSATION)).is_err());
    std::fs::remove_file(&root).unwrap();
    std::fs::rename(&original, &root).unwrap();
    std::fs::create_dir(root.join("unexpected")).unwrap();
    assert!(storage::cleanup(dir.path(), &owner, &ConversationId::new(CONVERSATION)).is_err());
    let binding: serde_json::Value =
        serde_json::from_slice(&std::fs::read(owner.join("output-namespaces.json")).unwrap())
            .unwrap();
    let arena = PathBuf::from(
        binding["namespaces"][0]["retirement"]["arena"]["root"]
            .as_str()
            .unwrap(),
    );
    let retired = arena.join(format!("{CONVERSATION}-n0"));
    assert!(retired.join(".owner").exists());
    std::fs::remove_dir(retired.join("unexpected")).unwrap();
    storage::cleanup(dir.path(), &owner, &ConversationId::new(CONVERSATION)).unwrap();
    storage::cleanup(dir.path(), &owner, &ConversationId::new(CONVERSATION)).unwrap();
    assert!(!root.exists());
}

#[test]
fn a_symlinked_system_temp_root_is_canonicalized_legitimately() {
    let dir = tempfile::tempdir().unwrap();
    let actual = dir.path().join("actual");
    std::fs::create_dir(&actual).unwrap();
    let alias = dir.path().join("alias");
    symlink(&actual, &alias).unwrap();
    let output = ManagedToolOutput::in_temp_root(
        ConversationId::new(CONVERSATION),
        &dir.path().join("private"),
        &alias,
    )
    .unwrap();
    assert!(
        output
            .open_spill()
            .unwrap()
            .path()
            .starts_with(actual.canonicalize().unwrap())
    );
}

#[test]
fn cold_restart_child() {
    let Some(root) = std::env::var_os("RUSTX_520_COLD_ROOT") else {
        return;
    };
    let root = Path::new(&root);
    let output = store(root);
    if let Some(old) = std::env::var_os("RUSTX_520_COLD_OLD") {
        let old = Path::new(&old);
        output.validate_locator(old).unwrap();
        assert_eq!(std::fs::read_to_string(old).unwrap(), "from first process");
        assert_ne!(old.parent().unwrap(), output.root().unwrap());
    } else {
        let mut spill = output.open_spill().unwrap();
        spill.write_all("from first process").unwrap();
        println!("COLD_LOCATOR:{}", spill.path().display());
    }
}

#[test]
fn genuine_cold_restart_keeps_the_original_locator() {
    let root = tempfile::tempdir().unwrap();
    let spawn = |old: Option<&str>| {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "tools::managed_output::tests::cold_restart_child",
                "--nocapture",
            ])
            .env("RUSTX_520_COLD_ROOT", root.path());
        if let Some(old) = old {
            command.env("RUSTX_520_COLD_OLD", old);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let stdout = spawn(None);
    let locator = stdout
        .lines()
        .find_map(|line| line.strip_prefix("COLD_LOCATOR:"))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(locator).unwrap(),
        "from first process"
    );
    spawn(Some(locator));
}

#[test]
fn recovery_does_not_require_the_current_temporary_root() {
    let dir = tempfile::tempdir().unwrap();
    let first = store(dir.path());
    let mut spill = first.open_spill().unwrap();
    spill.write_all("old output").unwrap();
    let old = spill.path().to_owned();
    drop((first, spill));
    let resumed = ManagedToolOutput::new_in_temp_root(
        ConversationId::new(CONVERSATION),
        &dir.path().join("private"),
        &dir.path().join("missing temp root"),
    )
    .unwrap();
    resumed.validate_locator(&old).unwrap();
    assert_eq!(std::fs::read_to_string(&old).unwrap(), "old output");
    assert!(resumed.open_spill().is_err());
    assert_eq!(std::fs::read_to_string(old).unwrap(), "old output");
}

#[test]
fn fifo_replacements_fail_without_waiting_for_a_peer() {
    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;
    let dir = tempfile::tempdir().unwrap();
    let output = store(dir.path());
    let execution = output.execution_identity().unwrap();
    let path = output.allocate_background_output(&execution).unwrap();
    std::fs::remove_file(&path).unwrap();
    mkfifo(&path, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
    assert!(output.open_background_output_sink(&execution).is_err());
    let marker = output.root().unwrap().join(".owner");
    std::fs::remove_file(&marker).unwrap();
    mkfifo(&marker, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
    assert!(output.validate_locator(&path).is_err());
}

#[test]
fn regular_background_replacement_is_rejected_and_settlement_releases_handle() {
    let dir = tempfile::tempdir().unwrap();
    let output = store(dir.path());
    let execution = output.execution_identity().unwrap();
    let path = output.allocate_background_output(&execution).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "replacement").unwrap();
    assert!(output.open_background_output_sink(&execution).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "replacement");
    output.settle_background_output(&execution);
    assert!(
        output.state.lock().unwrap().background[&execution]
            .file
            .is_none()
    );
    assert!(output.open_background_output_sink(&execution).is_err());
    assert_eq!(output.background_output_path(&execution), path);
}

#[test]
fn nonregular_ownership_metadata_never_blocks_recovery_or_allocation() {
    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;
    let dir = tempfile::tempdir().unwrap();
    let output = store(dir.path());
    let owner = dir.path().join("private");
    let binding = owner.join("output-namespaces.json");
    mkfifo(&binding, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
    assert!(
        ManagedToolOutput::new_in_temp_root(ConversationId::new(CONVERSATION), &owner, dir.path())
            .is_err()
    );
    std::fs::remove_file(binding).unwrap();
    mkfifo(
        &owner.join("output-namespaces.lock"),
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .unwrap();
    assert!(output.open_spill().is_err());
}

#[test]
fn markerless_empty_directory_is_not_cleanup_authority() {
    let dir = tempfile::tempdir().unwrap();
    let output = store(dir.path());
    let namespace = output.root().unwrap();
    std::fs::remove_file(namespace.join(".owner")).unwrap();
    let owner = dir.path().join("private").canonicalize().unwrap();
    assert!(storage::cleanup(dir.path(), &owner, &ConversationId::new(CONVERSATION)).is_err());
    assert!(namespace.is_dir());
    assert!(owner.join("output-namespaces.json").exists());
    std::fs::remove_dir(&namespace).unwrap();
    storage::cleanup(dir.path(), &owner, &ConversationId::new(CONVERSATION)).unwrap();
}

#[test]
fn partial_cleanup_retires_the_current_namespace_without_repairing_it() {
    use crate::tools::output::ForegroundOutputCapture;
    let dir = tempfile::tempdir().unwrap();
    let output = store(dir.path());
    let mut old = output.open_spill().unwrap();
    old.write_all("published output").unwrap();
    let old_path = old.path().to_owned();
    let old_root = old_path.parent().unwrap().to_owned();
    drop(old);
    std::fs::remove_file(old_root.join(".owner")).unwrap();

    let mut capture = ForegroundOutputCapture::with_limit(1);
    capture.push("fresh foreground output", &output).unwrap();
    let foreground = capture.finish(true).output_locator.unwrap();
    assert_ne!(foreground.parent().unwrap(), old_root);
    assert_eq!(
        std::fs::read_to_string(&foreground).unwrap(),
        "fresh foreground output"
    );
    let execution = output.execution_identity().unwrap();
    let background = output.allocate_background_output(&execution).unwrap();
    output
        .open_background_output_sink(&execution)
        .unwrap()
        .append("fresh background output")
        .unwrap();
    assert_eq!(background.parent(), foreground.parent());
    assert_eq!(
        std::fs::read_to_string(background).unwrap(),
        "fresh background output"
    );
    assert_eq!(
        std::fs::read_to_string(&old_path).unwrap(),
        "published output"
    );
    assert!(!old_root.join(".owner").exists());
    assert!(output.validate_locator(&old_path).is_err());
}

#[test]
fn a_temporary_root_inside_the_workspace_reports_configuration_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    for nested in [false, true] {
        let temporary = if nested {
            workspace.join(".tmp")
        } else {
            workspace.clone()
        };
        std::fs::create_dir_all(&temporary).unwrap();
        let output = ManagedToolOutput::new_in_temp_root(
            ConversationId::new(CONVERSATION),
            &dir.path()
                .join(if nested { "private-b" } else { "private-a" }),
            &temporary,
        )
        .unwrap()
        .with_workspace_root(&workspace.canonicalize().unwrap());
        let diagnostic = output.open_spill().unwrap_err().to_string();
        assert!(diagnostic.contains("inside workspace"), "{diagnostic}");
        assert!(!diagnostic.contains("collision"), "{diagnostic}");
        assert!(output.roots().is_empty());
        assert!(
            output
                .allocate_background_output(&output.execution_identity().unwrap())
                .is_err()
        );
    }
}
