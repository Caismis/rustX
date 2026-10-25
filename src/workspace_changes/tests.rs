use super::*;
use std::fs;
mod global_attributes;
mod raw;

fn tempdir() -> tempfile::TempDir {
    // File tools pass resolved mutation paths. Keep fixtures in the same path
    // domain as the canonical workspace, including macOS /var -> /private/var.
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn snapshot_paths(root: &Path, scratch: &Path, tree: &str) -> String {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .env("GIT_OBJECT_DIRECTORY", scratch.join("objects"))
        .env(
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            root.join(".git/objects"),
        )
        .args(["ls-tree", "-r", "--name-only", tree])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn repository() -> tempfile::TempDir {
    let dir = tempdir();
    git(dir.path(), &["init", "-q"]);
    fs::write(dir.path().join("tracked.txt"), "one\ntwo\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "initial",
        ],
    );
    dir
}
fn start(cwd: &Path, store: &Arc<ChangesStore>, id: &str) -> Arc<ChangeRecording> {
    ChangeRecording::start(
        AttemptId::new(id),
        cwd.canonicalize().unwrap(),
        Arc::clone(store),
        CancellationSignal::new(),
    )
    .unwrap()
}

#[tokio::test]
async fn snapshots_exclude_preexisting_edits_and_leave_index_objects_and_refs_untouched() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    fs::write(root.join("tracked.txt"), "already dirty\ntwo\n").unwrap();
    fs::write(root.join("earlier.txt"), "prior untracked file\n").unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let objects = git(root, &["count-objects", "-v"]);
    let head = git(root, &["rev-parse", "HEAD"]);
    let record = start(root, &store, "a");
    record.before_tools().await;
    fs::write(root.join("tracked.txt"), "already dirty\nchanged\n").unwrap();
    fs::write(root.join("new.txt"), "created\n").unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("a")).unwrap();
    assert_eq!(summary.total, 2);
    assert_eq!(
        summary
            .files
            .iter()
            .map(|f| f.display.as_str())
            .collect::<Vec<_>>(),
        vec!["new.txt", "tracked.txt"]
    );
    assert_eq!((summary.added, summary.deleted), (2, 1));
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(git(root, &["count-objects", "-v"]), objects);
    assert_eq!(git(root, &["rev-parse", "HEAD"]), head);
    fs::write(root.join("tracked.txt"), "a later turn\n").unwrap();
    let diff = store
        .comparison(&AttemptId::new("a"), 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        Some(diff.hunks[0].old_text.as_str()),
        Some("already dirty\ntwo\n")
    );
    assert_eq!(
        Some(diff.hunks[0].new_text.as_str()),
        Some("already dirty\nchanged\n")
    );
}

#[tokio::test]
async fn non_git_file_captures_coalesce_edits_and_use_final_content() {
    let dir = tempdir();
    let root = dir.path();
    let store = Arc::default();
    let path = root.join("a.txt");
    fs::write(&path, "original\n").unwrap();
    let record = start(root, &store, "a");
    record.before_tools().await;
    record.capture(&path);
    fs::write(&path, "middle\n").unwrap();
    record.capture(&path);
    fs::write(&path, "final\nnew\n").unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("a")).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!((summary.added, summary.deleted), (2, 1));
    let diff = store
        .comparison(&AttemptId::new("a"), 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Some(diff.hunks[0].old_text.as_str()), Some("original\n"));
    assert_eq!(Some(diff.hunks[0].new_text.as_str()), Some("final\nnew\n"));
}

#[tokio::test]
async fn ignored_files_are_captured_and_net_zero_edits_leave_no_card() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
    let path = root.join("ignored.txt");
    fs::write(&path, "before\n").unwrap();
    let record = start(root, &store, "a");
    record.before_tools().await;
    record.capture(&path);
    fs::write(&path, "after\n").unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("a")).unwrap();
    assert_eq!(summary.files[0].display, "ignored.txt");
    let record = start(root, &store, "b");
    record.before_tools().await;
    record.capture(&path);
    fs::write(&path, "temporary\n").unwrap();
    fs::write(&path, "after\n").unwrap();
    record.finish().await;
    assert!(store.summary(&AttemptId::new("b")).is_none());
}

#[tokio::test]
async fn binary_oversized_deleted_and_renamed_files_keep_honest_comparisons() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    let record = start(root, &store, "a");
    record.before_tools().await;
    fs::rename(root.join("tracked.txt"), root.join("renamed.txt")).unwrap();
    fs::write(root.join("binary.dat"), [0, 1, 2]).unwrap();
    fs::write(
        root.join("large.txt"),
        vec![b'a'; capture::MAX_FILE_BYTES + 1],
    )
    .unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("a")).unwrap();
    assert_eq!(summary.total, 3);
    assert!(
        store
            .comparison(&AttemptId::new("a"), 0)
            .await
            .unwrap()
            .unwrap()
            .binary
    );
    assert!(
        store
            .comparison(&AttemptId::new("a"), 1)
            .await
            .unwrap()
            .unwrap()
            .oversized
    );
    let renamed = store
        .comparison(&AttemptId::new("a"), 2)
        .await
        .unwrap()
        .unwrap();
    assert!(renamed.hunks.is_empty());
    assert!(store.comparison(&AttemptId::new("a"), 3).await.is_err());
    let record = start(root, &store, "b");
    record.before_tools().await;
    fs::remove_file(root.join("renamed.txt")).unwrap();
    record.finish().await;
    let deleted = store
        .comparison(&AttemptId::new("b"), 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Some(deleted.hunks[0].new_text.as_str()), Some(""));
}

