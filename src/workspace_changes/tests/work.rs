//! Deterministic per-attempt work, including a fully settled inference baseline.
use super::*;

#[tokio::test]
async fn git_text_and_read_turns_create_no_processes_or_snapshot_directories() {
    const CHILD_ROOT: &str = "RUSTX_WORKSPACE_CHANGES_PROCESS_CHILD";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        unchanged_rounds(Path::new(&root)).await;
        return;
    }
    let dir = repository();
    #[cfg(target_os = "linux")]
    {
        // Setup Git outside the traced child. Count all process/FS creation,
        // including library-internal launches that bypass our command driver.
        let trace_owner = tempdir();
        let trace = trace_owner.path().join("work.strace");
        let mut command = tokio::process::Command::new("strace");
        command.args(["--kill-on-exit", "-f", "-qq", "-e", "trace=process,mkdir,mkdirat", "-o"])
            .arg(&trace).arg(std::env::current_exe().unwrap())
            .args(["--exact", "workspace_changes::tests::work::git_text_and_read_turns_create_no_processes_or_snapshot_directories", "--nocapture", "--test-threads=1"])
            .env(CHILD_ROOT, dir.path()).kill_on_drop(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(60), command.output())
            .await
            .expect("the traced workload must settle")
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        let trace = fs::read_to_string(trace).unwrap();
        let mut executions = 0;
        for line in trace.lines() {
            if line.contains("execve(") || line.contains("execveat(") {
                executions += 1;
            }
            assert!(
                !line.contains("fork(") && !line.contains("vfork("),
                "{line}"
            );
            if line.contains("clone(") || line.contains("clone3(") {
                assert!(
                    line.contains("CLONE_THREAD"),
                    "unexpected process creation: {line}"
                );
            }
            assert!(
                !line.contains("mkdir(") && !line.contains("mkdirat("),
                "unexpected directory creation: {line}"
            );
        }
        assert_eq!(
            executions, 1,
            "only the test child's initial exec is allowed"
        );
    }
    #[cfg(not(target_os = "linux"))]
    unchanged_rounds(dir.path()).await;
}

async fn unchanged_rounds(root: &Path) {
    let metadata = raw::git_files(root);
    let store = Arc::default();
    for tool in [false, true] {
        for turn in 0..64 {
            let record = start(root, &store, &format!("work-{tool}-{turn}"));
            // A slow inference must be just as cheap as one that cancels an
            // unfinished baseline: force every baseline to finish here.
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
            assert!(record.directory.get().is_none());
            if tool {
                record.before_tools().await;
                assert_eq!(
                    fs::read_to_string(root.join("tracked.txt")).unwrap(),
                    "one\ntwo\n"
                );
            }
            record.finish().await;
            assert_eq!(commands.load(Ordering::Relaxed), 0);
            assert!(record.directory.get().is_none());
        }
    }
    assert!(store.0.lock().unwrap().is_empty());
    assert_eq!(raw::git_files(root), metadata);
}

#[tokio::test]
async fn changed_turn_uses_one_supervised_diff_and_keeps_the_inference_baseline() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    let record = start(root, &store, "external-during-inference");
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
    // This external edit happens while the model is still waiting. Even a
    // subsequent read-only tool must retain it as an attempt change.
    fs::write(root.join("tracked.txt"), "external during inference\n").unwrap();
    record.before_tools().await;
    record.finish().await;
    assert_eq!(commands.load(Ordering::Relaxed), 1);
    assert!(record.directory.get().is_some());
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
        1,
        "comparisons read owned memory"
    );
}

#[tokio::test]
async fn completed_comparison_survives_repository_removal() {
    let dir = repository();
    let store = Arc::default();
    let record = start(dir.path(), &store, "retained");
    record.before_tools().await;
    fs::write(dir.path().join("tracked.txt"), "after\n").unwrap();
    record.finish().await;
    fs::remove_dir_all(dir.path().join(".git")).unwrap();
    raw::text_change(
        &store,
        "retained",
        "tracked.txt",
        "one\ntwo\n",
        "after\n",
        (1, 2),
    )
    .await;
}

