//! Issue #398 executable evidence, not a selectable upload backend.
//! The candidate deliberately retains native policy, bootstrap, publication and
//! commit owners. Tests also record where the unwrapped high-level API differs.
#![allow(clippy::too_many_lines)]
use super::*;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, DirBuilder, DirBuilderExt, OpenOptions, OpenOptionsExt};
use nix::fcntl::{FcntlArg, FdFlag, Flock, FlockArg, fcntl};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier};

thread_local! {
    static NATIVE_SYNC_FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}
pub(super) fn native_sync_checkpoint(stage: &str) -> io::Result<()> {
    NATIVE_SYNC_FAULT.with(|fault| {
        if fault.get() == Some(stage) {
            fault.set(None);
            Err(io::Error::other("injected native sync failure"))
        } else {
            Ok(())
        }
    })
}

#[tokio::test]
async fn native_sync_failures_never_commit_ready() {
    use crate::local_runtime::session::SessionPersistentState;
    use crate::local_runtime::session_controller::SessionController;
    for stage in ["file sync", "directory sync"] {
        let product = fixture();
        let workspace = fixture();
        let controller = SessionController::open(product.path()).unwrap();
        let state = SessionPersistentState::from_input(
            &crate::local_runtime::configuration::SessionConfigInput::new(
                workspace.path().to_path_buf(),
            ),
        );
        let id = controller.create_session(state).await.unwrap().session.id;
        NATIVE_SYNC_FAULT.with(|fault| fault.set(Some(stage)));
        assert!(controller.upload(&id, None, inputs()).await.is_err());
        NATIVE_SYNC_FAULT.with(|fault| assert!(fault.get().is_none()));
        let registry = controller
            .catalog
            .lock()
            .await
            .upload_registry(&id)
            .unwrap();
        assert_eq!(registry.allocations.len(), 1);
        let batch = registry.allocations.keys().next().unwrap();
        assert!(!registry.allocations[batch].ready);
        assert!(registry.receipts(&id, batch).is_err());
    }
}

fn fixture() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}
fn session() -> SessionId {
    SessionId::new("ses_84097828-fc31-78c8-8292-10df48901a85")
}
fn inputs() -> Vec<UploadFile> {
    vec![UploadFile {
        name: "payload.txt".into(),
        bytes: vec![42; 4096],
    }]
}
fn mkdir(parent: &Dir, name: &str) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    parent.create_dir_with(name, &builder)
}
fn sync_dir(dir: &Dir) -> io::Result<()> {
    // Linux open_dir uses O_PATH: duplicate/conversion cannot make it syncable.
    // Reopen the same object relative to itself, with read access, without a
    // pathname/ambient handoff. This is additional retained interop cost.
    readable_directory(dir)?.sync_all()
}
fn create(parent: &Dir, name: &str) -> io::Result<cap_std::fs::File> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .mode(0o600)
        .follow(FollowSymlinks::No);
    parent.open_with(name, &options)
}
fn regular(parent: &Dir, name: &str) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .custom_flags(libc::O_NONBLOCK);
    let file = parent.open_with(name, &options)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("not regular"));
    }
    Ok(())
}