#[tokio::test]
async fn no_tools_missing_attempt_and_bounded_file_lists_have_no_fabricated_state() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    let record = start(root, &store, "empty");
    record.finish().await;
    assert!(store.summary(&AttemptId::new("empty")).is_none());
    assert!(
        store
            .comparison(&AttemptId::new("missing"), 0)
            .await
            .unwrap()
            .is_none()
    );
    let record = start(root, &store, "many");
    record.before_tools().await;
    for i in 0..503 {
        fs::write(root.join(format!("file-{i:03}")), "x\n").unwrap();
    }
    record.finish().await;
    let summary = store.summary(&AttemptId::new("many")).unwrap();
    assert_eq!(summary.total, 503);
    assert!(!summary.files.is_empty());
    assert!(summary.files.len() <= MAX_FILES);
    assert!(serde_json::to_vec(&summary).unwrap().len() <= 96 * 1024);
    assert_eq!(summary.added, 503);
    for (index, file) in summary.files.iter().enumerate() {
        assert_eq!(file.display, format!("file-{index:03}"));
    }
    assert!(
        store
            .comparison(&AttemptId::new("many"), summary.files.len())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn comparisons_treat_dynamic_route_and_unicode_file_names_literally() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    fs::create_dir(root.join("[id]")).unwrap();
    let path = root.join("[id]/页面.tsx");
    fs::write(&path, "before\n").unwrap();
    let record = start(root, &store, "route");
    record.before_tools().await;
    fs::write(&path, "after\n").unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("route")).unwrap();
    assert_eq!(summary.files[0].display, "[id]/页面.tsx");
    let diff = store
        .comparison(&AttemptId::new("route"), 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(diff.hunks[0].old_text, "before\n");
    assert_eq!(diff.hunks[0].new_text, "after\n");
}

#[tokio::test]
async fn a_failed_repository_baseline_never_becomes_a_capture_only_record() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    fs::write(root.join(".git/index"), "invalid index").unwrap();
    let record = start(root, &store, "failed");
    record.before_tools().await;
    let path = root.join("tracked.txt");
    record.capture(&path);
    fs::write(&path, "after\n").unwrap();
    record.finish().await;
    assert!(store.summary(&AttemptId::new("failed")).is_none());
}

#[tokio::test]
async fn review_git_endpoints_win_over_intermediate_file_tool_captures() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    let path = root.join("tracked.txt");
    let record = start(root, &store, "restored");
    record.before_tools().await;
    fs::write(&path, "shell intermediate\n").unwrap();
    record.capture(&path);
    fs::write(&path, "one\ntwo\n").unwrap();
    record.finish().await;
    assert!(store.summary(&AttemptId::new("restored")).is_none());

    let shared = "shared\n".repeat(10);
    fs::write(&path, format!("{shared}old\n")).unwrap();
    let record = start(root, &store, "renamed");
    record.before_tools().await;
    record.capture(&path);
    fs::write(&path, format!("{shared}changed\n")).unwrap();
    fs::rename(&path, root.join("renamed.txt")).unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("renamed")).unwrap();
    assert_eq!(summary.total, 1, "a rename must not add a second deletion");
    assert_eq!(summary.files[0].display, "renamed.txt");
}

#[tokio::test]
async fn review_empty_file_and_newline_changes_survive_zero_line_counts() {
    for git_workspace in [false, true] {
        let dir = if git_workspace {
            repository()
        } else {
            tempdir()
        };
        let root = dir.path();
        if git_workspace {
            fs::write(root.join(".gitignore"), "captured.txt\n").unwrap();
        }
        let store = Arc::default();
        let path = root.join("captured.txt");
        for (id, before, after) in [
            ("create", None, Some("")),
            ("delete", Some(""), None),
            ("newline", Some("text"), Some("text\n")),
        ] {
            if let Some(before) = before {
                fs::write(&path, before).unwrap();
            }
            let record = start(root, &store, id);
            record.before_tools().await;
            record.capture(&path);
            match after {
                Some(after) => fs::write(&path, after).unwrap(),
                None => fs::remove_file(&path).unwrap(),
            }
            record.finish().await;
            let summary = store.summary(&AttemptId::new(id)).unwrap();
            assert_eq!(summary.total, 1, "{id}, Git={git_workspace}");
            assert_eq!((summary.added, summary.deleted), (0, 0));
        }
    }
}

