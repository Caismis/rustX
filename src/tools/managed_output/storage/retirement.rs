//! Durable retirement into a product-owned directory on the source filesystem.
//!
//! The shared arena and its marker outlive Session deletion. A frozen slot is
//! addressed by the permanently reserved Conversation identity and namespace
//! ordinal, never by scanning outputs. Only a confirmed move grants authority
//! to finish deleting a markerless slot after process death.
use super::{
    Binding, MARKER, Marker, Ownership, RETRIES, directory, directory_at, invalid, lock,
    random_name, read, safe_hex, write_with_sync,
};
use crate::runtime::identity::ConversationId;
use nix::fcntl::{AtFlags, Flock, FlockArg, OFlag, openat};
use nix::sys::stat::{Mode, fstatat, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use rustix::fs::{RenameFlags, renameat_with};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Arena {
    root: PathBuf,
    device: u64,
    inode: u64,
    token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Retirement {
    arena: Arena,
    confirmed: bool,
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArenaMarker {
    product: PathBuf,
    token: String,
}

fn private_directory(dir: &File, device: u64, inode: u64) -> io::Result<()> {
    let metadata = dir.metadata()?;
    if metadata.dev() != device
        || metadata.ino() != inode
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(invalid());
    }
    Ok(())
}

fn marker<T: serde::de::DeserializeOwned>(dir: &File) -> io::Result<T> {
    let file = File::from(openat(
        dir,
        MARKER,
        OFlag::O_RDONLY | OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?);
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    serde_json::from_reader(file.take(8192)).map_err(io::Error::other)
}

impl Arena {
    fn open(&self, product: &Path, source_parent: &Path) -> io::Result<File> {
        if self.root.parent() != Some(source_parent)
            || !self
                .root
                .file_name()
                .and_then(OsStr::to_str)
                .and_then(|s| s.strip_prefix(".rx-retired-"))
                .is_some_and(safe_hex)
            || !safe_hex(&self.token)
        {
            return Err(invalid());
        }
        let dir = directory(&self.root)?;
        private_directory(&dir, self.device, self.inode)?;
        if marker::<ArenaMarker>(&dir).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                invalid()
            } else {
                e
            }
        })? != (ArenaMarker {
            product: product.to_owned(),
            token: self.token.clone(),
        }) {
            return Err(invalid());
        }
        Ok(dir)
    }

    fn acquire(product: &Path, source_parent: &Path) -> io::Result<Self> {
        let product_dir = directory(product)?;
        match mkdirat(
            &product_dir,
            "output-retirement",
            Mode::from_bits_truncate(0o700),
        ) {
            Ok(()) => product_dir.sync_all()?,
            Err(nix::errno::Errno::EEXIST) => {}
            Err(e) => return Err(e.into()),
        }
        let control = directory_at(&product_dir, OsStr::new("output-retirement"))?;
        let metadata = control.metadata()?;
        private_directory(&control, metadata.dev(), metadata.ino())?;
        let control_path = product.join("output-retirement");
        let lock_file = File::from(openat(
            &control,
            "lock",
            OFlag::O_RDWR
                | OFlag::O_CREAT
                | OFlag::O_NONBLOCK
                | OFlag::O_NOFOLLOW
                | OFlag::O_CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )?);
        if !lock_file.metadata()?.is_file() {
            return Err(invalid());
        }
        let _lock =
            Flock::lock(lock_file, FlockArg::LockExclusive).map_err(|(_, e)| io::Error::from(e))?;
        let record = format!(
            "{:x}.json",
            Sha256::digest(source_parent.as_os_str().as_bytes())
        );
        match openat(
            &control,
            record.as_str(),
            OFlag::O_RDONLY | OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let file = File::from(fd);
                if !file.metadata()?.is_file() {
                    return Err(invalid());
                }
                let arena: Self = serde_json::from_reader(file).map_err(io::Error::other)?;
                if arena.open(product, source_parent).is_ok() {
                    return Ok(arena);
                }
                // An externally lost arena is never repaired. Pending worksets
                // retain their own binding; future retirements get a fresh one.
            }
            Err(nix::errno::Errno::ENOENT) => {}
            Err(e) => return Err(e.into()),
        }
        Self::create(product, source_parent, &control, &control_path, &record)
    }

    fn create(
        product: &Path,
        source_parent: &Path,
        control: &File,
        control_path: &Path,
        record: &str,
    ) -> io::Result<Self> {
        let parent = directory(source_parent)?;
        for _ in 0..RETRIES {
            let leaf = format!(".rx-retired-{}", random_name()?);
            match mkdirat(&parent, leaf.as_str(), Mode::from_bits_truncate(0o700)) {
                Ok(()) => {}
                Err(nix::errno::Errno::EEXIST) => continue,
                Err(e) => return Err(e.into()),
            }
            let dir = directory_at(&parent, OsStr::new(&leaf))?;
            let metadata = dir.metadata()?;
            let arena = Self {
                root: source_parent.join(leaf),
                device: metadata.dev(),
                inode: metadata.ino(),
                token: random_name()?,
            };
            let mut proof = File::from(openat(
                &dir,
                MARKER,
                OFlag::O_WRONLY
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )?);
            serde_json::to_writer(
                &mut proof,
                &ArenaMarker {
                    product: product.to_owned(),
                    token: arena.token.clone(),
                },
            )
            .map_err(io::Error::other)?;
            proof.sync_all()?;
            dir.sync_all()?;
            parent.sync_all()?;
            let mut file = tempfile::NamedTempFile::new_in(control_path)?;
            serde_json::to_writer(file.as_file_mut(), &arena).map_err(io::Error::other)?;
            file.as_file().sync_all()?;
            file.persist(control_path.join(record))
                .map_err(|e| e.error)?;
            control.sync_all()?;
            return Ok(arena);
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary output retirement arena collision budget exhausted",
        ))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Point {
    BeforeMove,
    AfterRenameBeforeValidation,
    AfterValidation,
    AfterMarkerRemoval,
}

