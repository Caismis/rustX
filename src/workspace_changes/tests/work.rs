//! Session reuse, bounded reads, and immutable user repository regressions.
use super::*;

#[tokio::test]
async fn text_and_read_turns_reuse_one_directory_and_index_without_new_objects() {
    let dir = repository();
    let metadata = raw::git_files(dir.path());
    let store = Arc::default();
    let first = start(dir.path(), &store, "initial");
    first.finish().await;
    let directory = first.directory.get().unwrap().path().to_owned();
    let objects = object_files(&directory.join("git/objects"));
    for tool in [false, true] {
        for turn in 0..8 {
            let record = start(dir.path(), &store, &format!("work-{tool}-{turn}"));
            record.ready().await;
            assert_eq!(record.directory.get().unwrap().path(), directory);
            if tool {
                record.before_tools().await;
            }
            record.finish().await;
            assert_eq!(object_files(&directory.join("git/objects")), objects);
        }
    }
    assert!(store.records.lock().unwrap().is_empty());
    assert_eq!(raw::git_files(dir.path()), metadata);
    drop(first);
    drop(store);
    assert!(
        !directory.exists(),
        "runtime disposal removes the session directory"
    );
}

fn object_files(root: &Path) -> usize {
    fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| {
            if entry.path().is_dir() {
                object_files(&entry.path())
            } else {
                1
            }
        })
        .sum()
}

#[tokio::test]
async fn changed_turn_keeps_the_inference_baseline_and_reads_comparisons_on_demand() {
    let dir = repository();
    let store = Arc::default();
    let record = start(dir.path(), &store, "external-during-inference");
    record.ready().await;
    let commands = Arc::clone(
        &record
            .baseline
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .repository
            .commands,
    );
    fs::write(
        dir.path().join("tracked.txt"),
        "external during inference\n",
    )
    .unwrap();
    record.before_tools().await;
    record.finish().await;
    assert_eq!(
        commands.load(Ordering::Relaxed),
        3,
        "two snapshot batches and one diff"
    );
    raw::text_change(
        &store,
        "external-during-inference",
        "tracked.txt",
        "one\ntwo\n",
        "external during inference\n",
        (1, 2),
    )
    .await;
    assert_eq!(
        commands.load(Ordering::Relaxed),
        7,
        "size and blob reads for two sides"
    );
}

