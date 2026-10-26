//! Git's implicit global attributes are independent of global Git config.
//! Exercise both discovery locations in separate processes, never `set_env`.
use super::*;

const CHILD_ENV: &str = "RUSTX_WORKSPACE_CHANGES_GLOBAL_ATTRIBUTES_CHILD";
const TEST_NAME: &str = "workspace_changes::tests::global_attributes::default_global_attributes_do_not_override_raw_endpoints";

#[tokio::test]
async fn default_global_attributes_do_not_override_raw_endpoints() {
    if std::env::var_os(CHILD_ENV).is_some() {
        assert_raw_endpoints().await;
        return;
    }

    for location in ["xdg", "home"] {
        let directory = tempdir();
        let home = directory.path().join("home");
        let configuration = if location == "xdg" {
            directory.path().join("xdg")
        } else {
            home.join(".config")
        };
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(configuration.join("git")).unwrap();
        fs::write(
            configuration.join("git/attributes"),
            "*.txt -diff\n*.bin diff\n",
        )
        .unwrap();
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, "1")
            .env("HOME", &home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .kill_on_drop(true);
        if location == "xdg" {
            command.env("XDG_CONFIG_HOME", &configuration);
        } else {
            command.env_remove("XDG_CONFIG_HOME");
        }
        let output = tokio::time::timeout(std::time::Duration::from_secs(60), command.output())
            .await
            .expect("the isolated global-attributes regression must settle")
            .unwrap();
        assert!(
            output.status.success(),
            "{location} global attributes:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "the child must execute the exact regression test"
        );
    }
}

async fn assert_raw_endpoints() {
    let directory = repository();
    let root = directory.path();
    assert_eq!(
        git(
            root,
            &["check-attr", "diff", "--", "text.txt", "binary.bin"]
        ),
        "text.txt: diff: unset\nbinary.bin: diff: set\n",
        "the fixture must exercise Git's implicit global attributes"
    );
    let before_text = "keep\nold-a\nold-b\nend\n";
    let after_text = "keep\nnew-a\nnew-b\nend\n";
    let before_binary = b"old\0bytes";
    let after_binary = b"new\0bytes";
    fs::write(root.join("text.txt"), before_text).unwrap();
    fs::write(root.join("binary.bin"), before_binary).unwrap();
    let store = Arc::default();
    let recording = start(root, &store, "global-attributes");
    recording.before_tools().await;
    // No native-tool captures: the snapshots must also cover shell/child edits.
    fs::write(root.join("text.txt"), after_text).unwrap();
    fs::write(root.join("binary.bin"), after_binary).unwrap();
    recording.finish().await;

    let attempt = AttemptId::new("global-attributes");
    let summary = store.summary(&attempt).unwrap();
    assert_eq!(summary.total, 2);
    assert_eq!((summary.added, summary.deleted), (2, 2));
    for (name, before, after, binary, counts) in [
        (
            "text.txt",
            before_text.as_bytes(),
            after_text.as_bytes(),
            false,
            (2, 2),
        ),
        (
            "binary.bin",
            before_binary.as_slice(),
            after_binary.as_slice(),
            true,
            (0, 0),
        ),
    ] {
        let index = summary
            .files
            .iter()
            .position(|file| file.display == name)
            .unwrap();
        let file = &summary.files[index];
        assert_eq!(file.binary, binary, "{name}");
        assert_eq!((file.added, file.deleted), counts, "{name}");
        let comparison = store.comparison(&attempt, index).await.unwrap().unwrap();
        assert_eq!(comparison.binary, binary, "{name}");
        assert!(!comparison.oversized && !comparison.truncated, "{name}");
        if binary {
            assert!(comparison.hunks.is_empty());
        } else {
            assert_eq!(comparison.hunks.len(), 1);
            assert_eq!(comparison.hunks[0].old_text, before_text);
            assert_eq!(comparison.hunks[0].new_text, after_text);
        }

        let recorded = store
            .0
            .lock()
            .unwrap()
            .get(attempt.as_str())
            .cloned()
            .unwrap();
        let repository = recorded.repository.as_ref().unwrap();
        let (old_tree, new_tree) = recorded.trees.as_ref().unwrap();
        assert_eq!(
            repository.bytes(old_tree, name).await.unwrap(),
            Some(before.to_vec()),
            "{name} must retain its actual original bytes"
        );
        assert_eq!(
            repository.bytes(new_tree, name).await.unwrap(),
            Some(after.to_vec()),
            "{name} must retain its actual final bytes"
        );
    }
}