/// Receives existing authority; never opens an ambient path. `checkpoint` is a
/// synchronous deterministic seam, not a production backend or global switch.
fn materialize(
    mut workspace: Dir,
    registry: &UploadRegistry,
    id: &SessionId,
    batch: &str,
    files: &[UploadFile],
    mut checkpoint: impl FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    let allocation = &registry.allocations[batch];
    super::super::validate_id(id.as_str(), "upload Session").map_err(io::Error::other)?;
    for name in [".agents", "uploads", id.as_str()] {
        match mkdir(&workspace, name) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
        // Final-only no-follow is sufficient here ONLY because every argument
        // is a single native/validated component. This custom walk remains.
        let child = workspace.open_dir_nofollow(name)?;
        sync_dir(&child)?;
        sync_dir(&workspace)?;
        workspace = child;
    }
    mkdir(&workspace, batch)?;
    sync_dir(&workspace)?;
    let directory = workspace.open_dir_nofollow(batch)?;
    checkpoint("opened")?;
    if allocation.files.len() != files.len() {
        return Err(invalid("batch length"));
    }
    for (entry, input) in allocation.files.iter().zip(files) {
        if entry.name != input.name {
            return Err(invalid("entry mismatch"));
        }
        let mut file = create(&directory, &entry.name)?;
        file.write_all(&input.bytes)?;
        checkpoint("file sync")?;
        file.sync_all()?;
    }
    checkpoint("directory sync")?;
    sync_dir(&directory)?;
    sync_dir(&workspace)?;
    checkpoint("publication")?;
    // Native full-path publication policy is intentionally retained.
    let reopened = session_directory(&allocation.workspace, id, false)?;
    let reopened = directory_at(&reopened, OsStr::new(batch))?;
    let expected = directory.try_clone()?.into_std_file().metadata()?;
    let actual = reopened.metadata()?;
    if (expected.dev(), expected.ino()) != (actual.dev(), actual.ino()) {
        return Err(invalid("upload path changed during materialization"));
    }
    for entry in &allocation.files {
        regular(&directory, &entry.name)?;
    }
    Ok(())
}

#[test]
fn path_policy_and_read_only_inspection() {
    let root = fixture();
    let native = stable_directory(root.path()).unwrap();
    let cap = Dir::from_std_file(native);
    for path in ["../escape", "/absolute", "missing/leaf"] {
        assert!(cap.open(path).is_err());
    }
    for name in ["", ".", "..", "a/b", "a\\b", "CON", "x\0y"] {
        assert!(validate_name(name).is_err());
    }
    assert!(validate_workspace(Path::new("relative")).is_err());
    assert!(validate_workspace(&root.path().join("../bad")).is_err());
    assert!(cap.open_dir("missing").is_err());
    assert!(session_directory(root.path(), &session(), false).is_err());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    create(&cap, "missing-leaf").unwrap();
    assert!(create(&cap, "missing-leaf").is_err());
    // General containment also permits internal '..'; rustX basename policy
    // must not be replaced by cap-std's containment check.
    mkdir(&cap, "sub").unwrap();
    assert!(cap.open("sub/../missing-leaf").is_ok());
}

#[test]
fn full_path_symlinks_distinguish_policy_from_confinement() {
    for location in ["root", "ancestor", "intermediate", "leaf"] {
        for target in ["inside", "outside", "dangling"] {
            let root = fixture();
            let outside = fixture();
            // Different positions in the full declared upload path. The
            // acquired parent capability is above the tested component.
            let suffix = match location {
                "ancestor" => "workspace/.agents/uploads/session/batch",
                "root" => ".agents/uploads/session/batch",
                "intermediate" => "session/batch",
                _ => "",
            };
            std::fs::create_dir_all(root.path().join("real").join(suffix)).unwrap();
            std::fs::create_dir_all(outside.path().join(suffix)).unwrap();
            let destination = match target {
                "inside" => PathBuf::from("real"),
                "outside" => outside.path().to_path_buf(),
                _ => PathBuf::from("absent"),
            };
            symlink(destination, root.path().join("link")).unwrap();
            let relative = if suffix.is_empty() {
                PathBuf::from("link")
            } else {
                PathBuf::from("link").join(suffix)
            };
            let declared = root.path().join(&relative);
            assert!(stable_directory(&declared).is_err(), "{location}/{target}");
            let cap = Dir::from_std_file(stable_directory(root.path()).unwrap());
            assert_eq!(
                cap.open_dir(&relative).is_ok(),
                target == "inside",
                "{location}/{target}"
            );
            // Final-only no-follow does NOT reject an inside symlink ancestor.
            assert_eq!(
                cap.open_dir_nofollow(&relative).is_ok(),
                target == "inside" && !suffix.is_empty()
            );
            assert!(cap.open_dir_nofollow("link").is_err());
            // Trusted root canonicalization permits existing aliases. Upload
            // traversal then uses that canonical spelling, not the raw alias.
            assert_eq!(declared.canonicalize().is_ok(), target != "dangling");
        }
    }
    let root = fixture();
    let cap = Dir::from_std_file(stable_directory(root.path()).unwrap());
    mkdir(&cap, "real").unwrap();
    create(&cap, "real/file").unwrap();
    symlink("real", root.path().join("alias")).unwrap();
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    assert!(cap.open_with("alias/file", &options).is_ok());
    symlink("real/file", root.path().join("leaf")).unwrap();
    assert!(cap.open_with("leaf", &options).is_err());
}