#[tokio::test]
async fn invalid_git_tree_paths_disable_observation_before_a_capture_can_publish() {
    let dir = repository();
    let root = dir.path();
    fs::create_dir(root.join(".GIT")).unwrap();
    fs::write(root.join(".GIT/file"), "invalid Git index path\n").unwrap();
    let store = Arc::default();
    let record = start(root, &store, "invalid-path");
    record.before_tools().await;
    assert!(record.failed.load(Ordering::Relaxed));
    record.capture(&root.join("tracked.txt"));
    fs::write(root.join("tracked.txt"), "after\n").unwrap();
    record.finish().await;
    assert!(store.summary(&AttemptId::new("invalid-path")).is_none());
}

#[tokio::test]
async fn failed_private_repository_open_never_enables_writes_to_the_real_object_store() {
    let dir = repository();
    let root = dir.path();
    let scratch = Arc::new(create_snapshot_directory(&std::env::temp_dir()).unwrap());
    let private = scratch.path().join("git");
    fs::create_dir(&private).unwrap();
    fs::create_dir(private.join("objects")).unwrap();
    fs::create_dir(private.join("refs")).unwrap();
    fs::write(private.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(private.join("config"), "[broken\n").unwrap();
    let repo = Repository::locate(
        root,
        Arc::new(OnceLock::from(scratch)),
        CancellationSignal::new(),
    )
    .await
    .unwrap()
    .unwrap();
    let before = repo.snapshot().await.unwrap();
    let metadata = raw::git_files(root);
    assert!(repo.materialize().await.is_err());
    fs::write(root.join("tracked.txt"), "after failed materialization\n").unwrap();
    assert_ne!(repo.snapshot().await.unwrap(), before);
    assert_eq!(raw::git_files(root), metadata);
}

#[tokio::test]
async fn capture_only_cards_release_repository_snapshots_and_keep_their_comparisons() {
    let dir = repository();
    let root = dir.path();
    fs::write(root.join(".gitignore"), "*.ignored\n").unwrap();
    fs::write(root.join("config.ignored"), "before\n").unwrap();
    let store = Arc::default();
    let record = start(root, &store, "capture-only");
    record.before_tools().await;
    record.capture(&root.join("config.ignored"));
    fs::write(root.join("config.ignored"), "after\n").unwrap();
    record.finish().await;
    let completed = store.0.lock().unwrap().get("capture-only").unwrap().clone();
    assert!(completed.repository.is_none());
    assert!(completed.trees.is_none());
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
async fn comparisons_reject_corrupted_private_trees_instead_of_trusting_cached_membership() {
    let dir = repository();
    let store = Arc::default();
    let record = start(dir.path(), &store, "corrupt-tree");
    record.before_tools().await;
    fs::write(dir.path().join("tracked.txt"), "after\n").unwrap();
    record.finish().await;
    let completed = store.0.lock().unwrap().get("corrupt-tree").unwrap().clone();
    let before = &completed.trees.as_ref().unwrap().0;
    let object = record
        .directory
        .get()
        .unwrap()
        .path()
        .join("git/objects")
        .join(&before[..2])
        .join(&before[2..]);
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
async fn missing_private_blobs_keep_the_existing_unknown_endpoint_result() {
    let dir = repository();
    let store = Arc::default();
    let record = start(dir.path(), &store, "missing-blob");
    record.before_tools().await;
    fs::write(dir.path().join("tracked.txt"), "after\n").unwrap();
    record.finish().await;
    let completed = store.0.lock().unwrap().get("missing-blob").unwrap().clone();
    let before = &completed.trees.as_ref().unwrap().0;
    let private = record.directory.get().unwrap().path().join("git");
    let blob = git(&private, &["rev-parse", &format!("{before}:tracked.txt")]);
    let blob = blob.trim();
    fs::remove_file(private.join("objects").join(&blob[..2]).join(&blob[2..])).unwrap();
    assert!(
        store
            .comparison(&AttemptId::new("missing-blob"), 0)
            .await
            .unwrap()
            .unwrap()
            .oversized
    );
}

#[tokio::test]
async fn cancelled_materialization_creates_no_private_directory() {
    let dir = repository();
    let cancellation = CancellationSignal::new();
    let directory = Arc::new(OnceLock::new());
    let repo = Repository::locate(dir.path(), Arc::clone(&directory), cancellation.clone())
        .await
        .unwrap()
        .unwrap();
    repo.snapshot().await.unwrap();
    cancellation.cancel();
    assert!(repo.materialize().await.is_err());
    assert!(directory.get().is_none());
}
