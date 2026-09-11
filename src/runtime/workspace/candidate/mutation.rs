//! Kernel mutation observation, drained synchronously at physical settlement.
//! Queue loss and watch loss invalidate validation, never imply unchanged.
use super::{BTreeSet, PathBuf, WorkspaceLease};
#[cfg(target_os = "macos")]
use nix::{fcntl::OFlag, sys::stat::Mode};

const MAX_WATCHES: usize = 100_000;
const MAX_DIRECTORY_DEPTH: usize = 64;
const MAX_DIRECTORY_ENTRIES: usize = 100_000;

// Include empty and ignored directories: filtering events remains a separate
// source-policy decision. Never traverse a symlink. Admission is bounded even
// when an ignored cache contains an enormous tree.
#[cfg(unix)]
fn existing_directories(root: &std::path::Path) -> Result<BTreeSet<PathBuf>, String> {
    use nix::fcntl::AtFlags;
    use nix::sys::stat::{SFlag, fstatat};
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
    let root_file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)
        .map_err(|e| e.to_string())?;
    let mut directories = BTreeSet::new();
    let mut pending = vec![(PathBuf::new(), 0)];
    let mut entries = 0;
    while let Some((relative, depth)) = pending.pop() {
        if depth > MAX_DIRECTORY_DEPTH || directories.len() >= MAX_WATCHES {
            return Err("candidate directory watch bound exceeded".into());
        }
        let file =
            super::open_directory_relative(&root_file, &relative).map_err(|e| e.to_string())?;
        let mut directory = nix::dir::Dir::from_fd(
            super::super::observation::retry_io(|| file.try_clone())
                .map_err(|e| e.to_string())?
                .into(),
        )
        .map_err(|e| e.to_string())?;
        directories.insert(root.join(&relative));
        for entry in directory.iter() {
            let entry = match entry {
                Err(nix::errno::Errno::EINTR) => continue, // same directory stream
                result => result.map_err(|e| e.to_string())?,
            };
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            entries += 1;
            if entries > MAX_DIRECTORY_ENTRIES {
                return Err("candidate directory enumeration bound exceeded".into());
            }
            let name = std::ffi::OsStr::from_bytes(name);
            let stat = super::super::observation::retry_nix(|| {
                fstatat(&file, name, AtFlags::AT_SYMLINK_NOFOLLOW)
            })
            .map_err(|e| e.to_string())?;
            if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR {
                pending.push((relative.join(name), depth + 1));
            }
        }
    }
    Ok(directories)
}

#[cfg(not(unix))]
fn existing_directories(_: &std::path::Path) -> Result<BTreeSet<PathBuf>, String> {
    Err("candidate directory observation requires Unix".into())
}

#[derive(Debug)]
pub(super) struct MutationWatch {
    kernel: Kernel,
    root: PathBuf,
    source: BTreeSet<PathBuf>,
    control: BTreeSet<PathBuf>,
}