fn missing<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[allow(clippy::too_many_lines)] // Keep durable retirement transitions in one ordered transaction.
pub(super) fn cleanup(
    product: &Path,
    owner: &Path,
    conversation: &ConversationId,
    hook: &mut dyn FnMut(Point, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let product = product.canonicalize()?;
    if !owner.starts_with(&product) || owner == product {
        return Err(invalid());
    }
    let Some(_) = missing(directory(owner))? else {
        return Ok(());
    };
    let _lock = lock(owner)?;
    let mut ownership = read(owner, conversation)?;
    if !ownership.namespaces.is_empty() {
        // A previous persist may have become visible before its directory sync
        // failed. Reconfirm recovered authority before any destructive effect.
        write_with_sync(&ownership, true)?;
    }
    for index in 0..ownership.namespaces.len() {
        let mut binding = ownership.namespaces[index].clone();
        let source_parent = binding.root.parent().ok_or_else(invalid)?;
        let slot = format!("{}-n{index}", conversation.as_str());
        if !crate::runtime::subagent::is_safe_child_conversation_component(conversation) {
            return Err(invalid());
        }
        if binding.retirement.is_none() {
            let Some(_) = missing(binding.open(owner, conversation))? else {
                continue;
            };
            let arena = Arena::acquire(&product, source_parent)?;
            binding.retirement = Some(Retirement {
                arena,
                confirmed: false,
            });
            ownership.namespaces[index] = binding.clone();
            write_with_sync(&ownership, true)?;
        }
        let prepared = binding.retirement.as_ref().expect("retirement prepared");
        let arena = match missing(prepared.arena.open(&product, source_parent))? {
            Some(dir) => dir,
            None if prepared.confirmed => continue,
            None => {
                if missing(binding.open(owner, conversation))?.is_none() {
                    // Both known locations are gone. A foreign replacement or
                    // missing marker at either existing location still fails.
                    if let Some(parent) = missing(directory(source_parent))? {
                        parent.sync_all()?;
                    }
                    ownership.namespaces[index]
                        .retirement
                        .as_mut()
                        .unwrap()
                        .confirmed = true;
                    write_with_sync(&ownership, true)?;
                    continue;
                }
                let fresh = Arena::acquire(&product, source_parent)?;
                let dir = fresh.open(&product, source_parent)?;
                binding.retirement = Some(Retirement {
                    arena: fresh,
                    confirmed: false,
                });
                ownership.namespaces[index] = binding.clone();
                write_with_sync(&ownership, true)?;
                dir
            }
        };
        let retirement = binding.retirement.as_ref().expect("retirement prepared");
        if !retirement.confirmed {
            let moved = match inspect_unconfirmed_slot(&binding, &ownership, &arena, &slot)? {
                Some(moved) => Some(moved),
                None => move_namespace(&binding, &ownership, &arena, &slot, hook)?,
            };
            let Some(moved) = moved else {
                // Neither source nor reserved slot exists: settle any earlier
                // interrupted rename before confirming absence.
                directory(source_parent)?.sync_all()?;
                arena.sync_all()?;
                ownership.namespaces[index]
                    .retirement
                    .as_mut()
                    .unwrap()
                    .confirmed = true;
                write_with_sync(&ownership, true)?;
                continue;
            };
            // This also covers restart after rename but before either sync.
            directory(source_parent)?.sync_all()?;
            arena.sync_all()?;
            ownership.namespaces[index]
                .retirement
                .as_mut()
                .unwrap()
                .confirmed = true;
            write_with_sync(&ownership, true)?;
            drop(moved);
        }
        remove_slot(&arena, &binding, owner, conversation, &slot, hook)?;
    }
    Ok(())
}

/// A reserved slot may contain any directory entry captured at the public name.
/// Live cleanup and restart recovery must return every unverified entry, even
/// when opening it as a directory or reading its marker fails.
fn inspect_unconfirmed_slot(
    binding: &Binding,
    ownership: &Ownership,
    arena: &File,
    slot: &str,
) -> io::Result<Option<File>> {
    let validation = (|| -> io::Result<Option<File>> {
        match fstatat(arena, slot, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(_) => {}
            Err(nix::errno::Errno::ENOENT) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let moved = directory_at(arena, OsStr::new(slot))?;
        private_directory(&moved, binding.device, binding.inode)?;
        if marker::<Marker>(&moved)? != binding.marker(&ownership.owner, &ownership.conversation) {
            return Err(invalid());
        }
        Ok(Some(moved))
    })();
    validation.map_err(|error| restore_unconfirmed_entry(binding, arena, slot, &error))
}

fn restore_unconfirmed_entry(
    binding: &Binding,
    arena: &File,
    slot: &str,
    validation: &io::Error,
) -> io::Error {
    let restoration = (|| -> io::Result<()> {
        let parent = directory(binding.root.parent().ok_or_else(invalid)?)?;
        renameat_with(
            arena,
            slot,
            &parent,
            binding.root.file_name().ok_or_else(invalid)?,
            RenameFlags::NOREPLACE,
        )?;
        parent.sync_all()?;
        arena.sync_all()?;
        Ok(())
    })();
    let recovery = match restoration {
        Ok(()) => format!("entry restored to {}", binding.root.display()),
        Err(error) => format!(
            "restoration to {} did not complete: {error}",
            binding.root.display(),
        ),
    };
    // A successful reverse move restores location, not ownership authority.
    // Keep the reserved intent and report pending until cleanup can revalidate.
    io::Error::other(format!(
        "unconfirmed temporary output entry failed validation: {validation}; {recovery}; cleanup remains pending",
    ))
}

fn move_namespace(
    binding: &Binding,
    ownership: &Ownership,
    arena: &File,
    slot: &str,
    hook: &mut dyn FnMut(Point, &Path) -> io::Result<()>,
) -> io::Result<Option<File>> {
    let Some(original) = missing(binding.open(&ownership.owner, &ownership.conversation))? else {
        return Ok(None);
    };
    let source_parent = binding.root.parent().ok_or_else(invalid)?;
    let source_leaf = binding.root.file_name().ok_or_else(invalid)?;
    let parent = directory(source_parent)?;
    hook(Point::BeforeMove, &binding.root)?;
    renameat_with(&parent, source_leaf, arena, slot, RenameFlags::NOREPLACE)?;
    let retired_path = binding.retirement.as_ref().unwrap().arena.root.join(slot);
    hook(Point::AfterRenameBeforeValidation, &retired_path)?;
    let moved = inspect_unconfirmed_slot(binding, ownership, arena, slot)?
        .ok_or_else(|| io::Error::other("temporary output entry disappeared before validation"))?;
    // Keep the original inode pinned through post-rename validation, preventing
    // reuse while the public name can be substituted before the move.
    drop(original);
    hook(Point::AfterValidation, &retired_path)?;
    Ok(Some(moved))
}

fn remove_slot(
    arena: &File,
    binding: &Binding,
    owner: &Path,
    conversation: &ConversationId,
    slot: &str,
    hook: &mut dyn FnMut(Point, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let Some(dir) = missing(directory_at(arena, OsStr::new(slot)))? else {
        // A preceding attempt may have unlinked the slot and then failed
        // its final durability barrier. Retry that barrier before success.
        arena.sync_all()?;
        return Ok(());
    };
    private_directory(&dir, binding.device, binding.inode)?;
    // Only the durable confirmed state and the still-proven private arena
    // allow this exact reserved slot to be markerless during recovery.
    if let Some(proof) = missing(marker::<Marker>(&dir))?
        && proof != binding.marker(owner, conversation)
    {
        return Err(invalid());
    }
    let mut entries = nix::dir::Dir::from_fd(dir.try_clone()?.into())?;
    let names = entries
        .iter()
        .map(|e| e.map(|e| e.file_name().to_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    for name in names {
        if [b".".as_slice(), b"..", MARKER.as_bytes()].contains(&name.as_bytes()) {
            continue;
        }
        match unlinkat(
            &dir,
            OsStr::from_bytes(name.as_bytes()),
            UnlinkatFlags::NoRemoveDir,
        ) {
            Ok(()) | Err(nix::errno::Errno::ENOENT) => {}
            Err(e) => return Err(e.into()),
        }
    }
    match unlinkat(&dir, MARKER, UnlinkatFlags::NoRemoveDir) {
        Ok(()) | Err(nix::errno::Errno::ENOENT) => {}
        Err(e) => return Err(e.into()),
    }
    hook(
        Point::AfterMarkerRemoval,
        &binding.retirement.as_ref().unwrap().arena.root.join(slot),
    )?;
    match unlinkat(arena, slot, UnlinkatFlags::RemoveDir) {
        Ok(()) | Err(nix::errno::Errno::ENOENT) => {}
        Err(e) => return Err(e.into()),
    }
    arena.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{BINDINGS, Namespaces};
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::{FileTypeExt, PermissionsExt, symlink};
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};
    const ID: &str = "conv_01900000-0000-7000-8000-000000000001";
    const CHILD: &str = "tools::managed_output::storage::retirement::tests::cleanup_process";

    fn fixture(root: &Path) -> (PathBuf, ConversationId, PathBuf) {
        let owner = root.join("private");
        let temporary = root.join("temporary");
        std::fs::create_dir(&temporary).unwrap();
        let id = ConversationId::new(ID);
        let mut namespaces = Namespaces::new(&owner, &id, &temporary).unwrap();
        let (mut file, path) = namespaces.create("r", ".txt").unwrap();
        file.write_all(b"original output").unwrap();
        (
            owner.canonicalize().unwrap(),
            id,
            path.parent().unwrap().to_owned(),
        )
    }

    fn command(root: &Path, mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env(
                "RUSTX_OUTPUT_RETIREMENT_TEST_ROOT",
                root.canonicalize().unwrap(),
            )
            .env("RUSTX_OUTPUT_RETIREMENT_TEST_MODE", mode);
        command
    }

    fn run_cleanup(root: &Path, mode: &str) {
        let output = command(root, mode).output().unwrap();
        assert!(
            output.status.success(),
            "cleanup child failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn kill_at_frontier(root: &Path, mode: &str) {
        use std::io::BufRead;
        let mut child = command(root, mode).stdout(Stdio::piped()).spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in io::BufReader::new(stdout).lines() {
                if line.unwrap().contains("RETIREMENT_FRONTIER") {
                    tx.send(()).unwrap();
                    return;
                }
            }
        });
        let frontier = rx.recv_timeout(std::time::Duration::from_secs(10));
        let killed = child.kill();
        let status = child.wait().unwrap();
        reader.join().unwrap();
        frontier.unwrap();
        killed.unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }

    fn reserved_slot(owner: &Path, id: &ConversationId) -> PathBuf {
        let ownership = read(owner, id).unwrap();
        let retirement = ownership.namespaces[0].retirement.as_ref().unwrap();
        assert!(!retirement.confirmed);
        retirement.arena.root.join(format!("{ID}-n0"))
    }

    fn replacement(root: &Path, kind: &str) -> PathBuf {
        let path = root.join("replacement");
        match kind {
            "directory" => {
                std::fs::create_dir(&path).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
                std::fs::write(path.join("payload"), "foreign payload").unwrap();
            }
            "file" => std::fs::write(&path, "foreign payload").unwrap(),
            "symlink" | "dangling-symlink" => {
                let target = root.join("symlink-target");
                if kind == "symlink" {
                    std::fs::write(&target, "protected target").unwrap();
                }
                symlink(target, &path).unwrap();
            }
            "fifo" => nix::unistd::mkfifo(&path, Mode::S_IRUSR | Mode::S_IWUSR).unwrap(),
            _ => panic!("unknown replacement type"),
        }
        path
    }

    fn assert_replacement(path: &Path, expected: &std::fs::Metadata, root: &Path, kind: &str) {
        let actual = std::fs::symlink_metadata(path).unwrap();
        assert_eq!(
            (actual.dev(), actual.ino(), actual.mode()),
            (expected.dev(), expected.ino(), expected.mode()),
            "{kind}"
        );
        match kind {
            "directory" => assert_eq!(
                std::fs::read_to_string(path.join("payload")).unwrap(),
                "foreign payload"
            ),
            "file" => assert_eq!(std::fs::read_to_string(path).unwrap(), "foreign payload"),
            "symlink" | "dangling-symlink" => {
                assert_eq!(
                    std::fs::read_link(path).unwrap(),
                    root.join("symlink-target")
                );
                if kind == "symlink" {
                    assert_eq!(
                        std::fs::read_to_string(root.join("symlink-target")).unwrap(),
                        "protected target"
                    );
                } else {
                    assert_missing(&root.join("symlink-target"));
                }
            }
            "fifo" => assert!(actual.file_type().is_fifo()),
            _ => unreachable!(),
        }
    }

    fn remove_replacement(path: &Path, kind: &str) {
        if kind == "directory" {
            std::fs::remove_dir_all(path).unwrap();
        } else {
            std::fs::remove_file(path).unwrap();
        }
    }

    fn assert_missing(path: &Path) {
        assert_eq!(
            std::fs::symlink_metadata(path).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    fn assert_original_payload(path: &Path) {
        let files: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.file_name().to_str().unwrap().starts_with("r-"))
            .collect();
        assert_eq!(files.len(), 1);
        assert_eq!(
            std::fs::read_to_string(files[0].path()).unwrap(),
            "original output"
        );
    }

    #[test]
    #[ignore = "fixture subprocess, invoked by its parent test"]
    fn cleanup_process() {
        let root = std::env::var_os("RUSTX_OUTPUT_RETIREMENT_TEST_ROOT")
            .expect("fixture parent must provide its environment");
        let root = PathBuf::from(root);
        let mode = std::env::var("RUSTX_OUTPUT_RETIREMENT_TEST_MODE").unwrap();
        let result = cleanup(
            &root,
            &root.join("private"),
            &ConversationId::new(ID),
            &mut |point, path| {
                if mode == "substitute" && point == Point::BeforeMove {
                    std::fs::rename(path, path.with_extension("moved"))?;
                    std::fs::rename(root.join("replacement"), path)?;
                }
                if (mode == "stop" && point == Point::AfterMarkerRemoval)
                    || (mode == "validated" && point == Point::AfterValidation)
                    || (mode == "substitute" && point == Point::AfterRenameBeforeValidation)
                {
                    println!("RETIREMENT_FRONTIER");
                    io::stdout().flush()?;
                    loop {
                        std::thread::park();
                    }
                }
                Ok(())
            },
        );
        if mode == "restore" {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("cleanup remains pending")
            );
        } else {
            result.unwrap();
        }
    }

    #[test]
    fn process_death_after_validation_or_marker_unlink_preserves_public_replacement() {
        for mode in ["validated", "stop"] {
            let root = tempfile::tempdir().unwrap();
            let (owner, id, public) = fixture(root.path());
            kill_at_frontier(root.path(), mode);
            let ownership = read(&owner, &id).unwrap();
            let retired = ownership.namespaces[0].retirement.as_ref().unwrap();
            assert_eq!(retired.confirmed, mode == "stop");
            let slot = retired.arena.root.join(format!("{ID}-n0"));
            assert!(slot.is_dir());
            assert_eq!(slot.join(MARKER).exists(), mode == "validated");
            assert!(retired.arena.root.join(MARKER).is_file());
            std::fs::create_dir(&public).unwrap();
            run_cleanup(root.path(), "finish");
            assert_missing(&slot);
            assert!(public.is_dir());
            assert!(retired.arena.root.join(MARKER).is_file());
            super::super::cleanup(root.path(), &owner, &id).unwrap();
            assert!(public.is_dir());
        }
    }

    #[test]
    fn public_substitution_after_validation_is_preserved_and_original_remains_owned() {
        for kind in ["directory", "file", "symlink", "dangling-symlink", "fifo"] {
            let root = tempfile::tempdir().unwrap();
            let (owner, id, public) = fixture(root.path());
            let foreign = replacement(root.path(), kind);
            let expected = std::fs::symlink_metadata(&foreign).unwrap();
            let original = public.with_extension("moved");
            let result = cleanup(root.path(), &owner, &id, &mut |point, path| {
                if point == Point::BeforeMove {
                    std::fs::rename(path, &original)?;
                    std::fs::rename(&foreign, path)?;
                }
                Ok(())
            });
            let error = result.unwrap_err().to_string();
            assert!(error.contains("entry restored"), "{error}");
            assert_replacement(&public, &expected, root.path(), kind);
            assert_missing(&reserved_slot(&owner, &id));
            assert_original_payload(&original);
            assert!(super::super::cleanup(root.path(), &owner, &id).is_err());
            assert_replacement(&public, &expected, root.path(), kind);
            remove_replacement(&public, kind);
            std::fs::rename(original, &public).unwrap();
            super::super::cleanup(root.path(), &owner, &id).unwrap();
            assert!(!public.exists());
        }
    }

    #[test]
    fn every_marker_or_identity_validation_failure_restores_the_unconfirmed_entry() {
        for fault in ["missing", "malformed", "fifo", "mode"] {
            let root = tempfile::tempdir().unwrap();
            let (owner, id, public) = fixture(root.path());
            let expected = std::fs::metadata(&public).unwrap();
            let proof = std::fs::read(public.join(MARKER)).unwrap();
            let result = cleanup(root.path(), &owner, &id, &mut |point, path| {
                if point == Point::AfterRenameBeforeValidation {
                    match fault {
                        "missing" => std::fs::remove_file(path.join(MARKER))?,
                        "malformed" => std::fs::write(path.join(MARKER), "invalid JSON")?,
                        "fifo" => {
                            std::fs::remove_file(path.join(MARKER))?;
                            nix::unistd::mkfifo(&path.join(MARKER), Mode::S_IRUSR | Mode::S_IWUSR)?;
                        }
                        "mode" => {
                            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o750))?;
                        }
                        _ => unreachable!(),
                    }
                }
                Ok(())
            });
            let error = result.unwrap_err().to_string();
            assert!(error.contains("entry restored"), "{fault}: {error}");
            let actual = std::fs::metadata(&public).unwrap();
            assert_eq!(
                (actual.dev(), actual.ino()),
                (expected.dev(), expected.ino())
            );
            assert_original_payload(&public);
            assert_missing(&reserved_slot(&owner, &id));
            std::fs::set_permissions(&public, std::fs::Permissions::from_mode(0o700)).unwrap();
            if fault == "fifo" {
                std::fs::remove_file(public.join(MARKER)).unwrap();
            }
            std::fs::write(public.join(MARKER), &proof).unwrap();
            std::fs::set_permissions(public.join(MARKER), std::fs::Permissions::from_mode(0o600))
                .unwrap();
            super::super::cleanup(root.path(), &owner, &id).unwrap();
            assert!(!public.exists());
        }
    }

    #[test]
    fn occupied_public_path_preserves_both_entries_until_restoration_can_retry() {
        for kind in ["directory", "file", "symlink", "dangling-symlink", "fifo"] {
            let root = tempfile::tempdir().unwrap();
            let (owner, id, public) = fixture(root.path());
            let foreign = replacement(root.path(), kind);
            let expected = std::fs::symlink_metadata(&foreign).unwrap();
            let original = public.with_extension("moved");
            let result = cleanup(root.path(), &owner, &id, &mut |point, path| {
                if point == Point::BeforeMove {
                    std::fs::rename(path, &original)?;
                    std::fs::rename(&foreign, path)?;
                }
                if point == Point::AfterRenameBeforeValidation {
                    std::fs::write(&public, "later occupant")?;
                }
                Ok(())
            });
            let error = result.unwrap_err().to_string();
            assert!(error.contains("restoration to"), "{error}");
            assert!(error.contains("cleanup remains pending"));
            let slot = reserved_slot(&owner, &id);
            assert_replacement(&slot, &expected, root.path(), kind);
            let occupant = std::fs::metadata(&public).unwrap();
            assert!(super::super::cleanup(root.path(), &owner, &id).is_err());
            assert_eq!(std::fs::metadata(&public).unwrap().ino(), occupant.ino());
            assert_eq!(std::fs::read_to_string(&public).unwrap(), "later occupant");
            assert_replacement(&slot, &expected, root.path(), kind);
            std::fs::remove_file(&public).unwrap();
            assert!(
                super::super::cleanup(root.path(), &owner, &id)
                    .unwrap_err()
                    .to_string()
                    .contains("entry restored")
            );
            assert_missing(&slot);
            assert_replacement(&public, &expected, root.path(), kind);
            assert_original_payload(&original);
            remove_replacement(&public, kind);
            std::fs::rename(original, &public).unwrap();
            super::super::cleanup(root.path(), &owner, &id).unwrap();
        }
    }

    #[test]
    fn process_death_before_validation_restores_each_foreign_entry_on_restart() {
        for kind in ["directory", "file", "symlink", "dangling-symlink", "fifo"] {
            let root = tempfile::tempdir().unwrap();
            let (owner, id, public) = fixture(root.path());
            let foreign = replacement(root.path(), kind);
            let expected = std::fs::symlink_metadata(&foreign).unwrap();
            kill_at_frontier(root.path(), "substitute");
            let slot = reserved_slot(&owner, &id);
            assert_missing(&public);
            assert_replacement(&slot, &expected, root.path(), kind);
            let original = public.with_extension("moved");
            assert_original_payload(&original);
            if kind == "directory" {
                std::fs::write(&public, "later occupant").unwrap();
                let occupant = std::fs::metadata(&public).unwrap();
                run_cleanup(root.path(), "restore");
                assert_eq!(std::fs::metadata(&public).unwrap().ino(), occupant.ino());
                assert_eq!(std::fs::read_to_string(&public).unwrap(), "later occupant");
                assert_replacement(&slot, &expected, root.path(), kind);
                std::fs::remove_file(&public).unwrap();
            }
            run_cleanup(root.path(), "restore");
            assert_replacement(&public, &expected, root.path(), kind);
            assert_missing(&slot);
            assert_original_payload(&original);
            assert!(owner.join(BINDINGS).is_file());
            remove_replacement(&public, kind);
            std::fs::rename(original, &public).unwrap();
            run_cleanup(root.path(), "finish");
            assert!(!public.exists());
        }
    }

    #[test]
    fn replacement_at_public_path_before_final_rmdir_is_untouched() {
        let root = tempfile::tempdir().unwrap();
        let (owner, id, public) = fixture(root.path());
        cleanup(root.path(), &owner, &id, &mut |point, retired| {
            if point == Point::AfterMarkerRemoval {
                assert!(!retired.join(MARKER).exists());
                std::fs::create_dir(&public)?;
            }
            Ok(())
        })
        .unwrap();
        assert!(public.is_dir());
    }

    #[test]
    fn losing_an_unconfirmed_arena_retries_only_with_valid_original_authority() {
        for source in ["valid", "missing", "markerless"] {
            let root = tempfile::tempdir().unwrap();
            let (owner, id, public) = fixture(root.path());
            assert!(
                cleanup(root.path(), &owner, &id, &mut |point, _| {
                    if point == Point::BeforeMove {
                        return Err(io::Error::other("interrupted before rename"));
                    }
                    Ok(())
                })
                .is_err()
            );
            let ownership = read(&owner, &id).unwrap();
            let arena = &ownership.namespaces[0].retirement.as_ref().unwrap().arena;
            std::fs::remove_dir_all(&arena.root).unwrap();
            if source == "missing" {
                std::fs::remove_dir_all(&public).unwrap();
            }
            if source == "markerless" {
                std::fs::remove_file(public.join(MARKER)).unwrap();
            }
            let result = super::super::cleanup(root.path(), &owner, &id);
            if source == "markerless" {
                assert!(result.is_err());
                assert!(public.is_dir());
            } else {
                result.unwrap();
                assert!(!public.exists());
            }
        }
    }

    #[test]
    fn missing_arena_marker_keeps_confirmed_cleanup_pending() {
        let root = tempfile::tempdir().unwrap();
        let (owner, id, _) = fixture(root.path());
        let mut slot = None;
        assert!(
            cleanup(root.path(), &owner, &id, &mut |point, path| {
                if point == Point::AfterMarkerRemoval {
                    slot = Some(path.to_owned());
                    return Err(io::Error::other("injected interruption"));
                }
                Ok(())
            })
            .is_err()
        );
        let slot = slot.unwrap();
        let proof = slot.parent().unwrap().join(MARKER);
        std::fs::remove_file(&proof).unwrap();
        assert!(super::super::cleanup(root.path(), &owner, &id).is_err());
        assert!(slot.is_dir());
        assert!(owner.join(BINDINGS).is_file());
    }

    #[test]
    fn a_foreign_replacement_in_a_retired_slot_never_grants_authority() {
        let root = tempfile::tempdir().unwrap();
        let (owner, id, _) = fixture(root.path());
        let mut slot = None;
        assert!(
            cleanup(root.path(), &owner, &id, &mut |point, path| {
                if point == Point::AfterMarkerRemoval {
                    slot = Some(path.to_owned());
                    return Err(io::Error::other("injected interruption"));
                }
                Ok(())
            })
            .is_err()
        );
        let slot = slot.unwrap();
        let original = slot.with_extension("moved");
        std::fs::rename(&slot, &original).unwrap();
        std::fs::create_dir(&slot).unwrap();
        assert!(super::super::cleanup(root.path(), &owner, &id).is_err());
        assert!(slot.is_dir());
        std::fs::remove_dir(&slot).unwrap();
        std::fs::rename(original, &slot).unwrap();
        super::super::cleanup(root.path(), &owner, &id).unwrap();
        assert_missing(&slot);
    }
}
