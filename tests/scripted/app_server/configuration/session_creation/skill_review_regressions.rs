//! Real Client bind regressions for shared and absent User Skill collections.

use super::{Fixture, bounded};
use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustx_execution_protocol::skills::{Reply, Request};
use sha2::{Digest as _, Sha256};

const GUIDE: &str = "---\nname: guide\ndescription: Shared guide\n---\nShared instructions.\n";
const TIPS: &str = "---\nname: tips\ndescription: Standalone tips\n---\nStandalone instructions.\n";

fn home(fixture: &Fixture) -> PathBuf {
    fixture
        .manager
        .configuration
        .source_bindings()
        .home_directory
        .clone()
}

fn write_guide(home: &Path) -> PathBuf {
    let package = home.join("rustx/.agents/skills/guide");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("SKILL.md"), GUIDE).unwrap();
    package
}

fn launcher(mount: &Path, bindings: &str) -> PathBuf {
    let binary = std::env::current_exe()
        .unwrap()
        .with_file_name("rustx-execution-client");
    std::os::unix::fs::symlink(binary, mount.join("client-real")).unwrap();
    let path = mount.join("client");
    let script = format!(
        r#"#!/bin/sh
set -eu
bin=${{0%/*}}
unset RUSTX_CLIENT_DOMAIN_STAGING
exec /usr/bin/bwrap --die-with-parent --ro-bind / / {bindings} --dev /dev -- "$bin/client-real" "$@"
"#
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn catalog_paths(fixture: &Fixture) -> BTreeMap<String, PathBuf> {
    let runtime = fixture
        .manager
        .configuration_runtime(&fixture.sessions[0].id)
        .unwrap();
    let resources = runtime.runtime_resources();
    resources
        .capability()
        .skills()
        .catalog_entries()
        .iter()
        .map(|entry| (entry.name.clone(), PathBuf::from(&entry.location)))
        .collect()
}

async fn inquiry(fixture: &Fixture, request: Request) -> Reply {
    let endpoint = fixture
        .manager
        .execution_endpoint(&fixture.sessions[0].id)
        .unwrap();
    tokio::task::spawn_blocking(move || {
        endpoint.user_skills_inquiry(&request, Instant::now() + Duration::from_secs(15))
    })
    .await
    .unwrap()
    .unwrap()
}

/// Restore fixture permissions even if a test assertion fails.
struct ReadOnlyDirectory {
    path: PathBuf,
    original: std::fs::Permissions,
}

impl ReadOnlyDirectory {
    fn new(path: &Path) -> Self {
        let original = std::fs::metadata(path).unwrap().permissions();
        let guard = Self {
            path: path.to_path_buf(),
            original,
        };
        let readonly = std::fs::Permissions::from_mode(guard.original.mode() & !0o222);
        std::fs::set_permissions(path, readonly).unwrap();

        // Verify the Host actually lacks write access; the Client additionally
        // sees a kernel read-only bind mount of the same directory.
        let probe = path.join(".fixture-write-check");
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied),
            Ok(file) => {
                drop(file);
                std::fs::remove_file(probe).unwrap();
                panic!("fixture directory is writable despite read-only permissions");
            }
        }
        guard
    }
}

impl Drop for ReadOnlyDirectory {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.path, self.original.clone());
    }
}

async fn assert_readonly_shared_root(product_readonly: bool) {
    let fixture = Fixture::with_session_count(None, 1).await;
    let home = home(&fixture);
    let product = home.join("rustx");
    let package = write_guide(&home);
    let collection = package.parent().unwrap().to_path_buf();

    if !product_readonly {
        let probe = product.join(".fixture-root-writable");
        std::fs::write(&probe, b"writable").unwrap();
        std::fs::remove_file(probe).unwrap();
    }
    let _collection_mode = ReadOnlyDirectory::new(&collection);
    let _product_mode = product_readonly.then(|| ReadOnlyDirectory::new(&product));
    let root_binding = if product_readonly {
        "--ro-bind"
    } else {
        "--bind"
    };
    let bindings = format!(
        "{root_binding} \"$HOME/rustx\" \"$HOME/rustx\" --ro-bind \"$HOME/rustx/.agents/skills\" \"$HOME/rustx/.agents/skills\""
    );
    let mount = tempfile::tempdir().unwrap();
    let launcher = launcher(mount.path(), &bindings);
    let mut client = fixture.start_execution_via(0, Some(&launcher)).await;
    fixture.load(0).await.unwrap().unwrap();

    let paths = catalog_paths(&fixture);
    assert_eq!(paths.get("guide"), Some(&package.join("SKILL.md")));
    assert!(!collection.join(".rustx-sync").exists());
    assert!(!collection.join(".rustx-sync-lock").exists());
    assert!(!collection.join(".rustx-sync-proof").exists());
    let expected = rustx_execution_protocol::save::Sha256Hex::from_digest(
        Sha256::digest(GUIDE.as_bytes()).into(),
    );
    assert_eq!(
        inquiry(
            &fixture,
            Request::Digest {
                package: PathBuf::from("guide")
            }
        )
        .await,
        Reply::Digest {
            digest: Some(expected)
        },
    );
    if std::env::var_os("RUSTX_PAIRED_RELEASE_PROOF").is_some_and(|value| value == "1") {
        let storage = tempfile::tempdir().unwrap();
        let tools = super::paired_release::NativeTools::new(&fixture, storage.path()).await;
        assert_eq!(tools.read(&package.join("SKILL.md")).await, GUIDE);
    }
    assert_eq!(
        std::fs::read_to_string(package.join("SKILL.md")).unwrap(),
        GUIDE
    );
    fixture.close().await;
    let _ = client.kill().await;
    let _ = client.wait().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real Client mount proof; run test-support/prove_execution_mount_view.sh"]
async fn readonly_shared_collection_and_product_root_are_admitted() {
    assert!(std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some());
    Box::pin(bounded(async {
        assert_readonly_shared_root(false).await;
        assert_readonly_shared_root(true).await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real Client bind proof; run test-support/prove_execution_mount_view.sh"]
async fn standalone_skill_does_not_force_a_shared_package_copy() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let home = home(&fixture);
        let package = write_guide(&home);
        let collection = package.parent().unwrap();
        std::fs::write(collection.join("tips.md"), TIPS).unwrap();
        std::fs::write(package.join("attachment.txt"), "ordinary attachment").unwrap();
        std::os::unix::fs::symlink("attachment.txt", package.join("linked.txt")).unwrap();

        let mut client = fixture.start_execution(0).await;
        fixture.load(0).await.unwrap().unwrap();
        let paths = catalog_paths(&fixture);
        assert_eq!(paths.get("guide"), Some(&package.join("SKILL.md")));
        assert_eq!(paths.get("tips"), Some(&collection.join("tips.md")));
        assert!(!collection.join(".rustx-sync").exists());
        assert!(!collection.join(".rustx-sync-lock").exists());
        assert_eq!(
            std::fs::read_link(package.join("linked.txt")).unwrap(),
            Path::new("attachment.txt")
        );
        assert_eq!(
            std::fs::read_to_string(package.join("linked.txt")).unwrap(),
            "ordinary attachment"
        );
        fixture.close().await;
        let _ = client.kill().await;
        let _ = client.wait().await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real Client mount proof; run test-support/prove_execution_mount_view.sh"]
async fn empty_host_catalog_ignores_a_dangling_client_collection() {
    assert!(std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some());
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let home = home(&fixture);
        let host_collection = home.join("rustx/.agents/skills");
        if host_collection.exists() {
            std::fs::remove_dir(&host_collection).unwrap();
        }
        std::fs::write(
            fixture.workspaces[0].join("still-live.txt"),
            "Client is live\n",
        )
        .unwrap();

        let physical = tempfile::Builder::new()
            .prefix("rustx-empty-client-")
            .tempdir_in("/dev/shm")
            .unwrap();
        assert_ne!(
            std::fs::metadata(&home).unwrap().dev(),
            std::fs::metadata(physical.path()).unwrap().dev()
        );
        let agents = physical.path().join("rustx/.agents");
        std::fs::create_dir_all(&agents).unwrap();
        let dangling = agents.join("skills");
        std::os::unix::fs::symlink("unmounted-collection", &dangling).unwrap();
        let before = std::fs::symlink_metadata(&dangling).unwrap();
        let mount = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(physical.path(), mount.path().join("client-home")).unwrap();
        let launcher = launcher(mount.path(), "--bind \"$bin/client-home\" \"$HOME\"");

        let mut client = fixture.start_execution_via(0, Some(&launcher)).await;
        fixture.load(0).await.unwrap().unwrap();
        assert!(catalog_paths(&fixture).is_empty());
        assert!(matches!(
            inquiry(&fixture, Request::Root { probe: None }).await,
            Reply::Root { .. }
        ));
        let endpoint = fixture
            .manager
            .execution_endpoint(&fixture.sessions[0].id)
            .unwrap();
        let (_, page) = tokio::task::spawn_blocking(move || {
            endpoint.read_lines(Path::new("still-live.txt"), 1, None, None)
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(page.text, "Client is live\n");
        let after = std::fs::symlink_metadata(&dangling).unwrap();
        assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
        assert_eq!(
            std::fs::read_link(&dangling).unwrap(),
            Path::new("unmounted-collection")
        );
        assert!(!agents.join("unmounted-collection").exists());
        assert!(!host_collection.exists());
        fixture.close().await;
        let _ = client.kill().await;
        let _ = client.wait().await;
    }))
    .await;
}
