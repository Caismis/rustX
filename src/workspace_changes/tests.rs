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
