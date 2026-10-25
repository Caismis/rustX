use super::*;
use std::fs;

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
fn repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    assert_eq!(summary.files.len(), MAX_FILES);
    assert_eq!(summary.added, 503);
    assert!(
        store
            .comparison(&AttemptId::new("many"), 500)
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
            tempfile::tempdir().unwrap()
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_cancelling_a_snapshot_reaps_git_filter_descendants() {
    use nix::{errno::Errno, sys::signal::kill, unistd::Pid};
    let dir = repository();
    let root = dir.path();
    let marker = root.join("filter.pid");
    let filter = format!("echo $$ > '{}'; exec sleep 60", marker.display());
    git(root, &["config", "filter.review.clean", &filter]);
    fs::write(root.join(".gitattributes"), "tracked.txt filter=review\n").unwrap();
    fs::write(root.join("tracked.txt"), "trigger the filter\n").unwrap();
    let record = start(root, &Arc::default(), "cancel-filter");
    let pid = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if let Ok(text) = tokio::fs::read_to_string(&marker).await
                && let Ok(pid) = text.trim().parse::<i32>()
            {
                break pid;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the owned Git filter must start");
    record.finish().await;
    drop(record);
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if kill(Pid::from_raw(pid), None) == Err(Errno::ESRCH) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the supervised Git filter must be gone and reaped");
}
