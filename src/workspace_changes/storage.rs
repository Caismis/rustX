//! One exact, owner-marked namespace shared by recorders using a temporary root.
//! Each Attempt owns only its child directory, including Git's stdin files.
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
};

const OWNER: &[u8] = b"rustX workspace change snapshots\n";
// Session admission can start several recorders together. Publish the marker
// before another local recorder attempts to open the new namespace.
static INITIALIZATION: Mutex<()> = Mutex::new(());

pub(super) struct SnapshotDirectory {
    root: PathBuf,
    directory: tempfile::TempDir,
}

impl SnapshotDirectory {
    pub fn create(temporary_root: &Path) -> io::Result<Self> {
        let _initialization = INITIALIZATION
            .lock()
            .map_err(|_| io::Error::other("snapshot storage initialization failed"))?;
        // Resolve aliases before comparing against Git's physical worktree path.
        let root = temporary_root.canonicalize()?.join(format!(
            "rustx-workspace-changes-{}",
            nix::unistd::geteuid()
        ));
        let created = match fs::DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(error),
        };
        let metadata = fs::symlink_metadata(&root)?;
        if !metadata.is_dir()
            || metadata.uid() != nix::unistd::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
        {
            return Err(io::Error::other(
                "snapshot storage requires an owner-only directory",
            ));
        }
        let marker = root.join(".owner");
        if created {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&marker)?
                .write_all(OWNER)?;
        } else {
            // Never adopt an existing user directory, even when its permissions
            // match. Incomplete initialization makes this optional observation
            // unavailable; it does not authorize overwriting someone else's data.
            let mut file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&marker)?;
            let metadata = file.metadata()?;
            let mut owner = Vec::new();
            if !metadata.is_file()
                || metadata.uid() != nix::unistd::geteuid().as_raw()
                || metadata.mode() & 0o077 != 0
            {
                return Err(io::Error::other("invalid snapshot storage owner"));
            }
            (&mut file)
                .take(OWNER.len() as u64 + 1)
                .read_to_end(&mut owner)?;
            if owner != OWNER {
                return Err(io::Error::other("unrecognized snapshot storage owner"));
            }
        }
        let directory = tempfile::Builder::new()
            .prefix("attempt-")
            .tempdir_in(&root)?;
        Ok(Self { root, directory })
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn contains(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }
}
