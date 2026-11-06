use super::*;

fn git_files(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                files.insert(path.strip_prefix(root).unwrap().to_owned(), None);
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    Some(fs::read(path).unwrap()),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, &root.join(".git"), &mut files);
    files
}

#[cfg(unix)]
fn program(root: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join(".git").join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn shell_path(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

async fn change(
    store: &ChangesStore,
    attempt: &str,
    display: &str,
) -> (ChangedFile, FileComparison) {
    let attempt = AttemptId::new(attempt);
    let summary = store
        .summary(&attempt)
        .expect("the recorded edit must be available");
    let index = summary
        .files
        .iter()
        .position(|file| file.display == display)
        .unwrap();
    let comparison = store.comparison(&attempt, index).await.unwrap().unwrap();
    (summary.files[index].clone(), comparison)
}

async fn text_change(
    store: &ChangesStore,
    attempt: &str,
    path: &str,
    before: &str,
    after: &str,
    counts: (usize, usize),
) {
    let (file, comparison) = change(store, attempt, path).await;
    assert_eq!((file.added, file.deleted), counts, "{path}");
    assert!(!file.binary && !file.oversized, "{path}");
    assert!(
        !comparison.binary && !comparison.oversized && !comparison.truncated,
        "{path}"
    );
    assert_eq!(comparison.hunks.len(), 1, "{path}");
    assert_eq!(comparison.hunks[0].old_text, before, "{path}");
    assert_eq!(comparison.hunks[0].new_text, after, "{path}");
}

async fn binary_change(store: &ChangesStore, attempt: &str, path: &str) {
    let (file, comparison) = change(store, attempt, path).await;
    assert!(file.binary && comparison.binary, "{path}");
    assert!(
        !file.oversized && !comparison.oversized && !comparison.truncated,
        "{path}"
    );
    assert_eq!((file.added, file.deleted), (0, 0), "{path}");
    assert!(comparison.hunks.is_empty(), "{path}");
}

#[cfg(unix)]
#[tokio::test]
async fn baseline_and_final_snapshots_never_execute_hooks_or_required_filters() {
    let directory = repository();
    let root = directory.path();
    let hook_marker = root.join(".git/hook-ran");
    program(
        root,
        "hooks/post-index-change",
        &format!("printf ran >> {}", shell_path(&hook_marker)),
    );
    let clean = program(
        root,
        "clean-filter",
        &format!(
            "printf ran >> {}\ncat",
            shell_path(&root.join(".git/clean-ran"))
        ),
    );
    // Deliberately invalid process protocol, and a required clean executable
    // that does not exist: neither may turn observation into execution/failure.
    let process = program(
        root,
        "process-filter",
        &format!(
            "printf ran >> {}\nprintf invalid\\n\nexit 1",
            shell_path(&root.join(".git/process-ran"))
        ),
    );
    git(root, &["config", "filter.clean.clean", &shell_path(&clean)]);
    git(root, &["config", "filter.clean.required", "true"]);
    git(
        root,
        &["config", "filter.process.process", &shell_path(&process)],
    );
    git(root, &["config", "filter.process.required", "true"]);
    git(
        root,
        &[
            "config",
            "filter.missing.clean",
            &shell_path(&root.join(".git/does-not-exist")),
        ],
    );
    git(root, &["config", "filter.missing.required", "true"]);
    fs::write(
        root.join(".gitattributes"),
        "clean.txt filter=clean\nprocess.txt filter=process\nmissing.txt filter=missing\n",
    )
    .unwrap();
    for name in ["clean.txt", "process.txt", "missing.txt"] {
        fs::write(root.join(name), "before\n").unwrap();
    }
    let metadata = git_files(root);
    let store = Arc::default();
    let record = start(root, &store, "programs");
    record.before_tools().await;
    assert_eq!(
        git_files(root),
        metadata,
        "baseline must not execute any repository program"
    );
    for name in ["clean.txt", "process.txt", "missing.txt"] {
        fs::write(root.join(name), "after\n").unwrap();
    }
    record.finish().await;
    assert_eq!(store.summary(&AttemptId::new("programs")).unwrap().total, 3);
    for name in ["clean.txt", "process.txt", "missing.txt"] {
        text_change(&store, "programs", name, "before\n", "after\n", (1, 1)).await;
    }
    assert_eq!(
        git_files(root),
        metadata,
        "final snapshot and comparisons must leave all real Git files untouched"
    );
    assert!(!hook_marker.exists());
    assert!(!root.join(".git/clean-ran").exists());
    assert!(!root.join(".git/process-ran").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn raw_endpoints_preserve_text_and_binary_for_native_and_shell_equivalent_edits() {
    let directory = repository();
    let root = directory.path();
    let filter = program(
        root,
        "pointer-filter",
        &format!(
            "printf ran >> {}\ncat >/dev/null\nprintf 'version https://git-lfs.github.com/spec/v1\\noid sha256:0000\\nsize 21\\n'",
            shell_path(&root.join(".git/pointer-ran"))
        ),
    );
    git(
        root,
        &["config", "filter.pointer.clean", &shell_path(&filter)],
    );
    git(root, &["config", "filter.pointer.required", "true"]);
    fs::write(
        root.join(".gitattributes"),
        "*.txt filter=pointer\n*.bin filter=pointer\n",
    )
    .unwrap();
    let before = "keep\nold-a\nold-b\nend\n";
    let after = "keep\nnew-a\nnew-b\nend\n";
    for name in ["native.txt", "shell.txt"] {
        fs::write(root.join(name), before).unwrap();
    }
    for name in ["native.bin", "shell.bin"] {
        fs::write(root.join(name), b"old\0bytes").unwrap();
    }
    let metadata = git_files(root);
    let store = Arc::default();
    let record = start(root, &store, "raw");
    record.before_tools().await;
    assert_eq!(git_files(root), metadata);
    for name in ["native.txt", "native.bin"] {
        record.capture(&root.join(name));
    }
    // Direct writes model Bash/subagent mutations without a native before-image.
    for name in ["native.txt", "shell.txt"] {
        fs::write(root.join(name), after).unwrap();
    }
    for name in ["native.bin", "shell.bin"] {
        fs::write(root.join(name), b"new\0bytes").unwrap();
    }
    record.finish().await;
    assert_eq!(store.summary(&AttemptId::new("raw")).unwrap().total, 4);
    for name in ["native.txt", "shell.txt"] {
        text_change(&store, "raw", name, before, after, (2, 2)).await;
    }
    for name in ["native.bin", "shell.bin"] {
        binary_change(&store, "raw", name).await;
    }
    assert_eq!(git_files(root), metadata);
    assert!(!root.join(".git/pointer-ran").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn encoding_attributes_and_diff_drivers_cannot_transform_observed_endpoints() {
    let directory = repository();
    let root = directory.path();
    let external = tempdir();
    let attributes = external.path().join("attributes");
    fs::write(&attributes, "global.txt -diff\nforced.bin diff\n").unwrap();
    git(
        root,
        &[
            "config",
            "core.attributesFile",
            attributes.to_str().unwrap(),
        ],
    );
    git(root, &["config", "core.autocrlf", "true"]);
    let converter = program(
        root,
        "text-converter",
        &format!(
            "printf ran >> {}\nprintf normalized\\n",
            shell_path(&root.join(".git/textconv-ran"))
        ),
    );
    let differ = program(
        root,
        "external-diff",
        &format!(
            "printf ran >> {}\nprintf normalized\\n",
            shell_path(&root.join(".git/diff-ran"))
        ),
    );
    git(
        root,
        &["config", "diff.observer.textconv", &shell_path(&converter)],
    );
    git(root, &["config", "diff.external", &shell_path(&differ)]);
    fs::write(root.join(".gitattributes"), "local.txt -diff\nconverted.txt diff=observer\ncrlf.txt text eol=lf\nencoded.txt text working-tree-encoding=UTF-16LE\n").unwrap();
    for name in ["global.txt", "local.txt", "converted.txt"] {
        fs::write(root.join(name), "before\n").unwrap();
    }
    fs::write(root.join("crlf.txt"), b"keep\r\nbefore\r\n").unwrap();
    fs::write(root.join("encoded.txt"), b"o\0l\0d\0\n\0").unwrap();
    fs::write(root.join("forced.bin"), b"old\0bytes").unwrap();
    let metadata = git_files(root);
    let store = Arc::default();
    let record = start(root, &store, "attributes");
    record.before_tools().await;
    assert_eq!(git_files(root), metadata);
    for name in ["global.txt", "local.txt", "converted.txt"] {
        fs::write(root.join(name), "after\n").unwrap();
    }
    fs::write(root.join("crlf.txt"), b"keep\r\nafter\r\n").unwrap();
    fs::write(root.join("encoded.txt"), b"n\0e\0w\0\n\0").unwrap();
    fs::write(root.join("forced.bin"), b"new\0bytes").unwrap();
    record.finish().await;
    assert_eq!(
        store.summary(&AttemptId::new("attributes")).unwrap().total,
        6
    );
    for name in ["global.txt", "local.txt", "converted.txt"] {
        text_change(&store, "attributes", name, "before\n", "after\n", (1, 1)).await;
    }
    text_change(
        &store,
        "attributes",
        "crlf.txt",
        "keep\r\nbefore\r\n",
        "keep\r\nafter\r\n",
        (1, 1),
    )
    .await;
    for name in ["encoded.txt", "forced.bin"] {
        binary_change(&store, "attributes", name).await;
    }
    assert_eq!(git_files(root), metadata);
    assert!(!root.join(".git/textconv-ran").exists());
    assert!(!root.join(".git/diff-ran").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn sha256_snapshots_keep_quoted_newline_paths_and_symlink_bytes_literal() {
    use std::os::unix::fs::symlink;
    let directory = tempdir();
    let root = directory.path();
    git(root, &["init", "-q", "--object-format=sha256"]);
    let name = "quote'\"\ttab\n[0].txt";
    fs::write(root.join(name), "before\n").unwrap();
    symlink("old-target", root.join("link")).unwrap();
    git(root, &["add", "."]);
    let metadata = git_files(root);
    let store = Arc::default();
    let record = start(root, &store, "sha256");
    record.before_tools().await;
    fs::write(root.join(name), "after\n").unwrap();
    fs::remove_file(root.join("link")).unwrap();
    symlink("new-target", root.join("link")).unwrap();
    record.finish().await;
    assert_eq!(store.summary(&AttemptId::new("sha256")).unwrap().total, 2);
    text_change(&store, "sha256", name, "before\n", "after\n", (1, 1)).await;
    text_change(
        &store,
        "sha256",
        "link",
        "old-target\n",
        "new-target\n",
        (1, 1),
    )
    .await;
    assert_eq!(git_files(root), metadata);
    assert_eq!(
        fs::read_link(root.join("link")).unwrap(),
        PathBuf::from("new-target")
    );
}

#[tokio::test]
#[ignore = "requires git-lfs; the required-filter raw endpoint contract is covered without it"]
async fn real_git_lfs_does_not_write_objects_or_replace_text_and_binary_with_pointers() {
    let directory = repository();
    let root = directory.path();
    git(root, &["lfs", "install", "--local"]);
    git(root, &["lfs", "track", "*.lfs"]);
    let before = "keep\nold-a\nold-b\nend\n";
    let after = "keep\nnew-a\nnew-b\nend\n";
    fs::write(root.join("text.lfs"), before).unwrap();
    fs::write(root.join("binary.lfs"), b"before\0bytes").unwrap();
    let metadata = git_files(root);
    let store = Arc::default();
    let record = start(root, &store, "lfs");
    record.before_tools().await;
    assert_eq!(
        git_files(root),
        metadata,
        "LFS must not populate its real object store during observation"
    );
    record.capture(&root.join("text.lfs"));
    fs::write(root.join("text.lfs"), after).unwrap();
    fs::write(root.join("binary.lfs"), b"after\0bytes").unwrap();
    record.finish().await;
    assert_eq!(store.summary(&AttemptId::new("lfs")).unwrap().total, 2);
    text_change(&store, "lfs", "text.lfs", before, after, (2, 2)).await;
    binary_change(&store, "lfs", "binary.lfs").await;
    assert_eq!(git_files(root), metadata);
}

fn nested_repository(root: &Path) -> PathBuf {
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    git(&nested, &["init", "-q"]);
    fs::write(nested.join("inside.txt"), "nested private content\n").unwrap();
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
            "nested initial",
        ],
    );
    nested
}

#[tokio::test]
async fn removing_a_nested_repository_and_its_index_entry_records_the_gitlink_deletion() {
    for tracked in [false, true] {
        let directory = repository();
        let root = directory.path();
        let nested = nested_repository(root);
        if tracked {
            git(root, &["add", "nested"]);
        }
        let store = Arc::default();
        let record = start(root, &store, "remove-nested");
        record.before_tools().await;
        // Native captures from inside the former repository still belong to
        // that nested owner, even when the outer tool deletes its directory.
        record.capture(&nested.join("inside.txt"));
        if tracked {
            git(root, &["rm", "--cached", "-f", "nested"]);
        }
        fs::remove_dir_all(&nested).unwrap();
        let metadata = git_files(root);
        record.finish().await;
        let completed = store
            .0
            .lock()
            .unwrap()
            .get("remove-nested")
            .unwrap()
            .clone();
        assert_eq!(completed.summary.total, 1, "tracked={tracked}");
        assert_eq!(completed.summary.files[0].display, "nested");
        assert_eq!(
            (
                completed.summary.files[0].added,
                completed.summary.files[0].deleted
            ),
            (0, 1)
        );
        let (before, after) = completed.trees.as_ref().unwrap();
        let initial = git(
            record.directory.path(),
            &["ls-tree", "-z", before, "--", "nested"],
        );
        assert!(initial.starts_with("160000 commit "));
        assert!(
            git(
                record.directory.path(),
                &["ls-tree", "-z", after, "--", "nested"]
            )
            .is_empty()
        );
        assert_eq!(git_files(root), metadata);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn replacing_a_nested_repository_samples_the_actual_file_or_symlink_endpoint() {
    use std::os::unix::fs::symlink;
    for remove_from_index in [false, true] {
        for symbolic in [false, true] {
            let directory = repository();
            let root = directory.path();
            let nested = nested_repository(root);
            git(root, &["add", "nested"]);
            let store = Arc::default();
            let record = start(root, &store, "replace-nested");
            record.before_tools().await;
            record.capture(&nested.join("inside.txt"));
            if remove_from_index {
                git(root, &["rm", "--cached", "-f", "nested"]);
            }
            fs::remove_dir_all(&nested).unwrap();
            let (mode, bytes) = if symbolic {
                symlink("replacement-target", &nested).unwrap();
                ("120000", b"replacement-target".as_slice())
            } else {
                fs::write(&nested, "replacement file\n").unwrap();
                ("100644", b"replacement file\n".as_slice())
            };
            let metadata = git_files(root);
            record.finish().await;
            let completed = store
                .0
                .lock()
                .unwrap()
                .get("replace-nested")
                .unwrap()
                .clone();
            assert_eq!(
                completed.summary.total, 1,
                "remove_from_index={remove_from_index}, symbolic={symbolic}"
            );
            assert_eq!(completed.summary.files[0].display, "nested");
            let (before, after) = completed.trees.as_ref().unwrap();
            assert!(
                git(
                    record.directory.path(),
                    &["ls-tree", "-z", before, "--", "nested"]
                )
                .starts_with("160000 commit ")
            );
            let final_entry = git(
                record.directory.path(),
                &["ls-tree", "-z", after, "--", "nested"],
            );
            assert!(
                final_entry.starts_with(&format!("{mode} blob ")),
                "{final_entry}"
            );
            assert_eq!(
                completed
                    .repository
                    .as_ref()
                    .unwrap()
                    .bytes(after, "nested")
                    .await
                    .unwrap(),
                Some(bytes.to_vec())
            );
            assert_eq!(git_files(root), metadata);
        }
    }
}
