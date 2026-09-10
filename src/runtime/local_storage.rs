//! OS-backed access to one canonical local product root.
//!
//! The directory inode is the lifecycle lock, so management never creates a
//! lock file. It must never be removed or replaced by product cleanup. A
//! separate permanent writer file admits one Session controller; child and
//! inspection access share the directory lock without claiming controller status.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use nix::fcntl::{Flock, FlockArg};

/// An access capability retained for the complete lifetime of native access.
#[derive(Debug)]
pub struct LocalStorageGuard {
    root: PathBuf,
    writer: Option<Flock<File>>,
    _lifecycle: Flock<File>,
}

impl LocalStorageGuard {
    /// Admits one product writer, creating the root only for explicit startup.
    ///
    /// # Errors
    /// Fails if another controller or exclusive lifecycle owner is present.
    pub fn writer(root: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(root)?;
        let mut guard = Self::acquire(root, FlockArg::LockSharedNonblock)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(guard.root.join(".product-writer.lock"))?;
        guard.writer = Some(lock(file, FlockArg::LockExclusiveNonblock)?);
        Ok(guard)
    }

    /// Opens existing product state for a child or inspection participant.
    ///
    /// # Errors
    /// Fails for a missing root or an exclusive lifecycle owner.
    pub fn access_existing(root: &Path) -> io::Result<Self> {
        Self::acquire(root, FlockArg::LockSharedNonblock)
    }

    /// Acquires exclusive lifecycle authority before any ownership read.
    /// This does not create, recover, dispose, or delete anything.
    ///
    /// # Errors
    /// Fails for a missing root or any live participant.
    pub(crate) fn exclusive_existing(root: &Path) -> io::Result<Self> {
        Self::acquire(root, FlockArg::LockExclusiveNonblock)
    }

    fn acquire(root: &Path, mode: FlockArg) -> io::Result<Self> {
        let root = root.canonicalize()?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&root)?;
        let lifecycle = lock(file, mode)?;
        Ok(Self {
            root,
            writer: None,
            _lifecycle: lifecycle,
        })
    }

    pub(crate) fn is_writer(&self) -> bool {
        self.writer.is_some()
    }

    /// Canonical identity established before acquiring the OS lock.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validates an identity-derived allocation, including missing leaves.
    /// No symlink below the canonical product root is a storage identity.
    ///
    /// # Errors
    /// Rejects escapes, non-normal components and symlinks, including dangling ones.
    pub fn confined(&self, path: &Path) -> io::Result<PathBuf> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| io::Error::other("storage path escapes canonical runtime root"))?;
        let mut current = self.root.clone();
        for component in relative.components() {
            let std::path::Component::Normal(name) = component else {
                return Err(io::Error::other("invalid storage path component"));
            };
            current.push(name);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(io::Error::other(
                        "symlink is not a private storage identity",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(current)
    }
}

fn lock(file: File, mode: FlockArg) -> io::Result<Flock<File>> {
    Flock::lock(file, mode).map_err(|(_, error)| {
        if error == nix::errno::Errno::EWOULDBLOCK {
            io::Error::new(io::ErrorKind::WouldBlock, "local product storage is in use")
        } else {
            io::Error::other(error)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Command, Stdio};

    #[test]
    fn lifecycle_process_gate() {
        let Some(root) = std::env::var_os("RUSTX_254_LOCK_ROOT") else {
            return;
        };
        let mode = std::env::var("RUSTX_254_LOCK_MODE").unwrap();
        let root = Path::new(&root);
        let _guard = match mode.as_str() {
            "writer" => LocalStorageGuard::writer(root),
            "access" => LocalStorageGuard::access_existing(root),
            "exclusive" => LocalStorageGuard::exclusive_existing(root),
            _ => panic!("invalid process mode"),
        }
        .unwrap();
        println!("LIFECYCLE_ACQUIRED");
        std::io::stdout().flush().unwrap();
        let mut byte = [0];
        std::io::stdin().read_exact(&mut byte).unwrap();
    }

    fn owner(root: &Path, mode: &str) -> std::process::Child {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::local_storage::tests::lifecycle_process_gate",
                "--nocapture",
            ])
            .env("RUSTX_254_LOCK_ROOT", root)
            .env("RUSTX_254_LOCK_MODE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "owner exited before gate"
            );
            if line.trim() == "LIFECYCLE_ACQUIRED" {
                break;
            }
        }
        child.stdout = Some(output.into_inner());
        child
    }

    #[test]
    fn cross_process_writer_exclusion_aliases_and_crash_release() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let aliases = tempfile::tempdir().unwrap();
        let alias = aliases.path().join("alias");
        std::os::unix::fs::symlink(root.path(), &alias).unwrap();
        let mut child = owner(root.path(), "writer");
        for path in [
            root.path().to_path_buf(),
            alias,
            root.path()
                .join("../")
                .join(root.path().file_name().unwrap()),
        ] {
            assert_eq!(
                LocalStorageGuard::writer(&path).unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
            assert_eq!(
                LocalStorageGuard::exclusive_existing(&path)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::WouldBlock
            );
        }
        let independent = LocalStorageGuard::writer(other.path()).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        let exclusive = LocalStorageGuard::exclusive_existing(root.path()).unwrap();
        assert!(LocalStorageGuard::access_existing(root.path()).is_err());
        drop(exclusive);
        let _successor = LocalStorageGuard::writer(root.path()).unwrap();
        drop(independent);
    }

    #[test]
    fn cross_process_child_or_inspector_blocks_exclusive_until_release() {
        let root = tempfile::tempdir().unwrap();
        let mut child = owner(root.path(), "access");
        assert_eq!(
            LocalStorageGuard::exclusive_existing(root.path())
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        child.stdin.take().unwrap().write_all(b"x").unwrap();
        assert!(child.wait().unwrap().success());
        let _exclusive = LocalStorageGuard::exclusive_existing(root.path()).unwrap();
    }

    #[test]
    fn management_lock_lookup_is_noncreating_and_paths_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("unknown");
        assert!(LocalStorageGuard::exclusive_existing(&missing).is_err());
        assert!(!missing.exists());
        let guard = LocalStorageGuard::exclusive_existing(root.path()).unwrap();
        std::os::unix::fs::symlink("/tmp", root.path().join("escape")).unwrap();
        assert!(guard.confined(&root.path().join("escape/unknown")).is_err());
        assert!(guard.confined(&root.path().join("../outside")).is_err());
    }
}
