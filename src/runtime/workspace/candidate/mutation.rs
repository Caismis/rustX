//! Kernel mutation observation, drained synchronously at physical settlement.
//! Queue loss and watch loss invalidate validation, never imply unchanged.
#[cfg(target_os = "macos")]
use super::super::super::{Mode, OFlag};
use super::{BTreeSet, PathBuf, WorkspaceLease};

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
        let mut directories = BTreeSet::from([root.clone()]);
        for path in source.iter().chain(&control) {
            let mut parent = path.parent();
            while let Some(path) = parent {
                if path.is_dir() {
                    directories.insert(path.to_path_buf());
                }
                if path == root || !path.starts_with(&root) {
                    break;
                }
                parent = path.parent();
            }
        }
        if source.len() + directories.len() > 100_000 {
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
        for path in self.kernel.drain()? {
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
    fn drain(&self) -> Result<BTreeSet<PathBuf>, String> {
        use nix::sys::inotify::AddWatchFlags as F;
        let mut changed = BTreeSet::new();
        loop {
            let events = match self.watch.read_events() {
                Ok(events) => events,
                Err(nix::errno::Errno::EAGAIN) => return Ok(changed),
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
    paths: std::collections::HashMap<usize, PathBuf>,
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
            if !path.exists() {
                continue;
            }
            let fd = nix::fcntl::open(
                path,
                OFlag::O_EVTONLY | OFlag::O_SYMLINK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
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
            paths.insert(ident, path.clone());
            files.push(file);
        }
        Ok(Self {
            queue,
            files,
            paths,
        })
    }
    fn drain(&self) -> Result<BTreeSet<PathBuf>, String> {
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
        loop {
            let count = self
                .queue
                .kevent(
                    &[],
                    &mut events,
                    Some(libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    }),
                )
                .map_err(|e| e.to_string())?;
            if count == 0 {
                return Ok(changed);
            }
            for event in &events[..count] {
                if event.flags().contains(EvFlags::EV_ERROR)
                    || event.fflags().contains(FilterFlag::NOTE_REVOKE)
                {
                    return Err("candidate mutation observation lost coverage".into());
                }
                changed.insert(
                    self.paths
                        .get(&event.ident())
                        .ok_or("unknown candidate watch")?
                        .clone(),
                );
            }
        }
    }
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
    fn drain(&self) -> Result<BTreeSet<PathBuf>, String> {
        Err("candidate mutation observation unavailable".into())
    }
}