#[tokio::test]
async fn review_nested_repository_contents_do_not_leak_into_parent_changes() {
    let dir = repository();
    let nested = dir.path().join("nested");
    fs::create_dir(&nested).unwrap();
    git(&nested, &["init", "-q"]);
    let path = nested.join("nested.txt");
    fs::write(&path, "before\n").unwrap();
    git(&nested, &["add", "."]);
    git(
        &nested,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "initial nested",
        ],
    );
    let store = Arc::default();
    for staged in [false, true] {
        if staged {
            git(dir.path(), &["add", "nested"]);
        }
        fs::write(&path, "before\n").unwrap();
        let id = format!("nested-{staged}");
        let record = start(dir.path(), &store, &id);
        record.before_tools().await;
        record.capture(&path);
        fs::write(&path, "after\n").unwrap();
        fs::write(dir.path().join("tracked.txt"), format!("outer-{staged}\n")).unwrap();
        record.finish().await;
        let summary = store.summary(&AttemptId::new(id)).unwrap();
        assert_eq!(summary.total, 1);
        assert_eq!(summary.files[0].display, "tracked.txt");
    }
}

#[tokio::test]
async fn review_each_snapshot_observes_the_current_real_index_without_mutating_it() {
    let dir = repository();
    let root = dir.path();
    fs::write(root.join(".gitignore"), "*.generated\n").unwrap();
    let store = Arc::default();
    let record = start(root, &store, "force-added");
    record.before_tools().await;
    fs::write(root.join("new.generated"), "created\n").unwrap();
    git(root, &["add", "-f", "new.generated"]);
    let index = fs::read(root.join(".git/index")).unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("force-added")).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].display, "new.generated");
    assert_eq!((summary.added, summary.deleted), (1, 0));
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
}

#[tokio::test]
async fn ignore_and_untrack_changes_do_not_report_unchanged_files_as_deleted() {
    for tracked in [false, true] {
        let dir = repository();
        let root = dir.path();
        fs::write(root.join("generated.log"), "unchanged\n").unwrap();
        if tracked {
            git(root, &["add", "generated.log"]);
        }
        let store = Arc::default();
        let record = start(root, &store, "ignored");
        record.before_tools().await;
        fs::write(root.join(".gitignore"), "generated.log\n").unwrap();
        if tracked {
            git(root, &["rm", "--cached", "generated.log"]);
        }
        let index = fs::read(root.join(".git/index")).unwrap();
        record.finish().await;
        let summary = store.summary(&AttemptId::new("ignored")).unwrap();
        assert_eq!(summary.total, 1);
        assert_eq!(summary.files[0].display, ".gitignore");
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    }
}

#[tokio::test]
async fn index_performance_flags_do_not_hide_physical_edits() {
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        let dir = repository();
        let root = dir.path();
        git(root, &["update-index", flag, "tracked.txt"]);
        let index = fs::read(root.join(".git/index")).unwrap();
        let store = Arc::default();
        let record = start(root, &store, "flagged");
        record.before_tools().await;
        let path = root.join("tracked.txt");
        record.capture(&path);
        fs::write(&path, "changed\n").unwrap();
        record.finish().await;
        let summary = store.summary(&AttemptId::new("flagged")).unwrap();
        assert_eq!(summary.total, 1);
        assert_eq!(summary.files[0].display, "tracked.txt");
        assert_eq!((summary.added, summary.deleted), (1, 2));
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    }
}

#[tokio::test]
async fn newly_ignored_restored_file_keeps_its_attempt_baseline() {
    let dir = repository();
    let root = dir.path();
    let path = root.join("generated.log");
    fs::write(&path, "original\n").unwrap();
    let store = Arc::default();
    let record = start(root, &store, "restored");
    record.before_tools().await;
    fs::write(&path, "middle\n").unwrap();
    record.capture(&path);
    fs::write(&path, "original\n").unwrap();
    fs::write(root.join(".gitignore"), "generated.log\n").unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("restored")).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].display, ".gitignore");
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
}

#[tokio::test]
async fn ignored_rename_source_is_not_reported_again_as_a_captured_deletion() {
    let dir = repository();
    let root = dir.path();
    let original = "shared line\n".repeat(10) + "before\n";
    let path = root.join("old.txt");
    fs::write(&path, original).unwrap();
    git(root, &["add", "old.txt"]);
    let store = Arc::default();
    let record = start(root, &store, "renamed");
    record.before_tools().await;
    record.capture(&path);
    fs::write(&path, "shared line\n".repeat(10) + "after\n").unwrap();
    git(root, &["mv", "old.txt", "new.txt"]);
    fs::write(root.join(".gitignore"), "old.txt\n").unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("renamed")).unwrap();
    assert_eq!(summary.total, 2);
    assert_eq!(
        summary
            .files
            .iter()
            .map(|file| file.display.as_str())
            .collect::<Vec<_>>(),
        [".gitignore", "new.txt"]
    );
    assert_eq!((summary.added, summary.deleted), (2, 1));
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
}

