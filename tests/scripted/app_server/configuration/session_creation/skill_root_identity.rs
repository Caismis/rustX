//! Real Client mount views and local User-root alias handling.

use super::*;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

fn write_skill(home: &Path) -> PathBuf {
    let package = home.join("rustx/.agents/skills/guide");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("SKILL.md"),
        "---\nname: guide\ndescription: Host guide\n---\nHost instructions\n",
    )
    .unwrap();
    std::fs::write(package.join("attachment"), "Host attachment").unwrap();
    package
}

fn launcher(
    mount: &Path,
    source: &Path,
    target_suffix: &str,
    cache: Option<(&Path, &Path)>,
) -> PathBuf {
    let binary = std::env::current_exe()
        .unwrap()
        .with_file_name("rustx-execution-client");
    std::os::unix::fs::symlink(binary, mount.join("client-real")).unwrap();
    std::os::unix::fs::symlink(source, mount.join("overlay")).unwrap();
    let cache_mounts = if let Some((cache, lock)) = cache {
        std::os::unix::fs::symlink(cache, mount.join("cache")).unwrap();
        std::os::unix::fs::symlink(lock, mount.join("lock")).unwrap();
        "--bind \"$bin/cache\" \"$HOME/rustx/.agents/skills/.rustx-sync\" --bind \"$bin/lock\" \"$HOME/rustx/.agents/skills/.rustx-sync-lock\""
    } else {
        ""
    };
    let path = mount.join("client");
    let script = format!(
        r#"#!/bin/sh
set -eu
bin=${{0%/*}}
unset RUSTX_CLIENT_DOMAIN_STAGING
exec /usr/bin/bwrap --die-with-parent --ro-bind / / --bind "$bin/overlay" "$HOME/{target_suffix}" {cache_mounts} --dev /dev -- "$bin/client-real" "$@"
"#
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn catalog_path(fixture: &Fixture) -> PathBuf {
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
        .find(|entry| entry.name == "guide")
        .unwrap()
        .location
        .clone()
        .into()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn symlinked_home_with_no_collection_still_binds() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_user_root_alias(false).await;
        let physical = fixture
            .manager
            .configuration
            .source_bindings()
            .home_directory
            .clone();
        assert!(
            std::fs::symlink_metadata(physical.with_file_name("home"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let collection = physical.join("rustx/.agents/skills");
        if collection.exists() {
            std::fs::remove_dir(&collection).unwrap();
        }
        assert!(!collection.exists());
        fixture
            .manager
            .configure_local_sessions(fixture.workspaces.to_vec())
            .unwrap();
        let created = fixture
            .manager
            .create_session(
                crate::local_runtime::session::SessionPersistentState::from_input(
                    &crate::local_runtime::configuration::SessionConfigInput::new(
                        fixture.workspaces[0].clone(),
                    ),
                ),
            )
            .await
            .unwrap();
        assert!(
            fixture
                .manager
                .execution_endpoint(&created.session.id)
                .unwrap()
                .control()
                .is_some()
        );
        fixture
            .manager
            .load(&created.session.id, None)
            .await
            .unwrap();
        let runtime = fixture
            .manager
            .configuration_runtime(&created.session.id)
            .unwrap();
        assert_eq!(
            runtime
                .runtime_resources()
                .capability()
                .skills()
                .catalog_entries(),
            []
        );
        assert_eq!(fixture.host.drain().await, [] as [String; 0]);
        fixture.manager.join_local_sessions().await.unwrap();
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn symlinked_product_root_publishes_a_readable_physical_path() {
    Box::pin(bounded(async {
        let fixture = Fixture::with_user_root_alias(true).await;
        let home = fixture
            .manager
            .configuration
            .source_bindings()
            .home_directory
            .clone();
        let product = home.join("rustx");
        let physical = home.join("user-product");
        assert!(
            std::fs::symlink_metadata(&product)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let package = write_skill(&home);
        let mut client = fixture.start_execution(0).await;
        fixture.load(0).await.unwrap().unwrap();
        assert_eq!(
            catalog_path(&fixture),
            physical.join(".agents/skills/guide/SKILL.md")
        );
        fixture.close().await;
        let _ = client.kill().await;
        assert_eq!(
            std::fs::read_to_string(package.join("attachment")).unwrap(),
            "Host attachment"
        );
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in bwrap proof; run test-support/prove_execution_mount_view.sh"]
async fn collection_overlay_is_copied_even_when_product_root_is_shared() {
    assert!(std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some());
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let home = fixture
            .manager
            .configuration
            .source_bindings()
            .home_directory
            .clone();
        let package = write_skill(&home);
        let physical = tempfile::Builder::new()
            .prefix("rustx-skill-overlay-")
            .tempdir_in("/dev/shm")
            .unwrap();
        let mount = tempfile::tempdir().unwrap();
        let launcher = launcher(mount.path(), physical.path(), "rustx/.agents/skills", None);
        let mut client = fixture.start_execution_via(0, Some(&launcher)).await;
        fixture.load(0).await.unwrap().unwrap();
        let location = catalog_path(&fixture);
        let client_collection = home.join("rustx/.agents/skills");
        let relative = location.strip_prefix(&client_collection).unwrap();
        assert!(relative.starts_with(".rustx-sync"));
        let copied = physical.path().join(relative);
        assert!(
            std::fs::read_to_string(&copied)
                .unwrap()
                .contains("Host instructions")
        );
        assert_eq!(
            std::fs::read_to_string(copied.parent().unwrap().join("attachment")).unwrap(),
            "Host attachment"
        );
        assert!(!client_collection.join(".rustx-sync").exists());
        fixture.close().await;
        let _ = client.kill().await;
        assert!(package.join("SKILL.md").is_file());
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in bwrap proof; run test-support/prove_execution_mount_view.sh"]
async fn package_overlay_is_copied_even_when_collection_is_shared() {
    assert!(std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some());
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let home = fixture
            .manager
            .configuration
            .source_bindings()
            .home_directory
            .clone();
        let package = write_skill(&home);
        let physical = tempfile::Builder::new()
            .prefix("rustx-package-overlay-")
            .tempdir_in("/dev/shm")
            .unwrap();
        std::fs::copy(package.join("SKILL.md"), physical.path().join("SKILL.md")).unwrap();
        std::fs::write(physical.path().join("attachment"), "Client attachment").unwrap();
        let collection = home.join("rustx/.agents/skills");
        std::fs::create_dir(collection.join(".rustx-sync")).unwrap();
        std::fs::write(collection.join(".rustx-sync-lock"), []).unwrap();
        let cache = tempfile::Builder::new()
            .prefix("rustx-cache-overlay-")
            .tempdir_in("/dev/shm")
            .unwrap();
        std::fs::write(
            cache.path().join(".rustx-owned"),
            b"rustx user skill cache v1\n",
        )
        .unwrap();
        let lock = tempfile::NamedTempFile::new_in("/dev/shm").unwrap();
        let mount = tempfile::tempdir().unwrap();
        let launcher = launcher(
            mount.path(),
            physical.path(),
            "rustx/.agents/skills/guide",
            Some((cache.path(), lock.path())),
        );
        let mut client = fixture.start_execution_via(0, Some(&launcher)).await;
        fixture.load(0).await.unwrap().unwrap();
        let location = catalog_path(&fixture);
        assert!(location.starts_with(home.join("rustx/.agents/skills/.rustx-sync")));
        let relative = location
            .strip_prefix(collection.join(".rustx-sync"))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(
                cache
                    .path()
                    .join(relative)
                    .parent()
                    .unwrap()
                    .join("attachment")
            )
            .unwrap(),
            "Host attachment"
        );
        assert_eq!(
            std::fs::read_to_string(physical.path().join("attachment")).unwrap(),
            "Client attachment"
        );
        fixture.close().await;
        let _ = client.kill().await;
        assert_eq!(
            std::fs::read_to_string(package.join("attachment")).unwrap(),
            "Host attachment"
        );
    }))
    .await;
}