#[test]
fn exclusive_creation_modes_types_and_cloexec() {
    let root = fixture();
    let cap = Arc::new(Dir::from_std_file(stable_directory(root.path()).unwrap()));
    let barrier = Arc::new(Barrier::new(3));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let cap = cap.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                mkdir(&cap, "batch")
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .find_map(|r| r.as_ref().err())
            .unwrap()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    let barrier = Arc::new(Barrier::new(3));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let cap = cap.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                create(&cap, "batch/file")
            })
        })
        .collect();
    barrier.wait();
    let files: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(files.iter().filter(|r| r.is_ok()).count(), 1);
    let file = files.into_iter().find_map(Result::ok).unwrap().into_std();
    assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(
        std::fs::metadata(root.path().join("batch"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(
        FdFlag::from_bits_truncate(fcntl(&file, FcntlArg::F_GETFD).unwrap())
            .contains(FdFlag::FD_CLOEXEC)
    );
    assert!(
        FdFlag::from_bits_truncate(fcntl(cap.as_ref(), FcntlArg::F_GETFD).unwrap())
            .contains(FdFlag::FD_CLOEXEC)
    );
    regular(&cap, "batch/file").unwrap();
    assert!(regular(&cap, "batch").is_err());
    nix::unistd::mkfifo(&root.path().join("fifo"), Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
    assert!(regular(&cap, "fifo").is_err()); // O_NONBLOCK: no writer needed.
    symlink("batch/file", root.path().join("link")).unwrap();
    assert!(regular(&cap, "link").is_err());
    assert!(create(&cap, "link").is_err());
    assert!(mkdir(&cap, "batch").is_err());
    assert_eq!(
        std::fs::metadata(root.path().join("batch/file"))
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn native_commit_failures_and_candidate_substitutions() {
    use crate::local_runtime::session::SessionPersistentState;
    use crate::local_runtime::session_controller::SessionController;
    for scenario in [
        "success",
        "file sync",
        "directory sync",
        "publication",
        "registry",
        "root",
        "ancestor",
        "intermediate",
        "batch",
        "leaf",
    ] {
        let product = fixture();
        let holder = fixture();
        let path = holder.path().join("workspace");
        std::fs::create_dir(&path).unwrap();
        let outside = fixture();
        let controller = SessionController::open(product.path()).unwrap();
        let state = SessionPersistentState::from_input(
            &crate::local_runtime::configuration::SessionConfigInput::new(path.clone()),
        );
        let id = controller.create_session(state).await.unwrap().session.id;
        // Same native admission/claim/commit owners as SessionController::upload.
        let access = controller.acquire_session(&id, None).await.unwrap();
        let workspace = access.settings.cwd.canonicalize().unwrap();
        let files = inputs();
        let mut registry = controller
            .catalog
            .lock()
            .await
            .upload_registry(&id)
            .unwrap();
        let batch = registry.claim(workspace.clone(), &files).unwrap();
        controller
            .catalog
            .lock()
            .await
            .commit_uploads(&id, registry.clone())
            .unwrap();
        let cap = Dir::from_std_file(stable_directory(&workspace).unwrap());
        let mut events = Vec::new();
        let result = materialize(cap, &registry, &id, &batch, &files, |stage| {
            events.push(stage.to_string());
            if stage == scenario {
                return Err(io::Error::other("injected barrier failure"));
            }
            if stage == "opened"
                && ["root", "ancestor", "intermediate", "batch"].contains(&scenario)
            {
                let replaced = match scenario {
                    "root" => workspace.clone(),
                    "ancestor" => workspace.join(".agents"),
                    "intermediate" => workspace.join(".agents/uploads"),
                    _ => workspace
                        .join(".agents/uploads")
                        .join(id.as_str())
                        .join(&batch),
                };
                // Gate entered with retained batch Dir -> rename fully completes
                // -> substitute tree -> return/release -> write through original.
                std::fs::rename(&replaced, holder.path().join("parked")).unwrap();
                std::fs::create_dir_all(
                    workspace
                        .join(".agents/uploads")
                        .join(id.as_str())
                        .join(&batch),
                )
                .unwrap();
            }
            if stage == "publication" && scenario == "leaf" {
                let leaf = file_path(&workspace, &id, &batch, "payload.txt");
                std::fs::remove_file(&leaf).unwrap();
                symlink(outside.path().join("no-write"), leaf).unwrap();
            }
            Ok(())
        });
        let outcome = if result.is_ok() {
            registry.verify_materialized(&id, &batch).unwrap();
            registry.allocations.get_mut(&batch).unwrap().ready = true;
            let mut catalog = controller.catalog.lock().await;
            if scenario == "registry" {
                catalog.arm_write_fault_before_rename();
            }
            catalog
                .commit_uploads(&id, registry.clone())
                .map_err(io::Error::other)
        } else {
            result
        };
        assert_eq!(outcome.is_ok(), scenario == "success", "{scenario}");
        let persisted = controller
            .catalog
            .lock()
            .await
            .upload_registry(&id)
            .unwrap();
        assert_eq!(persisted.allocations[&batch].ready, scenario == "success");
        assert_eq!(
            persisted.receipts(&id, &batch).is_ok(),
            scenario == "success"
        );
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        if ["root", "ancestor", "intermediate", "batch"].contains(&scenario) {
            let suffix = match scenario {
                "root" => PathBuf::from(".agents/uploads")
                    .join(id.as_str())
                    .join(&batch),
                "ancestor" => PathBuf::from("uploads").join(id.as_str()).join(&batch),
                "intermediate" => PathBuf::from(id.as_str()).join(&batch),
                _ => PathBuf::new(),
            };
            assert_eq!(
                std::fs::read(
                    holder
                        .path()
                        .join("parked")
                        .join(suffix)
                        .join("payload.txt")
                )
                .unwrap(),
                files[0].bytes
            );
        }
        if scenario == "success" {
            assert_eq!(
                events,
                ["opened", "file sync", "directory sync", "publication"]
            );
        }
        drop(access); // no directory capability survives native admission.
    }
}

fn readable_directory(cap: &Dir) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY)
        .follow(FollowSymlinks::No);
    Ok(cap.open_with(".", &options)?.into_std())
}
fn locked(cap: &Dir, mode: FlockArg) -> Result<Flock<File>, nix::errno::Errno> {
    // Reopen, never duplicate, for independent flock ownership.
    Flock::lock(readable_directory(cap).unwrap(), mode).map_err(|(_, e)| e)
}
#[test]
#[cfg(target_os = "linux")]
fn default_directory_handles_are_not_sync_or_lock_handles() {
    let root = fixture();
    let cap = Dir::from_std_file(stable_directory(root.path()).unwrap());
    let dir = cap.open_dir(".").unwrap();
    let clone = dir.try_clone().unwrap().into_std_file();
    assert_eq!(
        clone.sync_all().unwrap_err().raw_os_error(),
        Some(libc::EBADF)
    );
    assert_eq!(
        Flock::lock(clone, FlockArg::LockSharedNonblock)
            .unwrap_err()
            .1,
        nix::errno::Errno::EBADF
    );
    sync_dir(&dir).unwrap();
    assert!(locked(&dir, FlockArg::LockSharedNonblock).is_ok());
}
#[test]
fn same_process_locks_and_clone_counterexample() {
    let root = fixture();
    let cap = Dir::from_std_file(stable_directory(root.path()).unwrap());
    for a in [
        FlockArg::LockSharedNonblock,
        FlockArg::LockExclusiveNonblock,
    ] {
        for b in [
            FlockArg::LockSharedNonblock,
            FlockArg::LockExclusiveNonblock,
        ] {
            let guard = locked(&cap, a).unwrap();
            let other = locked(&cap, b);
            assert_eq!(other.is_ok(), a == FlockArg::LockSharedNonblock && b == a);
            if let Err(error) = other.as_ref() {
                assert_eq!(*error, nix::errno::Errno::EWOULDBLOCK);
            }
            drop(guard);
            if other.is_ok() {
                assert!(locked(&cap, FlockArg::LockExclusiveNonblock).is_err());
            }
            drop(other);
            assert!(locked(&cap, FlockArg::LockExclusiveNonblock).is_ok());
        }
    }
    let a = Flock::lock(
        cap.try_clone().unwrap().into_std_file(),
        FlockArg::LockExclusiveNonblock,
    )
    .unwrap();
    let b = Flock::lock(
        cap.try_clone().unwrap().into_std_file(),
        FlockArg::LockExclusiveNonblock,
    )
    .unwrap();
    assert!(locked(&cap, FlockArg::LockExclusiveNonblock).is_err());
    drop(a); // Flock's explicit LOCK_UN unlocks the shared open-file-description.
    assert!(locked(&cap, FlockArg::LockExclusiveNonblock).is_ok());
    drop(b);
}

#[test]
fn lock_child() {
    let Some(path) = std::env::var_os("RUSTX_398_LOCK") else {
        return;
    };
    let cap = Dir::from_std_file(stable_directory(Path::new(&path)).unwrap());
    let mode = if std::env::var("RUSTX_398_MODE").unwrap() == "shared" {
        FlockArg::LockSharedNonblock
    } else {
        FlockArg::LockExclusiveNonblock
    };
    let a = locked(&cap, mode).unwrap();
    let b = if mode == FlockArg::LockSharedNonblock {
        Some(locked(&cap, mode).unwrap())
    } else {
        None
    };
    println!("ACQUIRED");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_exact(&mut [0]).unwrap();
    drop(a);
    println!("DROPPED_A");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_exact(&mut [0]).unwrap();
    drop(b);
}
#[test]
fn cross_process_locks_and_drop() {
    for mode in ["shared", "exclusive"] {
        let root = fixture();
        let cap = Dir::from_std_file(stable_directory(root.path()).unwrap());
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_runtime::session::uploads::cap_std_validation::lock_child",
                "--nocapture",
            ])
            .env("RUSTX_398_LOCK", root.path())
            .env("RUSTX_398_MODE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        wait_line(&mut reader, "ACQUIRED");
        assert_eq!(
            locked(&cap, FlockArg::LockSharedNonblock).is_ok(),
            mode == "shared"
        );
        assert_eq!(
            locked(&cap, FlockArg::LockExclusiveNonblock).unwrap_err(),
            nix::errno::Errno::EWOULDBLOCK
        );
        child.stdin.as_mut().unwrap().write_all(b"a").unwrap();
        wait_line(&mut reader, "DROPPED_A");
        assert_eq!(
            locked(&cap, FlockArg::LockExclusiveNonblock).is_ok(),
            mode == "exclusive"
        );
        child.stdin.as_mut().unwrap().write_all(b"b").unwrap();
        assert!(child.wait().unwrap().success());
        assert!(locked(&cap, FlockArg::LockExclusiveNonblock).is_ok());
    }
}
fn wait_line(reader: &mut impl BufRead, expected: &str) {
    loop {
        let mut line = String::new();
        assert_ne!(
            reader.read_line(&mut line).unwrap(),
            0,
            "child exited before {expected}"
        );
        if line.trim() == expected {
            return;
        }
    }
}

#[test]
fn identity_is_not_lifecycle_authority() {
    use crate::runtime::local_storage::{ConversationAccess, ConversationExclusion, ProductRoot};
    let root = fixture();
    let identity = ProductRoot::existing(root.path()).unwrap();
    let path = root.path().join("allocation");
    std::fs::create_dir(&path).unwrap();
    let cap = Dir::from_std_file(stable_directory(&path).unwrap());
    let exclusion = ConversationExclusion::acquire(&identity, &path).unwrap();
    assert!(ConversationAccess::existing(&identity, &path).is_err());
    assert!(cap.dir_metadata().is_ok()); // capability does NOT enforce admission.
    drop(cap);
    drop(exclusion);
    assert!(ConversationAccess::existing(&identity, &path).is_ok());
    let a = identity.ownership_mutation().unwrap();
    let b = identity.ownership_mutation().unwrap();
    drop(a);
    assert!(identity.freeze_ownership().is_err());
    drop(b);
    assert!(identity.freeze_ownership().is_ok());
}

#[test]
fn same_environment_measurement() {
    // No timing threshold. Alternate order; identical fresh fixtures and bytes.
    // Includes native authorized bootstrap on both sides, excludes claim/commit.
    for repetition in 0..5 {
        for candidate in if repetition % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let root = fixture();
            let id = session();
            let files = inputs();
            let mut registry = UploadRegistry::default();
            let batches: Vec<_> = (0..20)
                .map(|_| registry.claim(root.path().to_path_buf(), &files).unwrap())
                .collect();
            let start = std::time::Instant::now();
            for batch in batches {
                if candidate {
                    let cap = Dir::from_std_file(stable_directory(root.path()).unwrap());
                    materialize(cap, &registry, &id, &batch, &files, |_| Ok(())).unwrap();
                } else {
                    registry.materialize(&id, &batch, &files).unwrap();
                    registry.verify_materialized(&id, &batch).unwrap();
                }
            }
            println!(
                "issue398 repetition={repetition} candidate={candidate} batches=20 bytes_per_batch=4096 elapsed_us={}",
                start.elapsed().as_micros()
            );
        }
    }
}

#[test]
fn leaf_types_match_native_verification() {
    let root = fixture();
    let outside = fixture();
    let files = inputs();
    let id = session();
    let mut registry = UploadRegistry::default();
    let batch = registry.claim(root.path().to_path_buf(), &files).unwrap();
    registry.materialize(&id, &batch, &files).unwrap();
    let leaf = file_path(root.path(), &id, &batch, "payload.txt");
    let directory = leaf.parent().unwrap();
    let cap = Dir::from_std_file(stable_directory(directory).unwrap());
    assert_eq!(
        std::fs::metadata(&leaf).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    std::fs::write(directory.join("inside"), b"keep").unwrap();
    std::fs::write(outside.path().join("outside"), b"keep").unwrap();
    std::fs::remove_file(&leaf).unwrap();
    for target in [
        PathBuf::from("inside"),
        outside.path().join("outside"),
        PathBuf::from("dangling"),
    ] {
        symlink(target, &leaf).unwrap();
        assert!(registry.verify_materialized(&id, &batch).is_err());
        assert!(regular(&cap, "payload.txt").is_err());
        assert!(create(&cap, "payload.txt").is_err());
        std::fs::remove_file(&leaf).unwrap();
    }
    nix::unistd::mkfifo(&leaf, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
    assert!(registry.verify_materialized(&id, &batch).is_err());
    assert!(regular(&cap, "payload.txt").is_err());
    std::fs::remove_file(&leaf).unwrap();
    std::fs::create_dir(&leaf).unwrap();
    assert!(registry.verify_materialized(&id, &batch).is_err());
    assert!(regular(&cap, "payload.txt").is_err());
    assert_eq!(std::fs::read(directory.join("inside")).unwrap(), b"keep");
    assert_eq!(
        std::fs::read(outside.path().join("outside")).unwrap(),
        b"keep"
    );
}

#[test]
fn replacement_before_component_open_fails_closed() {
    for target in ["inside", "outside", "dangling"] {
        let root = fixture();
        let outside = fixture();
        std::fs::create_dir(root.path().join("next")).unwrap();
        let native = stable_directory(root.path()).unwrap();
        let cap = Dir::from_std_file(native.try_clone().unwrap());
        // Acquired parent gate -> next component renamed and symlink installed
        // -> resume native/candidate single-component traversal.
        std::fs::rename(root.path().join("next"), root.path().join("parked")).unwrap();
        let destination = match target {
            "inside" => PathBuf::from("parked"),
            "outside" => outside.path().to_path_buf(),
            _ => PathBuf::from("missing"),
        };
        symlink(destination, root.path().join("next")).unwrap();
        assert!(directory_at(&native, OsStr::new("next")).is_err());
        assert!(cap.open_dir_nofollow("next").is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