#[tokio::test]
async fn absent_sparse_entries_do_not_become_physical_deletions() {
    let dir = repository();
    let root = dir.path();
    git(root, &["update-index", "--skip-worktree", "tracked.txt"]);
    fs::remove_file(root.join("tracked.txt")).unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let store = Arc::default();
    let record = start(root, &store, "sparse");
    record.before_tools().await;
    fs::write(root.join("new.txt"), "created\n").unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("sparse")).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].display, "new.txt");
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
}

#[tokio::test]
async fn sparse_checkout_samples_existing_excluded_files_and_preserves_absent_entries() {
    let dir = repository();
    let root = dir.path();
    fs::create_dir(root.join("excluded")).unwrap();
    for name in ["edited.txt", "absent.txt"] {
        fs::write(root.join("excluded").join(name), "before\n").unwrap();
    }
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "sparse fixture",
        ],
    );
    git(root, &["sparse-checkout", "set", "included"]);
    assert!(!root.join("excluded/absent.txt").exists());
    fs::create_dir_all(root.join("excluded")).unwrap();
    let path = root.join("excluded/edited.txt");
    fs::write(&path, "before\n").unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let store = Arc::default();
    let record = start(root, &store, "sparse-checkout");
    record.before_tools().await;
    record.capture(&path);
    fs::write(&path, "after\n").unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("sparse-checkout")).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].display, "excluded/edited.txt");
    assert_eq!((summary.added, summary.deleted), (1, 1));
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
}

#[tokio::test]
async fn review_non_regular_capture_does_not_discard_other_file_changes() {
    let dir = tempdir();
    let store = Arc::default();
    let path = dir.path().join("becomes-directory");
    let kept = dir.path().join("kept.txt");
    fs::write(&path, "before\n").unwrap();
    fs::write(&kept, "before\n").unwrap();
    let record = start(dir.path(), &store, "directory");
    record.before_tools().await;
    record.capture(&path);
    record.capture(&kept);
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    fs::write(&kept, "after\n").unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("directory")).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].display, "kept.txt");
}

async fn blocked_recording(
    cwd: &Path,
) -> (Arc<ChangeRecording>, tokio::sync::oneshot::Receiver<()>) {
    let record = start(cwd, &Arc::default(), "blocked");
    let (started, waiting) = tokio::sync::oneshot::channel();
    let (alive, dropped) = tokio::sync::oneshot::channel();
    let task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        let _ = started.send(());
        let result = std::future::pending::<Result<Option<Baseline>, String>>().await;
        drop(alive);
        result
    }));
    *record.task.lock().await = Some(task);
    waiting.await.unwrap();
    (record, dropped)
}

#[tokio::test]
async fn review_unused_cancelled_and_dropped_recordings_abort_their_baseline() {
    let dir = tempdir();
    for mode in ["unused", "cancelled", "dropped"] {
        let (record, dropped) = blocked_recording(dir.path()).await;
        if mode == "cancelled" {
            // Poll the real pre-tool wait, then drop it as the Agent's
            // cancellation select does before any executor can start.
            let wait = record.before_tools();
            tokio::pin!(wait);
            tokio::select! {
                biased;
                () = &mut wait => panic!("baseline is deliberately blocked"),
                () = tokio::task::yield_now() => {}
            }
        }
        if mode != "dropped" {
            tokio::time::timeout(std::time::Duration::from_secs(1), record.finish())
                .await
                .expect("an unused baseline must not delay the terminal");
        }
        drop(record);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), dropped)
                .await
                .expect("baseline task must be aborted")
                .is_err()
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn review_fifo_endpoints_never_block_other_file_changes() {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = tempdir();
    let store = Arc::default();
    let path = dir.path().join("becomes-fifo");
    let kept = dir.path().join("kept.txt");
    fs::write(&path, "before\n").unwrap();
    fs::write(&kept, "before\n").unwrap();
    let record = start(dir.path(), &store, "fifo");
    record.before_tools().await;
    record.capture(&path);
    record.capture(&kept);
    fs::remove_file(&path).unwrap();
    nix::unistd::mkfifo(
        &path,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    fs::write(&kept, "after\n").unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), record.finish()).await;
    if result.is_err() {
        // Release an incorrectly blocking read too, so a failing regression
        // cannot leave the test runtime waiting on its blocking worker.
        let _ = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path);
    }
    result.expect("a non-regular endpoint must not block completion");
    let summary = store.summary(&AttemptId::new("fifo")).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].display, "kept.txt");
}

