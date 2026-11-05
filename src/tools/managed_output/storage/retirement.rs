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
use nix::fcntl::{Flock, FlockArg, OFlag, openat};
use nix::sys::stat::{Mode, mkdirat};
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
    AfterMove,
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
            if missing(directory_at(&arena, OsStr::new(&slot)))?.is_none() {
                let Some(original) = missing(binding.open(owner, conversation))? else {
                    // Neither source nor reserved slot exists: settle any
                    // earlier interrupted rename before confirming absence.
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
                move_namespace(&binding, &ownership, &original, &arena, &slot, hook)?;
            }
            let moved = directory_at(&arena, OsStr::new(&slot))?;
            private_directory(&moved, binding.device, binding.inode)?;
            if marker::<Marker>(&moved)? != binding.marker(owner, conversation) {
                return Err(invalid());
            }
            // This also covers restart after rename but before either sync.
            directory(source_parent)?.sync_all()?;
            arena.sync_all()?;
            ownership.namespaces[index]
                .retirement
                .as_mut()
                .unwrap()
                .confirmed = true;
            write_with_sync(&ownership, true)?;
        }
        remove_slot(&arena, &binding, owner, conversation, &slot, hook)?;
    }
    Ok(())
}

fn move_namespace(
    binding: &Binding,
    ownership: &Ownership,
    original: &File,
    arena: &File,
    slot: &str,
    hook: &mut dyn FnMut(Point, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let owner = &ownership.owner;
    let conversation = &ownership.conversation;
    let source_parent = binding.root.parent().ok_or_else(invalid)?;
    let source_leaf = binding.root.file_name().ok_or_else(invalid)?;
    let parent = directory(source_parent)?;
    hook(Point::BeforeMove, &binding.root)?;
    renameat_with(&parent, source_leaf, arena, slot, RenameFlags::NOREPLACE)?;
    let moved = directory_at(arena, OsStr::new(slot))?;
    if private_directory(
        &moved,
        original.metadata()?.dev(),
        original.metadata()?.ino(),
    )
    .is_err()
        || marker::<Marker>(&moved)? != binding.marker(owner, conversation)
    {
        // A public-path substitution was captured. Never delete it
        // or overwrite a new occupant while trying to restore it.
        let _ = renameat_with(arena, slot, &parent, source_leaf, RenameFlags::NOREPLACE);
        return Err(invalid());
    }
    hook(
        Point::AfterMove,
        &binding.retirement.as_ref().unwrap().arena.root.join(slot),
    )?;
    Ok(())
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
            .args(["--exact", CHILD, "--nocapture"])
            .env(
                "RUSTX_OUTPUT_RETIREMENT_TEST_ROOT",
                root.canonicalize().unwrap(),
            )
            .env("RUSTX_OUTPUT_RETIREMENT_TEST_MODE", mode);
        command
    }

    #[test]
    fn cleanup_process() {
        let Some(root) = std::env::var_os("RUSTX_OUTPUT_RETIREMENT_TEST_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let mode = std::env::var("RUSTX_OUTPUT_RETIREMENT_TEST_MODE").unwrap();
        cleanup(
            &root,
            &root.join("private"),
            &ConversationId::new(ID),
            &mut |point, _| {
                if (mode == "stop" && point == Point::AfterMarkerRemoval)
                    || (mode == "move" && point == Point::AfterMove)
                {
                    println!("RETIREMENT_MARKER_REMOVED");
                    io::stdout().flush()?;
                    loop {
                        std::thread::park();
                    }
                }
                Ok(())
            },
        )
        .unwrap();
    }

    #[test]
    fn process_death_after_rename_or_marker_unlink_preserves_public_replacement() {
        use std::io::BufRead;
        for mode in ["move", "stop"] {
            let root = tempfile::tempdir().unwrap();
            let (owner, id, public) = fixture(root.path());
            let mut child = command(root.path(), mode)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                for line in io::BufReader::new(stdout).lines() {
                    if line.unwrap().contains("RETIREMENT_MARKER_REMOVED") {
                        tx.send(()).unwrap();
                        return;
                    }
                }
            });
            let frontier = rx.recv_timeout(std::time::Duration::from_secs(10));
            child.kill().unwrap();
            child.wait().unwrap();
            reader.join().unwrap();
            frontier.unwrap();
            let ownership = read(&owner, &id).unwrap();
            let retired = ownership.namespaces[0].retirement.as_ref().unwrap();
            assert_eq!(retired.confirmed, mode == "stop");
            let slot = retired.arena.root.join(format!("{ID}-n0"));
            assert!(slot.is_dir());
            assert_eq!(slot.join(MARKER).exists(), mode == "move");
            assert!(retired.arena.root.join(MARKER).is_file());
            std::fs::create_dir(&public).unwrap();
            assert!(command(root.path(), "finish").status().unwrap().success());
            assert!(!slot.exists());
            assert!(public.is_dir());
            assert!(retired.arena.root.join(MARKER).is_file());
            super::super::cleanup(root.path(), &owner, &id).unwrap();
            assert!(public.is_dir());
        }
    }

    #[test]
    fn public_substitution_after_validation_is_preserved_and_original_remains_owned() {
        let root = tempfile::tempdir().unwrap();
        let (owner, id, public) = fixture(root.path());
        let original = public.with_extension("moved");
        let mut swapped = false;
        let result = cleanup(root.path(), &owner, &id, &mut |point, path| {
            if point == Point::BeforeMove && !swapped {
                std::fs::rename(path, &original)?;
                std::fs::create_dir(path)?;
                swapped = true;
            }
            Ok(())
        });
        assert!(result.is_err());
        assert!(public.is_dir());
        assert_eq!(std::fs::read_dir(&public).unwrap().count(), 0);
        assert!(original.join(MARKER).is_file());
        assert_eq!(std::fs::read_dir(&original).unwrap().count(), 2);
        std::fs::remove_dir(&public).unwrap();
        std::fs::rename(original, &public).unwrap();
        super::super::cleanup(root.path(), &owner, &id).unwrap();
        assert!(!public.exists());
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
        assert!(!slot.exists());
    }
}
