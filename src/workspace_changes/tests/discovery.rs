//! Fresh discovery, membership and configuration at every endpoint/attempt.
use super::*;

async fn edit(root: &Path, name: &str, expected_before: &str) {
    let store = Arc::default();
    let record = start(root, &store, "fresh");
    record.before_tools().await;
    assert!(record.baseline.lock().unwrap().is_some());
    assert!(record.directory.get().is_none());
    let after = if expected_before == "changed\n" {
        "different\n"
    } else {
        "changed\n"
    };
    fs::write(root.join(name), after).unwrap();
    record.finish().await;
    raw::text_change(
        &store,
        "fresh",
        name,
        expected_before,
        after,
        (1, expected_before.lines().count()),
    )
    .await;
}

#[tokio::test]
async fn fresh_attempts_observe_checkout_commit_stash_and_index_replacement() {
    let dir = repository();
    let root = dir.path();
    let first = git(root, &["rev-parse", "HEAD"]);
    edit(root, "tracked.txt", "one\ntwo\n").await;
    git(root, &["add", "tracked.txt"]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "second",
        ],
    );
    edit(root, "tracked.txt", "changed\n").await;
    fs::write(root.join("tracked.txt"), "stashed\n").unwrap();
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "stash",
            "push",
            "-qm",
            "stash",
        ],
    );
    edit(root, "tracked.txt", "changed\n").await;
    git(root, &["reset", "--hard", "HEAD"]);
    git(root, &["stash", "pop", "--quiet"]);
    edit(root, "tracked.txt", "stashed\n").await;
    git(root, &["checkout", "--force", "--detach", first.trim()]);
    // Replace the actual index with a new one as reset/checkouts do.
    fs::remove_file(root.join(".git/index")).unwrap();
    git(root, &["reset", "--mixed", "HEAD"]);
    edit(root, "tracked.txt", "one\ntwo\n").await;
}

#[tokio::test]
async fn linked_worktree_and_retargeted_gitfile_use_the_current_index() {
    let dir = repository();
    let owner = tempdir();
    let linked = owner.path().join("linked");
    git(
        dir.path(),
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    edit(&linked, "tracked.txt", "one\ntwo\n").await;
    let other = owner.path().join("other");
    git(
        dir.path(),
        &["worktree", "add", "--detach", other.to_str().unwrap()],
    );
    // A changed gitfile is read afresh; worktree-local index/HEAD must not come
    // from the preceding attempt's linked repository.
    fs::copy(other.join(".git"), linked.join(".git")).unwrap();
    edit(&linked, "tracked.txt", "changed\n").await;
}

#[tokio::test]
async fn local_and_included_config_change_ignore_membership_without_processes() {
    let dir = repository();
    let root = dir.path();
    let excludes = root.join(".git/extra-excludes");
    fs::write(root.join("generated.log"), "before\n").unwrap();
    fs::write(&excludes, "generated.log\n").unwrap();
    let included = root.join(".git/included-config");
    fs::write(
        &included,
        format!("[core]\nexcludesFile = {}\n", excludes.display()),
    )
    .unwrap();
    git(
        root,
        &["config", "include.path", included.to_str().unwrap()],
    );
    let store = Arc::default();
    let record = start(root, &store, "ignored");
    record.before_tools().await;
    fs::write(root.join("generated.log"), "ignored change\n").unwrap();
    record.finish().await;
    assert!(store.summary(&AttemptId::new("ignored")).is_none());
    assert!(record.directory.get().is_none());
    fs::write(&excludes, "").unwrap();
    edit(root, "generated.log", "ignored change\n").await;
}

#[cfg(unix)]
#[tokio::test]
async fn symbolic_git_directory_and_workspace_alias_preserve_physical_identity() {
    let dir = repository();
    let owner = tempdir();
    let metadata = owner.path().join("metadata");
    fs::rename(dir.path().join(".git"), &metadata).unwrap();
    std::os::unix::fs::symlink(&metadata, dir.path().join(".git")).unwrap();
    let alias = owner.path().join("alias");
    std::os::unix::fs::symlink(dir.path(), &alias).unwrap();
    edit(&alias, "tracked.txt", "one\ntwo\n").await;
}

#[tokio::test]
async fn bare_configuration_does_not_allocate_snapshot_storage() {
    let dir = repository();
    git(dir.path(), &["config", "core.bare", "true"]);
    let record = start(dir.path(), &Arc::default(), "bare");
    record.before_tools().await;
    assert!(record.baseline.lock().unwrap().is_none());
    record.finish().await;
    assert!(record.directory.get().is_none());
}

#[tokio::test]
async fn sha256_and_split_indices_keep_raw_endpoints_without_writing_the_index() {
    for sha256 in [false, true] {
        let dir = tempdir();
        let root = dir.path();
        git(
            root,
            &[
                "init",
                "-q",
                if sha256 {
                    "--object-format=sha256"
                } else {
                    "--object-format=sha1"
                },
            ],
        );
        fs::write(root.join("tracked.txt"), "one\ntwo\n").unwrap();
        git(root, &["add", "tracked.txt"]);
        git(root, &["update-index", "--split-index"]);
        let index = fs::read(root.join(".git/index")).unwrap();
        edit(root, "tracked.txt", "one\ntwo\n").await;
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    }
}

#[tokio::test]
async fn replacing_repository_format_during_an_attempt_preserves_the_original_raw_baseline() {
    let dir = repository();
    let root = dir.path();
    let store = Arc::default();
    let record = start(root, &store, "replacement");
    record.before_tools().await;
    fs::remove_dir_all(root.join(".git")).unwrap();
    git(root, &["init", "-q", "--object-format=sha256"]);
    git(root, &["add", "tracked.txt"]);
    fs::write(root.join("tracked.txt"), "after replacement\n").unwrap();
    record.finish().await;
    raw::text_change(
        &store,
        "replacement",
        "tracked.txt",
        "one\ntwo\n",
        "after replacement\n",
        (1, 2),
    )
    .await;
}

#[tokio::test]
async fn removing_an_inner_repository_keeps_the_attempt_root_with_enclosing_git_membership() {
    let parent = repository();
    let root = parent.path().join("inner");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    fs::write(root.join("tracked.txt"), "before\n").unwrap();
    git(&root, &["add", "tracked.txt"]);
    let store = Arc::default();
    let record = start(&root, &store, "enclosing");
    record.before_tools().await;
    fs::remove_dir_all(root.join(".git")).unwrap();
    fs::write(root.join("created.txt"), "new\n").unwrap();
    git(
        parent.path(),
        &["add", "inner/created.txt", "inner/tracked.txt"],
    );
    record.finish().await;
    let summary = store.summary(&AttemptId::new("enclosing")).unwrap();
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].display, "created.txt");
    assert_eq!((summary.added, summary.deleted), (1, 0));
}