#[tokio::test]
async fn review_supervised_git_treats_quoted_newline_paths_as_data() {
    let dir = repository();
    let root = dir.path();
    fs::write(root.join(".gitignore"), "*.txt\n").unwrap();
    let name = "captured'\n$(touch injected).txt";
    let path = root.join(name);
    fs::write(&path, "before\n").unwrap();
    let store = Arc::default();
    let record = start(root, &store, "quoted");
    record.before_tools().await;
    record.capture(&path);
    fs::write(&path, "after\n").unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("quoted")).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].display, name);
    assert!(!root.join("injected").exists());
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_cancelling_a_snapshot_reaps_its_blocked_git_reader() {
    use std::os::unix::fs::OpenOptionsExt;
    use std::time::Duration;

    let dir = repository();
    let root = dir.path();
    let fifo = root.join(".git/held-config");
    nix::unistd::mkfifo(
        &fifo,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    git(root, &["config", "include.path", fifo.to_str().unwrap()]);
    let record = start(root, &Arc::default(), "cancel-git-reader");
    let scratch = record.directory.path().to_owned();
    let _writer = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
            {
                Ok(writer) => break writer,
                Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {}
                Err(error) => panic!("open held Git configuration: {error}"),
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Git must start reading its configuration");
    // Keep the writer open without data or EOF while identifying the actual
    // Git child. This blocks data reading, without invoking hooks or filters.
    let pid = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            for entry in fs::read_dir("/proc").unwrap().flatten() {
                let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                    continue;
                };
                if fs::read_link(entry.path().join("exe"))
                    .ok()
                    .and_then(|path| path.file_name().map(|name| name == "git"))
                    != Some(true)
                {
                    continue;
                }
                if fs::read_dir(entry.path().join("fd")).is_ok_and(|fds| {
                    fds.flatten()
                        .any(|fd| fs::read_link(fd.path()).ok().as_ref() == Some(&fifo))
                }) {
                    return pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the owned Git reader must hold the configuration FIFO");
    assert!(
        scratch.is_dir(),
        "in-flight snapshot storage must remain owned"
    );
    tokio::time::timeout(Duration::from_secs(10), record.finish())
        .await
        .expect("an unused recording must not wait for its blocked Git read");
    drop(record);
    tokio::time::timeout(Duration::from_secs(10), async {
        while Path::new(&format!("/proc/{pid}")).exists() || scratch.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Git must be physically reaped and its snapshot storage reclaimed");
}

async fn ignored_existing_file_entering_git_preserves_before(force_add: bool) {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    let path = root.join("config.txt");
    fs::write(root.join(".gitignore"), "config.txt\n").unwrap();
    fs::write(&path, "unchanged\nold\n").unwrap();
    let record = start(root, &store, "visible");
    record.before_tools().await;
    record.capture(&path);
    fs::write(&path, "unchanged\nnew\n").unwrap();
    if force_add {
        git(root, &["add", "-f", "config.txt"]);
    } else {
        fs::write(root.join(".gitignore"), "").unwrap();
    }
    let index = fs::read(root.join(".git/index")).unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("visible")).unwrap();
    let position = summary
        .files
        .iter()
        .position(|file| file.display == "config.txt")
        .unwrap();
    let file = &summary.files[position];
    assert_eq!((file.added, file.deleted), (1, 1));
    let comparison = store
        .comparison(&AttemptId::new("visible"), position)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(comparison.hunks[0].old_text, "unchanged\nold\n");
    assert_eq!(comparison.hunks[0].new_text, "unchanged\nnew\n");
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
}

#[tokio::test]
async fn ignored_existing_file_unignored_after_edit_keeps_its_original_contents() {
    ignored_existing_file_entering_git_preserves_before(false).await;
}

#[tokio::test]
async fn ignored_existing_file_force_added_after_edit_keeps_its_original_contents() {
    ignored_existing_file_entering_git_preserves_before(true).await;
}

#[tokio::test]
async fn restored_ignored_file_entering_git_has_no_fabricated_addition() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    let path = root.join("config.txt");
    fs::write(root.join(".gitignore"), "config.txt\n").unwrap();
    fs::write(&path, "unchanged\n").unwrap();
    let record = start(root, &store, "restored");
    record.before_tools().await;
    record.capture(&path);
    fs::write(&path, "intermediate\n").unwrap();
    fs::write(&path, "unchanged\n").unwrap();
    git(root, &["add", "-f", "config.txt"]);
    record.finish().await;
    assert!(store.summary(&AttemptId::new("restored")).is_none());
}

#[tokio::test]
async fn existing_ignored_destination_keeps_capture_when_git_infers_a_rename() {
    for oversized in [false, true] {
        let dir = repository();
        let root = dir.path();
        let store = Arc::default();
        let source = root.join("source.txt");
        let destination = root.join("config.txt");
        fs::write(root.join(".gitignore"), "config.txt\n").unwrap();
        fs::write(&source, "unchanged\nnew\n").unwrap();
        git(root, &["add", "source.txt", ".gitignore"]);
        fs::write(
            &destination,
            if oversized {
                vec![b'a'; capture::MAX_FILE_BYTES + 1]
            } else {
                b"unchanged\nold\n".to_vec()
            },
        )
        .unwrap();
        let record = start(root, &store, "false-rename");
        record.before_tools().await;
        record.capture(&destination);
        fs::write(&destination, "unchanged\nnew\n").unwrap();
        fs::remove_file(&source).unwrap();
        fs::write(root.join(".gitignore"), "").unwrap();
        let index = fs::read(root.join(".git/index")).unwrap();
        record.finish().await;
        let summary = store.summary(&AttemptId::new("false-rename")).unwrap();
        assert_eq!(
            summary.total, 3,
            "ignore rule, existing destination and deleted source are distinct changes"
        );
        let destination_index = summary
            .files
            .iter()
            .position(|file| file.display == "config.txt")
            .unwrap();
        let file = &summary.files[destination_index];
        let comparison = store
            .comparison(&AttemptId::new("false-rename"), destination_index)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(file.oversized, oversized);
        assert_eq!(comparison.oversized, oversized);
        if !oversized {
            assert_eq!((file.added, file.deleted), (1, 1));
            assert_eq!(comparison.hunks[0].old_text, "unchanged\nold\n");
            assert_eq!(comparison.hunks[0].new_text, "unchanged\nnew\n");
        }
        let source_index = summary
            .files
            .iter()
            .position(|file| file.display == "source.txt")
            .unwrap();
        assert_eq!(
            (
                summary.files[source_index].added,
                summary.files[source_index].deleted
            ),
            (0, 2)
        );
        let comparison = store
            .comparison(&AttemptId::new("false-rename"), source_index)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(comparison.hunks[0].old_text, "unchanged\nnew\n");
        assert_eq!(comparison.hunks[0].new_text, "");
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    }
}

#[tokio::test]
async fn missing_destination_capture_does_not_split_a_real_git_rename() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    let destination = root.join("renamed.txt");
    let record = start(root, &store, "real-rename");
    record.before_tools().await;
    record.capture(&destination);
    fs::rename(root.join("tracked.txt"), &destination).unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("real-rename")).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].display, "renamed.txt");
    assert_eq!((summary.added, summary.deleted), (0, 0));
    let comparison = store
        .comparison(&AttemptId::new("real-rename"), 0)
        .await
        .unwrap()
        .unwrap();
    assert!(comparison.hunks.is_empty());
}

