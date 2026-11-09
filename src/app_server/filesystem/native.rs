//! Native observers bind retained descriptors, never mutable registration paths.
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
pub(super) struct Watcher {
    _watcher: notify::RecommendedWatcher,
}
#[cfg(target_os = "linux")]
impl Watcher {
    pub(super) fn new(
        source: File,
        authorized: PathBuf,
        mut handler: impl FnMut(notify::Result<notify::Event>) + Send + 'static,
    ) -> io::Result<Self> {
        use notify::Watcher as _;
        let descriptor_path = PathBuf::from(format!("/proc/self/fd/{}", source.as_raw_fd()));
        #[cfg(test)]
        let registered = (authorized.clone(), source.try_clone()?);
        let prefix = descriptor_path.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                // notify drops its handle before its native event loop exits. Keep
                // the source alive in that loop, alongside the callback admission.
                let _source = &source;
                handler(event.map(|mut event| {
                    let mut invalid = false;
                    event.paths = event
                        .paths
                        .into_iter()
                        .filter_map(|path| {
                            if let Ok(relative) = path.strip_prefix(&prefix) {
                                Some(authorized.join(relative))
                            } else {
                                invalid = true;
                                None
                            }
                        })
                        .collect();
                    if invalid {
                        event = event.set_flag(notify::event::Flag::Rescan);
                    }
                    event
                }));
            })
            .map_err(io::Error::other)?;
        watcher
            .watch(&descriptor_path, notify::RecursiveMode::NonRecursive)
            .map_err(io::Error::other)?;
        #[cfg(test)]
        super::native_registered(&registered.0, &registered.1);
        Ok(Self { _watcher: watcher })
    }
}