#[tokio::test]
async fn comparisons_report_missing_alternate_objects_after_repository_removal() {
    let dir = repository();
    let store = Arc::default();
    let record = start(dir.path(), &store, "retained");
    record.before_tools().await;
    fs::write(dir.path().join("tracked.txt"), "after\n").unwrap();
    record.finish().await;
    fs::remove_dir_all(dir.path().join(".git")).unwrap();
    assert!(
        store
            .comparison(&AttemptId::new("retained"), 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn failed_private_repository_setup_never_writes_the_real_object_store() {
    let dir = repository();
    let metadata = raw::git_files(dir.path());
    let scratch = Arc::new(create_snapshot_directory(&std::env::temp_dir()).unwrap());
    fs::create_dir(scratch.path().join("git")).unwrap();
    assert!(
        Repository::locate(
            dir.path(),
            Arc::new(OnceLock::from(scratch)),
            CancellationSignal::new()
        )
        .await
        .is_err()
    );
    assert_eq!(raw::git_files(dir.path()), metadata);
}

#[tokio::test]
async fn capture_only_cards_keep_session_storage_and_their_comparisons() {
    let dir = repository();
    fs::write(dir.path().join(".gitignore"), "*.ignored\n").unwrap();
    fs::write(dir.path().join("config.ignored"), "before\n").unwrap();
    let store = Arc::default();
    let record = start(dir.path(), &store, "capture-only");
    record.before_tools().await;
    record.capture(&dir.path().join("config.ignored"));
    fs::write(dir.path().join("config.ignored"), "after\n").unwrap();
    record.finish().await;
    raw::text_change(
        &store,
        "capture-only",
        "config.ignored",
        "before\n",
        "after\n",
        (1, 1),
    )
    .await;
}

#[tokio::test]
async fn comparisons_reject_corrupted_private_trees() {
    let dir = repository();
    let store = Arc::default();
    let record = start(dir.path(), &store, "corrupt-tree");
    record.before_tools().await;
    fs::write(dir.path().join("tracked.txt"), "after\n").unwrap();
    record.finish().await;
    let completed = store
        .records
        .lock()
        .unwrap()
        .get("corrupt-tree")
        .unwrap()
        .clone();
    let after = &completed.trees.as_ref().unwrap().1;
    let object = record
        .directory
        .get()
        .unwrap()
        .path()
        .join("git/objects")
        .join(&after[..2])
        .join(&after[2..]);
    fs::remove_file(&object).unwrap();
    fs::write(object, "corrupt compressed tree").unwrap();
    assert!(
        store
            .comparison(&AttemptId::new("corrupt-tree"), 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn missing_private_blobs_are_errors_instead_of_oversized_files() {
    let dir = repository();
    let store = Arc::default();
    let record = start(dir.path(), &store, "missing-blob");
    record.before_tools().await;
    fs::write(dir.path().join("tracked.txt"), "after\n").unwrap();
    record.finish().await;
    let blob = git(dir.path(), &["hash-object", "tracked.txt"]);
    let blob = blob.trim();
    fs::remove_file(
        record
            .directory
            .get()
            .unwrap()
            .path()
            .join("git/objects")
            .join(&blob[..2])
            .join(&blob[2..]),
    )
    .unwrap();
    assert!(
        store
            .comparison(&AttemptId::new("missing-blob"), 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn big_workspace_baselines_exclude_between_turn_edits_and_capture_current_turn_edits() {
    let dir = repository();
    for number in 0..5265 {
        fs::write(
            dir.path().join(format!("unchanged-{number:04}.txt")),
            "unchanged\n",
        )
        .unwrap();
    }
    git(dir.path(), &["add", "--all"]);
    let metadata = raw::git_files(dir.path());
    let store = Arc::default();
    let first = start(dir.path(), &store, "first");
    first.before_tools().await;
    first.finish().await;
    fs::write(dir.path().join("tracked.txt"), "between turns\n").unwrap();
    fs::write(dir.path().join("new.txt"), "between turns new\n").unwrap();
    fs::remove_file(dir.path().join("unchanged-0000.txt")).unwrap();
    let second = start(dir.path(), &store, "second");
    second.before_tools().await;
    second.finish().await;
    assert!(store.summary(&AttemptId::new("second")).is_none());
    let third = start(dir.path(), &store, "third");
    third.before_tools().await;
    fs::write(dir.path().join("tracked.txt"), "current turn\n").unwrap();
    fs::write(dir.path().join("created.txt"), "current new\n").unwrap();
    fs::remove_file(dir.path().join("unchanged-0001.txt")).unwrap();
    third.finish().await;
    let summary = store.summary(&AttemptId::new("third")).unwrap();
    assert_eq!(
        summary
            .files
            .iter()
            .map(|file| file.display.as_str())
            .collect::<Vec<_>>(),
        ["created.txt", "tracked.txt", "unchanged-0001.txt"]
    );
    assert_eq!((summary.added, summary.deleted), (2, 2));
    raw::text_change(
        &store,
        "third",
        "tracked.txt",
        "between turns\n",
        "current turn\n",
        (1, 1),
    )
    .await;
    assert_eq!(raw::git_files(dir.path()), metadata);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn unchanged_worktree_contents_are_not_reopened() {
    const CHILD: &str = "RUSTX_CHANGES_READ_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = Path::new(&root);
        let store = Arc::default();
        let warm = start(root, &store, "warm");
        warm.before_tools().await;
        warm.finish().await;
        // Separate file/index timestamps to avoid Git's racy-index rereads.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let warm = start(root, &store, "warm-again");
        warm.before_tools().await;
        warm.finish().await;
        fs::write(root.parent().unwrap().join("steady-phase"), "start").unwrap();
        for turn in 0..8 {
            let record = start(root, &store, &format!("steady-{turn}"));
            record.before_tools().await;
            record.finish().await;
        }
        return;
    }
    let owner = tempdir();
    let root = owner.path().join("workspace");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    for number in 0..5265 {
        fs::write(
            root.join(format!("unchanged-{number:04}.txt")),
            format!("contents-{number}\n"),
        )
        .unwrap();
    }
    git(&root, &["add", "--all"]);
    let trace = owner.path().join("reads.strace");
    let output = tokio::process::Command::new("strace")
        .args(["--kill-on-exit", "-f", "-qq", "-e", "trace=openat", "-o"])
        .arg(&trace)
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "workspace_changes::tests::work::unchanged_worktree_contents_are_not_reopened",
            "--test-threads=1",
        ])
        .env(CHILD, root)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let trace = fs::read_to_string(trace).unwrap();
    let steady = trace.split_once("steady-phase").unwrap().1;
    let reads = steady
        .lines()
        .filter(|line| line.contains("unchanged-") && line.contains("O_RDONLY"))
        .count();
    assert_eq!(
        reads, 0,
        "unchanged contents must remain behind the index stat cache"
    );
}

#[tokio::test]
async fn ambient_git_environment_cannot_redirect_snapshots_or_inject_configuration() {
    const CHILD: &str = "RUSTX_CHANGES_ENV_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = Path::new(&root);
        let metadata = raw::git_files(root);
        let store = Arc::default();
        let record = start(root, &store, "scrubbed");
        record.before_tools().await;
        fs::write(root.join("tracked.txt"), "scrubbed\n").unwrap();
        record.finish().await;
        raw::text_change(
            &store,
            "scrubbed",
            "tracked.txt",
            "one\ntwo\n",
            "scrubbed\n",
            (1, 2),
        )
        .await;
        assert_eq!(raw::git_files(root), metadata);
        return;
    }
    let dir = repository();
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "workspace_changes::tests::work::ambient_git_environment_cannot_redirect_snapshots_or_inject_configuration", "--test-threads=1"])
        .env(CHILD, dir.path())
        .env("GIT_DIR", "/nonexistent/injected-git")
        .env("GIT_INDEX_FILE", dir.path().join(".git/injected-index"))
        .env("GIT_OBJECT_DIRECTORY", dir.path().join(".git/injected-objects"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.bare")
        .env("GIT_CONFIG_VALUE_0", "true")
        .output().await.unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelled_git_reaps_before_the_next_snapshot_reuses_its_index() {
    use std::os::unix::fs::PermissionsExt;
    const CHILD: &str = "RUSTX_CHANGES_CANCEL_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = Path::new(&root);
        let marker = PathBuf::from(std::env::var_os("RUSTX_CHANGES_CANCEL_MARKER").unwrap());
        let store = Arc::default();
        let cancelled = start(root, &store, "cancelled-add");
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !marker.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cancelled.cancellation.cancel();
        cancelled.finish().await;
        let next = start(root, &store, "next-add");
        tokio::time::timeout(std::time::Duration::from_secs(10), next.before_tools())
            .await
            .unwrap();
        assert!(
            next.baseline.lock().unwrap().is_some(),
            "the abandoned private index lock must be removed after reaping"
        );
        fs::write(root.join("tracked.txt"), "next turn\n").unwrap();
        next.finish().await;
        raw::text_change(
            &store,
            "next-add",
            "tracked.txt",
            "one\ntwo\n",
            "next turn\n",
            (1, 2),
        )
        .await;
        return;
    }
    let dir = repository();
    let owner = tempdir();
    let marker = owner.path().join("once");
    let wrapper = owner.path().join("git");
    fs::write(
        &wrapper,
        r#"#!/bin/sh
for argument do
    if [ "$argument" = add ] && [ ! -f "$RUSTX_CHANGES_CANCEL_MARKER" ]; then
        : > "$GIT_INDEX_FILE.lock"
        : > "$RUSTX_CHANGES_CANCEL_MARKER"
        sleep 30
    fi
done
exec /usr/bin/git "$@"
"#,
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "workspace_changes::tests::work::cancelled_git_reaps_before_the_next_snapshot_reuses_its_index", "--test-threads=1"])
        .env(CHILD, dir.path())
        .env("RUSTX_CHANGES_CANCEL_MARKER", marker)
        .env("PATH", format!("{}:/usr/bin:/bin", owner.path().display()))
        .output().await.unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn captured_oversized_endpoints_have_no_counts_or_binary_comparison() {
    let dir = tempdir();
    let path = dir.path().join("large.dat");
    fs::write(&path, b"before\0").unwrap();
    let store = Arc::default();
    let record = start(dir.path(), &store, "oversized");
    record.before_tools().await;
    record.capture(&path);
    fs::write(&path, vec![0; capture::MAX_FILE_BYTES + 1]).unwrap();
    record.finish().await;
    let summary = store.summary(&AttemptId::new("oversized")).unwrap();
    let file = &summary.files[0];
    assert!(file.oversized && !file.binary);
    assert_eq!((summary.added, summary.deleted), (0, 0));
    let comparison = store
        .comparison(&AttemptId::new("oversized"), 0)
        .await
        .unwrap()
        .unwrap();
    assert!(comparison.oversized && !comparison.binary && comparison.hunks.is_empty());
    let record = start(dir.path(), &store, "both-oversized");
    record.before_tools().await;
    record.capture(&path);
    record.finish().await;
    assert!(
        store
            .summary(&AttemptId::new("both-oversized"))
            .unwrap()
            .files[0]
            .oversized,
        "two unread oversized sides are never assumed equal"
    );
}

#[tokio::test]
async fn host_tmp_stays_excluded_when_the_platform_temporary_root_is_custom() {
    const CHILD: &str = "RUSTX_CHANGES_TEMP_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let workspace = tempdir();
        let outside = tempfile::tempdir_in("/tmp").unwrap();
        let store = Arc::default();
        let record = start(workspace.path(), &store, "custom-temp");
        record.before_tools().await;
        for path in [
            workspace.path().join("kept.txt"),
            outside.path().join("excluded.txt"),
        ] {
            record.capture(&path);
            fs::write(path, "created\n").unwrap();
        }
        record.finish().await;
        let summary = store.summary(&AttemptId::new("custom-temp")).unwrap();
        assert_eq!(summary.total, 1);
        assert_eq!(summary.files[0].display, "kept.txt");
        return;
    }
    let temporary_root = tempdir();
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "workspace_changes::tests::work::host_tmp_stays_excluded_when_the_platform_temporary_root_is_custom", "--test-threads=1"])
        .env(CHILD, "1").env("TMPDIR", temporary_root.path()).output().await.unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