#[tokio::test]
async fn newly_created_nested_repository_captures_stay_with_the_nested_owner() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    let record = start(root, &store, "new-nested");
    record.before_tools().await;
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    git(&nested, &["init", "-q"]);
    let child = nested.join("child.txt");
    fs::write(&child, "before\n").unwrap();
    git(&nested, &["add", "child.txt"]);
    git(
        &nested,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "nested initial",
        ],
    );
    record.capture(&child);
    fs::write(&child, "after\n").unwrap();
    fs::write(root.join("tracked.txt"), "outer change\n").unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let nested_index = fs::read(nested.join(".git/index")).unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("new-nested")).unwrap();
    assert_eq!(
        summary
            .files
            .iter()
            .map(|file| file.display.as_str())
            .collect::<Vec<_>>(),
        ["nested", "tracked.txt"]
    );
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(nested.join(".git/index")).unwrap(), nested_index);
}

#[cfg(unix)]
#[test]
fn temporary_root_aliases_match_resolved_mutation_paths() {
    let dir = tempdir();
    let physical = dir.path().join("physical");
    let alias = dir.path().join("alias");
    fs::create_dir(&physical).unwrap();
    std::os::unix::fs::symlink(&physical, &alias).unwrap();
    // A new file need not exist: Write resolves its deepest existing parent.
    let resolved = physical.join("not-created-yet.txt");
    assert!(is_temporary_path(&resolved, &physical));
    assert!(is_temporary_path(&resolved, &alias));
    assert!(!is_temporary_path(&dir.path().join("outside.txt"), &alias));
}

#[tokio::test]
async fn external_temporary_edits_are_excluded_while_workspace_files_are_kept() {
    let dir = tempdir();
    let workspace = dir.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let local = workspace.join("kept.txt");
    let external = dir.path().join("temporary.txt");
    for path in [&local, &external] {
        fs::write(path, "before\n").unwrap();
    }
    let store = Arc::default();
    let record = start(&workspace, &store, "temporary-output");
    record.before_tools().await;
    for path in [&local, &external] {
        record.capture(path);
        fs::write(path, "after\n").unwrap();
    }
    record.finish().await;
    let summary = store.summary(&AttemptId::new("temporary-output")).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].display, "kept.txt");
    assert_eq!((summary.added, summary.deleted), (1, 1));
}