impl MutationWatch {
    pub(super) async fn start(lease: &WorkspaceLease) -> Result<Self, String> {
        let root = lease
            .physical_worktree_root()
            .ok_or("missing candidate root")?
            .to_path_buf();
        let listing = lease
            .source_listing(&[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ])
            .await?;
        let committed = lease
            .source_listing(&["ls-tree", "-r", "--name-only", "-z", "HEAD"])
            .await?;
        let mut source = BTreeSet::new();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            for path in listing
                .split(|b| *b == 0)
                .chain(committed.split(|b| *b == 0))
                .filter(|p| !p.is_empty())
            {
                source.insert(root.join(std::ffi::OsStr::from_bytes(path)));
            }
        }
        #[cfg(not(unix))]
        {
            let _ = listing;
            return Err("candidate mutation observation requires Unix".into());
        }
        let branch = &lease
            .snapshot
            .git_worktree()
            .ok_or("missing candidate Git facts")?
            .branch;
        let mut control = BTreeSet::new();
        for name in [
            "index".to_owned(),
            "HEAD".to_owned(),
            format!("refs/heads/{branch}"),
            "packed-refs".to_owned(),
        ] {
            let path = lease
                .manager
                .git_text(
                    &root,
                    vec![
                        "rev-parse".into(),
                        "--path-format=absolute".into(),
                        "--git-path".into(),
                        name.into(),
                    ],
                    None,
                )
                .await
                .map_err(|e| e.to_string())?;
            control.insert(PathBuf::from(path));
        }
        let mut directories = existing_directories(&root)?;
        for path in source.iter().chain(&control) {
            let mut parent = path.parent();
            while let Some(path) = parent {
                if super::super::observation::retry_io(|| std::fs::metadata(path))
                    .map(|metadata| metadata.is_dir())
                    .or_else(|error| {
                        if error.kind() == std::io::ErrorKind::NotFound {
                            Ok(false)
                        } else {
                            Err(error)
                        }
                    })
                    .map_err(|e| e.to_string())?
                {
                    directories.insert(path.to_path_buf());
                }
                if path == root || !path.starts_with(&root) {
                    break;
                }
                parent = path.parent();
            }
        }
        if source.len() + directories.len() + control.len() > MAX_WATCHES {
            return Err("candidate mutation watch bound exceeded".into());
        }
        let kernel = Kernel::start(&directories, &source, &control)?;
        Ok(Self {
            kernel,
            root,
            source,
            control,
        })
    }

    pub(super) async fn changed(&self, lease: &WorkspaceLease) -> Result<bool, String> {
        let (paths, directory_mutation) = self.kernel.drain()?;
        if directory_mutation {
            return Ok(true);
        }
        for path in paths {
            if self.source.contains(&path) || self.control.contains(&path) {
                return Ok(true);
            }
            let Ok(relative) = path.strip_prefix(&self.root) else {
                continue;
            };
            if relative.as_os_str().is_empty() {
                return Ok(true);
            }
            // Only Git-ignored additions/cache mutations may be excluded.
            // Tracked files bypass this test even if an ignore rule matches.
            let ignored = lease
                .manager
                .git_raw(
                    &self.root,
                    vec![
                        "check-ignore".into(),
                        "--quiet".into(),
                        "--".into(),
                        relative.as_os_str().into(),
                    ],
                    None,
                )
                .await
                .map_err(|e| e.to_string())?;
            match ignored.status.code() {
                Some(0) => {}
                Some(1) => return Ok(true),
                _ => return Err("could not classify a candidate mutation".into()),
            }
        }
        Ok(false)
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct Kernel {
    watch: nix::sys::inotify::Inotify,
    paths: std::collections::HashMap<nix::sys::inotify::WatchDescriptor, PathBuf>,
}

#[cfg(target_os = "linux")]
impl Kernel {
    fn start(
        directories: &BTreeSet<PathBuf>,
        _: &BTreeSet<PathBuf>,
        _: &BTreeSet<PathBuf>,
    ) -> Result<Self, String> {
        use nix::sys::inotify::{AddWatchFlags as F, InitFlags, Inotify};
        let watch = Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC)
            .map_err(|e| e.to_string())?;
        let mut paths = std::collections::HashMap::new();
        for path in directories {
            let descriptor = watch
                .add_watch(
                    path,
                    F::IN_MODIFY
                        | F::IN_ATTRIB
                        | F::IN_CREATE
                        | F::IN_DELETE
                        | F::IN_MOVED_FROM
                        | F::IN_MOVED_TO
                        | F::IN_DELETE_SELF
                        | F::IN_MOVE_SELF
                        | F::IN_ONLYDIR
                        | F::IN_DONT_FOLLOW,
                )
                .map_err(|e| format!("cannot watch candidate source: {e}"))?;
            paths.insert(descriptor, path.clone());
        }
        Ok(Self { watch, paths })
    }
    fn drain(&self) -> Result<(BTreeSet<PathBuf>, bool), String> {
        use nix::sys::inotify::AddWatchFlags as F;
        let mut changed = BTreeSet::new();
        let mut directory_mutation = false;
        loop {
            let events = match super::super::observation::retry_nix(|| self.watch.read_events()) {
                Ok(events) => events,
                Err(nix::errno::Errno::EAGAIN) => return Ok((changed, directory_mutation)),
                Err(error) => return Err(error.to_string()),
            };
            for event in events {
                if event.mask.intersects(
                    F::IN_Q_OVERFLOW
                        | F::IN_IGNORED
                        | F::IN_UNMOUNT
                        | F::IN_DELETE_SELF
                        | F::IN_MOVE_SELF,
                ) {
                    return Err("candidate mutation observation lost coverage".into());
                }
                if event.mask.contains(F::IN_ISDIR)
                    && event.mask.intersects(F::IN_CREATE | F::IN_MOVED_TO)
                {
                    // No descendant watch was admitted for this new tree.
                    directory_mutation = true;
                }
                let directory = self.paths.get(&event.wd).ok_or("unknown candidate watch")?;
                changed.insert(
                    event
                        .name
                        .map_or_else(|| directory.clone(), |name| directory.join(name)),
                );
                if changed.len() > 100_000 {
                    return Err("candidate mutation observation overflow".into());
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct Kernel {
    queue: nix::sys::event::Kqueue,
    files: Vec<std::fs::File>,
    paths: std::collections::HashMap<usize, (PathBuf, std::fs::Metadata)>,
}

#[cfg(target_os = "macos")]
impl Kernel {
    fn start(
        directories: &BTreeSet<PathBuf>,
        source: &BTreeSet<PathBuf>,
        control: &BTreeSet<PathBuf>,
    ) -> Result<Self, String> {
        use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};
        use std::os::fd::AsRawFd;
        let queue = Kqueue::new().map_err(|e| e.to_string())?;
        let mut files = Vec::new();
        let mut paths = std::collections::HashMap::new();
        for path in directories.iter().chain(source).chain(control) {
            if !super::super::observation::retry_io(|| path.try_exists())
                .map_err(|e| e.to_string())?
            {
                continue;
            }
            let fd = super::super::observation::retry_nix(|| {
                nix::fcntl::open(
                    path,
                    // nix does not name these Darwin-only flags. Retain their
                    // libc bits so symlinks are observed, never followed.
                    OFlag::from_bits_retain(libc::O_EVTONLY | libc::O_SYMLINK) | OFlag::O_CLOEXEC,
                    Mode::empty(),
                )
            })
            .map_err(|e| e.to_string())?;
            let file = std::fs::File::from(fd);
            let ident = usize::try_from(file.as_raw_fd()).map_err(|e| e.to_string())?;
            let event = KEvent::new(
                ident,
                EventFilter::EVFILT_VNODE,
                EvFlags::EV_ADD | EvFlags::EV_CLEAR,
                FilterFlag::NOTE_WRITE
                    | FilterFlag::NOTE_EXTEND
                    | FilterFlag::NOTE_ATTRIB
                    | FilterFlag::NOTE_LINK
                    | FilterFlag::NOTE_RENAME
                    | FilterFlag::NOTE_DELETE
                    | FilterFlag::NOTE_REVOKE,
                0,
                0,
            );
            queue
                .kevent(
                    &[event],
                    &mut [],
                    Some(libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    }),
                )
                .map_err(|e| e.to_string())?;
            paths.insert(
                ident,
                (
                    path.clone(),
                    super::super::observation::retry_io(|| file.metadata())
                        .map_err(|e| e.to_string())?,
                ),
            );
            files.push(file);
        }
        Ok(Self {
            queue,
            files,
            paths,
        })
    }
    fn drain(&self) -> Result<(BTreeSet<PathBuf>, bool), String> {
        use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent};
        let _keep_descriptors = &self.files;
        let empty = KEvent::new(
            0,
            EventFilter::EVFILT_VNODE,
            EvFlags::empty(),
            FilterFlag::empty(),
            0,
            0,
        );
        let mut events = [empty; 128];
        let mut changed = BTreeSet::new();
        let mut directory_mutation = false;
        loop {
            let count = super::super::observation::retry_nix(|| {
                self.queue.kevent(
                    &[],
                    &mut events,
                    Some(libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    }),
                )
            })
            .map_err(|e| e.to_string())?;
            if count == 0 {
                return Ok((changed, directory_mutation));
            }
            for event in &events[..count] {
                if event.flags().contains(EvFlags::EV_ERROR)
                    || event.fflags().contains(FilterFlag::NOTE_REVOKE)
                {
                    return Err("candidate mutation observation lost coverage".into());
                }
                let (path, initial) = self
                    .paths
                    .get(&event.ident())
                    .ok_or("unknown candidate watch")?;
                if initial.is_dir()
                    && event
                        .fflags()
                        .intersects(FilterFlag::NOTE_DELETE | FilterFlag::NOTE_RENAME)
                {
                    return Err("candidate mutation observation lost directory coverage".into());
                }
                if initial.is_dir() && event.fflags().contains(FilterFlag::NOTE_WRITE) {
                    // vnode has no child name/type. A directory-entry change
                    // could introduce an unwatched tree, even under an ignored
                    // cache with source exceptions. Conservatively fail closed.
                    directory_mutation = true;
                }
                if event.fflags() == FilterFlag::NOTE_ATTRIB {
                    let current =
                        super::super::observation::retry_io(|| std::fs::symlink_metadata(path))
                            .map_err(|e| e.to_string())?;
                    if unchanged_non_access_attributes(initial, &current) {
                        continue;
                    }
                }
                #[cfg(test)]
                eprintln!(
                    "candidate vnode observation: {} {:?}",
                    path.display(),
                    event.fflags()
                );
                changed.insert(path.clone());
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn unchanged_non_access_attributes(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // NOTE_ATTRIB includes access-time bookkeeping from reads. This only
    // excludes that noise; it never establishes source equality. Exact
    // content/index/mode hashing and independent NOTE_WRITE/EXTEND events
    // remain mandatory, including for write-and-restore detection.
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.mode() == after.mode()
        && before.nlink() == after.nlink()
        && before.uid() == after.uid()
        && before.gid() == after.gid()
        && before.len() == after.len()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[derive(Debug)]
struct Kernel;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl Kernel {
    fn start(
        _: &BTreeSet<PathBuf>,
        _: &BTreeSet<PathBuf>,
        _: &BTreeSet<PathBuf>,
    ) -> Result<Self, String> {
        Err("candidate mutation observation is supported only on Linux/macOS".into())
    }
    fn drain(&self) -> Result<(BTreeSet<PathBuf>, bool), String> {
        Err("candidate mutation observation unavailable".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_depth_bound_rejects_incomplete_watch_admission() {
        let root = tempfile::tempdir().unwrap();
        let mut path = root.path().to_path_buf();
        for _ in 0..MAX_DIRECTORY_DEPTH {
            path.push("d");
        }
        std::fs::create_dir_all(&path).unwrap();
        assert_eq!(
            existing_directories(root.path()).unwrap().len(),
            MAX_DIRECTORY_DEPTH + 1
        );
        std::fs::create_dir(path.join("too-deep")).unwrap();
        assert!(
            existing_directories(root.path())
                .unwrap_err()
                .contains("bound exceeded")
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_admission_never_traverses_symlinks() {
        let root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(root.path(), root.path().join("cycle")).unwrap();
        assert_eq!(
            existing_directories(root.path()).unwrap(),
            BTreeSet::from([root.path().to_path_buf()])
        );
    }
}
