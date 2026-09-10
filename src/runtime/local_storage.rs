//! Canonical product identity, controller admission and Conversation lifecycle access.
use nix::fcntl::{Flock, FlockArg};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Canonical product identity; this is not a live storage-access guard.
#[derive(Debug, Clone)]
pub struct ProductRoot {
    root: PathBuf,
}
impl ProductRoot {
    /// Resolve existing native product state without creating anything.
    /// # Errors
    /// Missing roots and filesystem errors are returned.
    pub fn existing(root: &Path) -> io::Result<Self> {
        let root = root.canonicalize()?;
        directory(&root)?;
        Ok(Self { root })
    }
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Freeze native ownership transitions, without excluding ordinary activity.
    /// # Errors
    /// A concurrent ownership transaction or preflight returns `WouldBlock`.
    pub(crate) fn freeze_ownership(&self) -> io::Result<OwnershipSnapshot> {
        Ok(OwnershipSnapshot {
            _lock: lock(directory(&self.root)?, FlockArg::LockExclusiveNonblock)?,
        })
    }
    /// Enter a native ownership transaction before accessing SQLite/catalog state.
    /// # Errors
    /// An ownership snapshot returns `WouldBlock`; callers must not mutate.
    pub(crate) fn ownership_mutation(&self) -> io::Result<OwnershipMutation> {
        Ok(OwnershipMutation {
            _lock: lock(directory(&self.root)?, FlockArg::LockSharedNonblock)?,
        })
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

/// The sole native Session/catalog controller, independent of target access.
#[derive(Debug)]
pub struct ProductController {
    root: ProductRoot,
    _lock: Flock<File>,
}
impl std::ops::Deref for ProductController {
    type Target = ProductRoot;
    fn deref(&self) -> &ProductRoot {
        &self.root
    }
}
impl ProductController {
    /// Admit a controller at explicit startup, creating only startup metadata.
    /// # Errors
    /// Another controller or invalid storage returns an error.
    pub fn acquire(root: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(root)?;
        let root = ProductRoot::existing(root)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(root.root().join(".product-writer.lock"))?;
        Ok(Self {
            root,
            _lock: lock(file, FlockArg::LockExclusiveNonblock)?,
        })
    }
}
#[derive(Debug)]
pub(crate) struct OwnershipSnapshot {
    _lock: Flock<File>,
}
/// OS-backed participation in local ownership transitions. Workspace disposal
/// retains this authority through physical mutation and durable settlement.
#[derive(Debug)]
pub struct OwnershipMutation {
    _lock: Flock<File>,
}

/// Shared native access to one authoritative Conversation allocation.
#[derive(Debug)]
pub struct ConversationAccess {
    root: ProductRoot,
    _lock: Flock<File>,
    controller: Option<Arc<ProductController>>,
}
impl std::ops::Deref for ConversationAccess {
    type Target = ProductRoot;
    fn deref(&self) -> &ProductRoot {
        &self.root
    }
}
impl ConversationAccess {
    /// Access an existing identity-derived allocation without creating it.
    /// # Errors
    /// Missing/unsafe allocations or a destructive owner are rejected.
    pub fn existing(root: &ProductRoot, allocation: &Path) -> io::Result<Self> {
        let path = root.confined(allocation)?;
        Ok(Self {
            root: root.clone(),
            _lock: lock(directory(&path)?, FlockArg::LockSharedNonblock)?,
            controller: None,
        })
    }
    pub(crate) fn existing_for_controller(
        controller: Arc<ProductController>,
        allocation: &Path,
    ) -> io::Result<Self> {
        let mut access = Self::existing(&controller, allocation)?;
        access.controller = Some(controller);
        Ok(access)
    }

    /// Explicit runtime startup reserves an allocation before any private writes.
    /// # Errors
    /// Unsafe paths or a destructive owner are rejected.
    pub(crate) fn start(controller: Arc<ProductController>, allocation: &Path) -> io::Result<Self> {
        let path = controller.confined(allocation)?;
        let _mutation = controller.ownership_mutation()?;
        std::fs::create_dir_all(&path)?;
        Self::existing_for_controller(controller, &path)
    }
}
/// Exclusive access acquired in sorted authoritative `ConversationId` order.
#[derive(Debug)]
pub(crate) struct ConversationExclusion {
    _lock: Flock<File>,
}
impl ConversationExclusion {
    pub(crate) fn acquire(root: &ProductRoot, allocation: &Path) -> io::Result<Self> {
        Ok(Self {
            _lock: lock(
                directory(&root.confined(allocation)?)?,
                FlockArg::LockExclusiveNonblock,
            )?,
        })
    }
}
fn directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
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
        let Some(root) = std::env::var_os("RUSTX_260_LOCK_ROOT") else {
            return;
        };
        let root = ProductRoot::existing(Path::new(&root)).unwrap();
        let allocation = root.root().join("conversation-b");
        let _guard: Box<dyn std::any::Any> = match std::env::var("RUSTX_260_LOCK_MODE")
            .unwrap()
            .as_str()
        {
            "controller" => Box::new(ProductController::acquire(root.root()).unwrap()),
            "access" => Box::new(ConversationAccess::existing(&root, &allocation).unwrap()),
            "exclusive" => Box::new(ConversationExclusion::acquire(&root, &allocation).unwrap()),
            _ => panic!("invalid gate mode"),
        };
        println!("LIFECYCLE_ACQUIRED");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_exact(&mut [0]).unwrap();
    }
    fn owner(root: &Path, mode: &str) -> std::process::Child {
        let mut process = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::local_storage::tests::lifecycle_process_gate",
                "--nocapture",
            ])
            .env("RUSTX_260_LOCK_ROOT", root)
            .env("RUSTX_260_LOCK_MODE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(process.stdout.take().unwrap());
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
        process.stdout = Some(output.into_inner());
        process
    }
    #[test]
    fn cross_process_target_conflicts_aliases_death_and_independent_roots() {
        let directory = tempfile::tempdir().unwrap();
        let root = ProductRoot::existing(directory.path()).unwrap();
        let allocation = root.root().join("conversation-b");
        std::fs::create_dir(&allocation).unwrap();
        let aliases = tempfile::tempdir().unwrap();
        let alias = aliases.path().join("alias");
        std::os::unix::fs::symlink(root.root(), &alias).unwrap();
        for mode in ["access", "exclusive"] {
            let mut process = owner(root.root(), mode);
            for spelling in [
                root.root().to_path_buf(),
                alias.clone(),
                root.root()
                    .join("../")
                    .join(root.root().file_name().unwrap()),
            ] {
                let identity = ProductRoot::existing(&spelling).unwrap();
                assert_eq!(
                    ConversationExclusion::acquire(
                        &identity,
                        &identity.root().join("conversation-b")
                    )
                    .unwrap_err()
                    .kind(),
                    io::ErrorKind::WouldBlock
                );
            }
            let independent = tempfile::tempdir().unwrap();
            let other = ProductRoot::existing(independent.path()).unwrap();
            std::fs::create_dir(other.root().join("conversation-b")).unwrap();
            let _unrelated =
                ConversationExclusion::acquire(&other, &other.root().join("conversation-b"))
                    .unwrap();
            process.kill().unwrap();
            process.wait().unwrap();
            let exclusive = ConversationExclusion::acquire(&root, &allocation).unwrap();
            assert!(ConversationAccess::existing(&root, &allocation).is_err());
            drop(exclusive);
            assert!(ConversationAccess::existing(&root, &allocation).is_ok());
        }
    }
    #[test]
    fn cross_process_controller_admission_is_independent_of_target_exclusion() {
        let directory = tempfile::tempdir().unwrap();
        let root = ProductRoot::existing(directory.path()).unwrap();
        std::fs::create_dir(root.root().join("conversation-b")).unwrap();
        let mut process = owner(root.root(), "controller");
        assert_eq!(
            ProductController::acquire(root.root()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let _snapshot = root.freeze_ownership().unwrap();
        let _target =
            ConversationExclusion::acquire(&root, &root.root().join("conversation-b")).unwrap();
        process.stdin.take().unwrap().write_all(b"x").unwrap();
        assert!(process.wait().unwrap().success());
        assert!(ProductController::acquire(root.root()).is_ok());
    }
    #[test]
    fn management_lock_lookup_is_noncreating_and_paths_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let root = ProductRoot::existing(directory.path()).unwrap();
        let missing = root.root().join("unknown");
        assert!(ProductRoot::existing(&missing).is_err());
        assert!(ConversationAccess::existing(&root, &missing).is_err());
        assert!(ConversationExclusion::acquire(&root, &missing).is_err());
        assert!(!missing.exists());
        std::os::unix::fs::symlink("/tmp", root.root().join("escape")).unwrap();
        assert!(root.confined(&root.root().join("escape/unknown")).is_err());
        assert!(root.confined(&root.root().join("../outside")).is_err());
    }
}