#[tokio::test]
async fn long_paths_bound_summary_bytes_without_losing_total_change_counts() {
    let dir = tempdir();
    let store = Arc::default();
    let record = start(dir.path(), &store, "long-paths");
    record.before_tools().await;
    for index in 0..256 {
        let path = dir
            .path()
            .join(format!("{index:03}-{}.txt", "x".repeat(180)));
        record.capture(&path);
        fs::write(path, "created\n").unwrap();
    }
    record.finish().await;
    let summary = store.summary(&AttemptId::new("long-paths")).unwrap();
    assert_eq!(summary.total, 256);
    assert_eq!((summary.added, summary.deleted), (256, 0));
    assert!(!summary.files.is_empty());
    assert!(
        summary.files.len() < summary.total,
        "the byte budget truncates before the 500-file limit"
    );
    assert!(serde_json::to_vec(&summary).unwrap().len() <= 96 * 1024);
    let comparison = store
        .comparison(&AttemptId::new("long-paths"), 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(comparison.hunks[0].new_text, "created\n");
    assert!(
        store
            .comparison(&AttemptId::new("long-paths"), summary.files.len())
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn aliased_temporary_root_inside_repository_never_snapshots_private_storage() {
    let dir = repository();
    let root = dir.path();
    let physical = root.join("snapshots");
    let alias = root.join("scratch-alias");
    fs::create_dir(&physical).unwrap();
    std::os::unix::fs::symlink(&physical, &alias).unwrap();
    // The production constructor uses this exact helper with env::temp_dir().
    // Supplying an alias here reproduces macOS without mutating global env.
    let scratch = Arc::new(create_snapshot_directory(&alias).unwrap());
    assert!(scratch.path().starts_with(&physical));
    assert_eq!(scratch.path(), scratch.path().canonicalize().unwrap());
    fs::write(
        scratch.path().join("private-marker"),
        "never a user change\n",
    )
    .unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let objects = git(root, &["count-objects", "-v"]);
    let repo = Repository::locate(root, Arc::clone(&scratch), CancellationSignal::new())
        .await
        .unwrap()
        .unwrap();
    let before = repo.snapshot().await.unwrap();
    fs::write(root.join("tracked.txt"), "real user change\n").unwrap();
    let after = repo.snapshot().await.unwrap();
    for tree in [&before, &after] {
        assert_eq!(
            snapshot_paths(root, scratch.path(), tree),
            "scratch-alias\ntracked.txt\n"
        );
    }
    let changes = repo.changes(&before, &after, true).await.unwrap();
    assert_eq!(
        changes.keys().map(String::as_str).collect::<Vec<_>>(),
        ["tracked.txt"]
    );
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(git(root, &["count-objects", "-v"]), objects);
}

#[tokio::test]
async fn interleaved_recorders_exclude_peer_storage_and_keep_temporary_parent_user_edits() {
    let dir = repository();
    let root = dir.path();
    let temporary_root = root.join("snapshots");
    let ordinary = temporary_root.join("rustx-changes-user/note.txt");
    let ignored = temporary_root.join("ignored.txt");
    fs::create_dir_all(ordinary.parent().unwrap()).unwrap();
    for path in [&ordinary, &ignored] {
        fs::write(path, "unchanged\nbefore\n").unwrap();
    }
    fs::write(root.join(".gitignore"), "snapshots/ignored.txt\n").unwrap();
    git(root, &["add", "-f", "snapshots/ignored.txt"]);
    let initial_index = fs::read(root.join(".git/index")).unwrap();
    let expected_paths =
        ".gitignore\nsnapshots/ignored.txt\nsnapshots/rustx-changes-user/note.txt\ntracked.txt\n";

    let scratch_a = Arc::new(create_snapshot_directory(&temporary_root).unwrap());
    fs::write(scratch_a.path().join("private-a"), "private A\n").unwrap();
    let a = Repository::locate(root, Arc::clone(&scratch_a), CancellationSignal::new())
        .await
        .unwrap()
        .unwrap();
    let before = a.snapshot().await.unwrap();
    assert_eq!(
        snapshot_paths(root, scratch_a.path(), &before),
        expected_paths
    );

    // A's baseline predates B's storage. B must not capture A, and A's final
    // snapshot must not report B's index, objects or captures as user edits.
    let scratch_b = Arc::new(create_snapshot_directory(&temporary_root).unwrap());
    let private_b = scratch_b.path().join("private-b");
    fs::write(&private_b, "private B\n").unwrap();
    let b = Repository::locate(root, Arc::clone(&scratch_b), CancellationSignal::new())
        .await
        .unwrap()
        .unwrap();
    let b_before = b.snapshot().await.unwrap();
    assert_eq!(
        snapshot_paths(root, scratch_b.path(), &b_before),
        expected_paths
    );
    assert_eq!(b_before, before);
    let after = a.snapshot().await.unwrap();
    assert_eq!(after, before);
    assert!(a.changes(&before, &after, true).await.unwrap().is_empty());
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), initial_index);

    // The force-add path must enforce the same exact storage exclusion. A
    // third recorder also must prune private entries copied from the real index.
    let private_relative = private_b.strip_prefix(root).unwrap().to_str().unwrap();
    fs::write(
        root.join(".git/info/exclude"),
        format!("/{private_relative}\n"),
    )
    .unwrap();
    git(root, &["add", "-f", "--", private_relative]);
    let index = fs::read(root.join(".git/index")).unwrap();
    let objects = git(root, &["count-objects", "-v"]);
    assert_eq!(a.snapshot().await.unwrap(), before);
    let scratch_c = Arc::new(create_snapshot_directory(&temporary_root).unwrap());
    let c = Repository::locate(root, Arc::clone(&scratch_c), CancellationSignal::new())
        .await
        .unwrap()
        .unwrap();
    let c_before = c.snapshot().await.unwrap();
    assert_eq!(
        snapshot_paths(root, scratch_c.path(), &c_before),
        expected_paths
    );
    assert_eq!(c_before, before);

    // A normal sibling with a rustx-changes-* name and a force-tracked ignored
    // sibling remain user files; excluding TMPDIR or a name glob would lose them.
    for path in [&ordinary, &ignored] {
        fs::write(path, "unchanged\nafter\n").unwrap();
    }
    for recorder in [&a, &b, &c] {
        let after = recorder.snapshot().await.unwrap();
        let changes = recorder.changes(&before, &after, true).await.unwrap();
        assert_eq!(
            changes.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "snapshots/ignored.txt",
                "snapshots/rustx-changes-user/note.txt"
            ]
        );
        for change in changes.values() {
            assert_eq!((change.added, change.deleted), (1, 1));
        }
    }
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(git(root, &["count-objects", "-v"]), objects);
}