#[cfg(target_os = "macos")]
pub(super) struct Watcher(std::sync::Arc<nix::sys::event::Kqueue>);
#[cfg(target_os = "macos")]
impl Drop for Watcher {
    fn drop(&mut self) {
        use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent};
        let _ = self.0.kevent(
            &[KEvent::new(
                0,
                EventFilter::EVFILT_USER,
                EvFlags::empty(),
                FilterFlag::NOTE_TRIGGER,
                0,
                0,
            )],
            &mut [],
            Some(libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            }),
        );
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{AsRawFd, File, PathBuf, Watcher, io};
    use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::Arc;
    use tokio::sync::{OwnedSemaphorePermit, Semaphore};

    struct Child {
        file: File,
        _capacity: OwnedSemaphorePermit,
    }
    fn registration(file: &File, child: bool) -> io::Result<KEvent> {
        let mut flags = FilterFlag::NOTE_ATTRIB
            | FilterFlag::NOTE_RENAME
            | FilterFlag::NOTE_DELETE
            | FilterFlag::NOTE_REVOKE;
        // Child directories contribute their own metadata changes, never
        // namespace changes to grandchildren. The root owns namespace hints.
        if !child || !file.metadata()?.is_dir() {
            flags |= FilterFlag::NOTE_WRITE | FilterFlag::NOTE_EXTEND | FilterFlag::NOTE_LINK;
        }
        Ok(KEvent::new(
            usize::try_from(file.as_raw_fd()).map_err(io::Error::other)?,
            EventFilter::EVFILT_VNODE,
            EvFlags::EV_ADD | EvFlags::EV_CLEAR,
            flags,
            0,
            0,
        ))
    }
    fn refresh(
        queue: &Kqueue,
        source: &File,
        children: &mut BTreeMap<OsString, Child>,
        capacity: &Arc<Semaphore>,
    ) -> io::Result<()> {
        if !source.metadata()?.is_dir() {
            return Ok(());
        }
        let mut names = std::collections::BTreeSet::new();
        for entry in rustix::fs::Dir::read_from(source)? {
            let entry = entry?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            if names.len() == 4096 {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "watch directory capacity",
                ));
            }
            names.insert(std::ffi::OsStr::from_bytes(name).to_os_string());
        }
        // Closing an old child removes its vnode filter before its descriptor
        // can be reused. Never retain a moved child as a new namespace member.
        children.retain(|name, child| {
            names.contains(name)
                && rustix::fs::statat(source, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
                    .is_ok_and(|current| {
                        rustix::fs::fstat(&child.file).is_ok_and(|retained| {
                            current.st_dev == retained.st_dev && current.st_ino == retained.st_ino
                        })
                    })
        });
        for name in names {
            if children.contains_key(&name) {
                continue;
            }
            let metadata =
                rustix::fs::statat(source, &name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)?;
            if !matches!(
                metadata.st_mode & libc::S_IFMT,
                libc::S_IFREG | libc::S_IFDIR
            ) {
                continue;
            }
            let permit = capacity
                .clone()
                .try_acquire_owned()
                .map_err(|_| io::Error::new(io::ErrorKind::AlreadyExists, "watch file capacity"))?;
            let file = File::from(rustix::fs::openat(
                source,
                &name,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOCTTY,
                rustix::fs::Mode::empty(),
            )?);
            let opened = rustix::fs::fstat(&file)?;
            let current = rustix::fs::statat(source, &name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)?;
            if !matches!(opened.st_mode & libc::S_IFMT, libc::S_IFREG | libc::S_IFDIR)
                || (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino)
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "watch child replaced",
                ));
            }
            queue.kevent(
                &[registration(&file, true)?],
                &mut [],
                Some(libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                }),
            )?;
            children.insert(
                name,
                Child {
                    file,
                    _capacity: permit,
                },
            );
        }
        Ok(())
    }
    fn observe(
        queue: Arc<Kqueue>,
        source: File,
        authorized: &std::path::Path,
        mut children: BTreeMap<OsString, Child>,
        capacity: &Arc<Semaphore>,
        mut handler: impl FnMut(notify::Result<notify::Event>),
    ) {
        let mut events = [registration(&source, false).expect("registered source"); 32];
        loop {
            let result = queue.kevent(&[], &mut events, None);
            match result {
                Ok(0) | Err(nix::errno::Errno::EINTR) => {}
                Ok(count) => {
                    if events[..count]
                        .iter()
                        .any(|event| event.filter() == Ok(EventFilter::EVFILT_USER))
                    {
                        break;
                    }
                    let observed: Vec<_> = children
                        .iter()
                        .filter_map(|(name, child)| {
                            let descriptor = usize::try_from(child.file.as_raw_fd()).ok()?;
                            if !events[..count]
                                .iter()
                                .any(|event| event.ident() == descriptor)
                            {
                                return None;
                            }
                            let identity = rustix::fs::fstat(&child.file).ok()?;
                            Some((name.clone(), (identity.st_dev, identity.st_ino)))
                        })
                        .collect();
                    if events[..count]
                        .iter()
                        .any(|event| event.flags().contains(EvFlags::EV_ERROR))
                        || refresh(&queue, &source, &mut children, capacity).is_err()
                    {
                        handler(Err(notify::Error::generic("descriptor watch retired")));
                        break;
                    }
                    // Name only descriptor identities still present after
                    // reconciliation; fd reuse must not relabel old events.
                    let mut paths: Vec<_> = observed
                        .into_iter()
                        .filter_map(|(name, identity)| {
                            let child = children.get(&name)?;
                            let retained = rustix::fs::fstat(&child.file).ok()?;
                            (identity == (retained.st_dev, retained.st_ino))
                                .then(|| authorized.join(name))
                        })
                        .collect();
                    if paths.is_empty()
                        || events[..count].iter().any(|event| {
                            usize::try_from(source.as_raw_fd())
                                .is_ok_and(|source| source == event.ident())
                        })
                    {
                        paths.push(authorized.to_path_buf());
                    }
                    let mut event = notify::Event::new(notify::EventKind::Any)
                        .set_flag(notify::event::Flag::Rescan);
                    event.paths = paths;
                    handler(Ok(event));
                }
                Err(error) => {
                    handler(Err(notify::Error::io(error.into())));
                    break;
                }
            }
        }
        // Release all physical resources before the handler's retained
        // observer capacity proves native settlement to Host shutdown.
        drop(children);
        drop(queue);
        drop(source);
        drop(handler);
    }
    impl Watcher {
        pub(in crate::app_server::filesystem) fn new(
            source: File,
            authorized: PathBuf,
            handler: impl FnMut(notify::Result<notify::Event>) + Send + 'static,
            capacity: Arc<Semaphore>,
        ) -> io::Result<Self> {
            let queue = Arc::new(Kqueue::new()?);
            nix::fcntl::fcntl(
                queue.as_ref(),
                nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC),
            )?;
            queue.kevent(
                &[
                    registration(&source, false)?,
                    KEvent::new(
                        0,
                        EventFilter::EVFILT_USER,
                        EvFlags::EV_ADD | EvFlags::EV_CLEAR,
                        FilterFlag::empty(),
                        0,
                        0,
                    ),
                ],
                &mut [],
                Some(libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                }),
            )?;
            let mut children = BTreeMap::new();
            refresh(&queue, &source, &mut children, &capacity)?;
            #[cfg(test)]
            super::super::native_registered(&authorized, &source);
            let retained_queue = queue.clone();
            std::thread::Builder::new()
                .name("rustx descriptor watch".into())
                .spawn(move || {
                    observe(
                        retained_queue,
                        source,
                        &authorized,
                        children,
                        &capacity,
                        handler,
                    );
                })?;
            Ok(Self(queue))
        }
    }
}
