use super::*;

fn mark_skipped(root: &Path) {
    git(root, &["update-index", "--skip-worktree", "tracked.txt"]);
    assert_eq!(
        git(root, &["ls-files", "-v", "--", "tracked.txt"]),
        "S tracked.txt\n"
    );
}

fn unchanged_skipped_index(root: &Path, index: &[u8]) {
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(
        git(root, &["ls-files", "-v", "--", "tracked.txt"]),
        "S tracked.txt\n"
    );
}

async fn deleted_file(store: &ChangesStore, id: &str, before: &str) {
    let attempt = AttemptId::new(id);
    let summary = store.summary(&attempt).unwrap();
    let index = summary
        .files
        .iter()
        .position(|file| file.display == "tracked.txt")
        .expect("a physically deleted file must remain in the change list");
    let file = &summary.files[index];
    assert_eq!((file.added, file.deleted), (0, 2));
    assert!(!file.binary && !file.oversized);
    let comparison = store.comparison(&attempt, index).await.unwrap().unwrap();
    assert!(!comparison.binary && !comparison.oversized && !comparison.truncated);
    assert_eq!(comparison.hunks.len(), 1);
    assert_eq!(comparison.hunks[0].old_text, before);
    assert!(comparison.hunks[0].new_text.is_empty());
}

#[tokio::test]
async fn skip_worktree_file_deletion_remains_hidden_by_git() {
    let dir = repository();
    let root = dir.path();
    let before = "already dirty\nactual worktree contents\n";
    fs::write(root.join("tracked.txt"), before).unwrap();
    mark_skipped(root);
    let index = fs::read(root.join(".git/index")).unwrap();
    let store = Arc::default();
    let record = start(root, &store, "deleted");
    record.before_tools().await;
    fs::remove_file(root.join("tracked.txt")).unwrap();
    record.finish().await;

    assert!(store.summary(&AttemptId::new("deleted")).is_none());
    unchanged_skipped_index(root, &index);
}

#[tokio::test]
async fn skip_worktree_deletion_is_hidden_alongside_other_edits() {
    let dir = repository();
    let root = dir.path();
    fs::write(root.join("other.txt"), "keep\nold\n").unwrap();
    git(root, &["add", "other.txt"]);
    mark_skipped(root);
    let index = fs::read(root.join(".git/index")).unwrap();
    let store = Arc::default();
    let record = start(root, &store, "mixed");
    record.before_tools().await;
    fs::remove_file(root.join("tracked.txt")).unwrap();
    fs::write(root.join("other.txt"), "keep\nnew\n").unwrap();
    record.finish().await;

    let summary = store.summary(&AttemptId::new("mixed")).unwrap();
    assert_eq!((summary.total, summary.added, summary.deleted), (1, 1, 1));
    assert_eq!(
        summary
            .files
            .iter()
            .map(|file| file.display.as_str())
            .collect::<Vec<_>>(),
        ["other.txt"]
    );
    assert_eq!((summary.files[0].added, summary.files[0].deleted), (1, 1));
    unchanged_skipped_index(root, &index);
}

#[tokio::test]
async fn sparse_file_already_absent_at_baseline_creates_no_changes_card() {
    let dir = repository();
    let root = dir.path();
    mark_skipped(root);
    fs::remove_file(root.join("tracked.txt")).unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let store = Arc::default();
    let record = start(root, &store, "absent");
    record.before_tools().await;
    record.finish().await;

    assert!(store.summary(&AttemptId::new("absent")).is_none());
    unchanged_skipped_index(root, &index);
}

#[tokio::test]
async fn skip_worktree_materialization_remains_hidden_by_git() {
    let dir = repository();
    let root = dir.path();
    mark_skipped(root);
    fs::remove_file(root.join("tracked.txt")).unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let store = Arc::default();
    let record = start(root, &store, "materialized");
    record.before_tools().await;
    let after = "newly materialized\nactual contents\n";
    fs::write(root.join("tracked.txt"), after).unwrap();
    record.finish().await;

    assert!(store.summary(&AttemptId::new("materialized")).is_none());
    unchanged_skipped_index(root, &index);
}

#[tokio::test]
async fn adding_skip_worktree_during_an_attempt_does_not_hide_a_deletion() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    let record = start(root, &store, "new-flag");
    record.before_tools().await;
    mark_skipped(root);
    let index = fs::read(root.join(".git/index")).unwrap();
    fs::remove_file(root.join("tracked.txt")).unwrap();
    record.finish().await;

    let summary = store.summary(&AttemptId::new("new-flag")).unwrap();
    assert_eq!((summary.total, summary.added, summary.deleted), (1, 0, 2));
    deleted_file(&store, "new-flag", "one\ntwo\n").await;
    unchanged_skipped_index(root, &index);
}