#[cfg(unix)]
#[test]
fn snapshot_storage_rejects_unowned_or_unsafe_namespaces_without_changing_them() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    for case in [
        "missing owner",
        "invalid owner",
        "shared root",
        "shared owner",
        "symlink root",
        "symlink owner",
    ] {
        let temporary_root = tempdir();
        let seed = create_snapshot_directory(temporary_root.path()).unwrap();
        let root = seed.root().to_owned();
        let marker = root.join(".owner");
        drop(seed);
        fs::write(root.join("user-file"), "keep this content\n").unwrap();
        match case {
            "missing owner" => fs::remove_file(&marker).unwrap(),
            "invalid owner" => fs::write(&marker, "user-owned namespace\n").unwrap(),
            "shared root" => fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap(),
            "shared owner" => {
                fs::set_permissions(&marker, fs::Permissions::from_mode(0o644)).unwrap();
            }
            "symlink root" => {
                let destination = temporary_root.path().join("user-owned-directory");
                fs::rename(&root, &destination).unwrap();
                symlink(destination, &root).unwrap();
            }
            "symlink owner" => {
                let destination = root.join("original-owner");
                fs::rename(&marker, &destination).unwrap();
                symlink(destination, &marker).unwrap();
            }
            _ => unreachable!(),
        }
        let identity = |path: &Path| {
            fs::symlink_metadata(path)
                .ok()
                .map(|metadata| (metadata.ino(), metadata.mode()))
        };
        let entries = || {
            let mut names = fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>();
            names.sort();
            names
        };
        let original_root = identity(&root);
        let original_marker = identity(&marker);
        let original_owner = fs::read(&marker).ok();
        let original_entries = entries();

        assert!(
            create_snapshot_directory(temporary_root.path()).is_err(),
            "accepted {case}"
        );
        assert_eq!(identity(&root), original_root, "changed root for {case}");
        assert_eq!(
            identity(&marker),
            original_marker,
            "changed marker for {case}"
        );
        assert_eq!(
            fs::read(&marker).ok(),
            original_owner,
            "changed owner for {case}"
        );
        assert_eq!(entries(), original_entries, "added files for {case}");
        assert_eq!(
            fs::read(root.join("user-file")).unwrap(),
            b"keep this content\n"
        );
    }
}

#[test]
fn snapshot_storage_shares_only_the_namespace_and_reclaims_attempt_directories() {
    let temporary_root = tempdir();
    let first = create_snapshot_directory(temporary_root.path()).unwrap();
    let second = create_snapshot_directory(temporary_root.path()).unwrap();
    let root = first.root().to_owned();
    let first_path = first.path().to_owned();
    let second_path = second.path().to_owned();
    let owner = fs::read(root.join(".owner")).unwrap();
    assert_eq!(second.root(), root);
    assert_ne!(first_path, second_path);
    assert!(first.contains(&second_path));
    assert!(second.contains(&first_path));
    fs::write(first_path.join("capture"), "first\n").unwrap();
    fs::write(second_path.join("capture"), "second\n").unwrap();

    drop(first);
    assert!(!first_path.exists());
    assert_eq!(fs::read(second_path.join("capture")).unwrap(), b"second\n");
    assert_eq!(fs::read(root.join(".owner")).unwrap(), owner);
    drop(second);
    assert!(!second_path.exists());
    assert_eq!(
        fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>(),
        [std::ffi::OsString::from(".owner")]
    );
    let next = create_snapshot_directory(temporary_root.path()).unwrap();
    assert_eq!(
        next.root(),
        root,
        "later attempts reuse the same exact namespace"
    );
}
