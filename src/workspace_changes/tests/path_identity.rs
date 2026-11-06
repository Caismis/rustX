use super::*;

async fn assert_changes(store: &ChangesStore, id: &str, expected: &[(&Path, &str, &str, &str)]) {
    let attempt = AttemptId::new(id);
    let summary = store.summary(&attempt).unwrap();
    assert_eq!(summary.total, expected.len());
    assert_eq!(summary.files.len(), expected.len());
    assert_eq!(
        (summary.added, summary.deleted),
        (expected.len(), expected.len())
    );
    for (index, (path, display, before, after)) in expected.iter().enumerate() {
        let file = &summary.files[index];
        assert_eq!(file.path, path.to_string_lossy());
        assert_eq!(file.display, *display);
        assert_eq!((file.added, file.deleted), (1, 1));
        let comparison = store.comparison(&attempt, index).await.unwrap().unwrap();
        assert!(!comparison.binary);
        assert!(!comparison.oversized);
        assert_eq!(comparison.hunks.len(), 1);
        assert_eq!(comparison.hunks[0].old_text, *before);
        assert_eq!(comparison.hunks[0].new_text, *after);
    }
}

#[tokio::test]
async fn tracked_literal_backslash_and_directory_paths_keep_both_comparisons() {
    let dir = repository();
    let root = dir.path();
    fs::create_dir(root.join("a")).unwrap();
    let nested = root.join("a/b.txt");
    let literal = root.join(r"a\b.txt");
    fs::write(&nested, "nested before\n").unwrap();
    fs::write(&literal, "literal before\n").unwrap();
    git(root, &["add", "--", "a/b.txt", r"a\b.txt"]);
    let index = fs::read(root.join(".git/index")).unwrap();
    let store = Arc::default();
    let record = start(root, &store, "tracked-paths");
    record.before_tools().await;
    fs::write(&nested, "nested after\n").unwrap();
    fs::write(&literal, "literal after\n").unwrap();
    record.finish().await;

    assert_changes(
        &store,
        "tracked-paths",
        &[
            (&nested, "a/b.txt", "nested before\n", "nested after\n"),
            (&literal, r"a\b.txt", "literal before\n", "literal after\n"),
        ],
    )
    .await;
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
}

#[tokio::test]
async fn git_and_ignored_capture_paths_keep_both_comparisons() {
    let dir = repository();
    let root = dir.path();
    fs::create_dir(root.join("a")).unwrap();
    let nested = root.join("a/b.txt");
    let literal = root.join(r"a\b.txt");
    fs::write(&nested, "nested before\n").unwrap();
    fs::write(&literal, "literal before\n").unwrap();
    // Force-track the nested path; the literal-backslash path remains ignored
    // and can only be observed through its native first-mutation capture.
    fs::write(root.join(".gitignore"), "*.txt\n").unwrap();
    git(root, &["add", "-f", "--", "a/b.txt"]);
    git(root, &["check-ignore", "--quiet", "--", r"a\b.txt"]);
    let index = fs::read(root.join(".git/index")).unwrap();
    let store = Arc::default();
    let record = start(root, &store, "mixed-paths");
    record.before_tools().await;
    record.capture(&literal);
    fs::write(&nested, "nested after\n").unwrap();
    fs::write(&literal, "literal after\n").unwrap();
    record.finish().await;

    assert_changes(
        &store,
        "mixed-paths",
        &[
            (&nested, "a/b.txt", "nested before\n", "nested after\n"),
            (&literal, r"a\b.txt", "literal before\n", "literal after\n"),
        ],
    )
    .await;
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
}

#[tokio::test]
async fn native_literal_backslash_and_directory_paths_keep_both_comparisons() {
    let dir = tempdir();
    let root = dir.path();
    fs::create_dir(root.join("a")).unwrap();
    let nested = root.join("a/b.txt");
    let literal = root.join(r"a\b.txt");
    fs::write(&nested, "nested before\n").unwrap();
    fs::write(&literal, "literal before\n").unwrap();
    let store = Arc::default();
    let record = start(root, &store, "native-paths");
    record.before_tools().await;
    record.capture(&nested);
    record.capture(&literal);
    fs::write(&nested, "nested after\n").unwrap();
    fs::write(&literal, "literal after\n").unwrap();
    record.finish().await;

    assert_changes(
        &store,
        "native-paths",
        &[
            (&nested, "a/b.txt", "nested before\n", "nested after\n"),
            (&literal, r"a\b.txt", "literal before\n", "literal after\n"),
        ],
    )
    .await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn native_paths_with_identical_lossy_labels_keep_both_comparisons() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

    let dir = tempdir();
    let root = dir.path();
    let first = root.join(OsStr::from_bytes(b"file-\x80.txt"));
    let second = root.join(OsStr::from_bytes(b"file-\x81.txt"));
    assert_ne!(first, second);
    assert_eq!(first.to_string_lossy(), second.to_string_lossy());
    fs::write(&first, "first before\n").unwrap();
    fs::write(&second, "second before\n").unwrap();
    let store = Arc::default();
    let record = start(root, &store, "lossy-paths");
    record.before_tools().await;
    // Reverse capture order to verify deterministic ordering comes from the
    // real paths even when every serialized path/display string is identical.
    record.capture(&second);
    record.capture(&first);
    fs::write(&first, "first after\n").unwrap();
    fs::write(&second, "second after\n").unwrap();
    record.finish().await;

    assert_changes(
        &store,
        "lossy-paths",
        &[
            (
                &first,
                "file-\u{fffd}.txt",
                "first before\n",
                "first after\n",
            ),
            (
                &second,
                "file-\u{fffd}.txt",
                "second before\n",
                "second after\n",
            ),
        ],
    )
    .await;
}
