//! Session-owned descriptor watchers and bounded change hints.
mod native;

use crate::{read_error, relative_entry};
use rustx_confined_fs::{FileMetadata, PackageDirectory};
use rustx_execution_protocol::ReadError;
use rustx_execution_protocol::watch::{
    MAX_CONFIGURATION_WATCHES, MAX_WATCH_HINT_BYTES, MAX_WATCH_HINT_PATHS, MAX_WATCHES,
    WatchChange, WatchId, WatchReply, WatchRequest,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const CLEANUP_BUDGET: Duration = Duration::from_millis(250);

pub(super) struct Watches {
    active: Mutex<State>,
    #[cfg(target_os = "macos")]
    children: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct State {
    closed: bool,
    entries: BTreeMap<WatchId, Entry>,
    stopping: BTreeMap<WatchId, mpsc::Receiver<()>>,
}

struct Entry {
    watcher: native::Watcher,
    finished: mpsc::Receiver<()>,
    path: PathBuf,
    expected: String,
    delta: Arc<Mutex<Pending>>,
}

struct Finished {
    receipt: mpsc::Sender<()>,
    #[cfg(all(target_os = "linux", feature = "test-support"))]
    retirement_gate: Option<(File, PathBuf)>,
}

impl Drop for Finished {
    fn drop(&mut self) {
        #[cfg(all(target_os = "linux", feature = "test-support"))]
        if let Some((descriptor, socket)) = self.retirement_gate.take() {
            use std::io::{Read as _, Write as _};
            if let Ok(mut gate) = std::os::unix::net::UnixStream::connect(socket) {
                let _ = gate.set_read_timeout(Some(Duration::from_secs(10)));
                let _ = gate.set_write_timeout(Some(Duration::from_secs(10)));
                if gate.write_all(b"retiring").is_ok() {
                    let _ = gate.read_exact(&mut [0]);
                }
            }
            drop(descriptor);
        }
        let _ = self.receipt.send(());
    }
}

#[derive(Default)]
struct Pending {
    paths: BTreeSet<PathBuf>,
    bytes: usize,
    resync: bool,
    retired: bool,
}

impl Pending {
    fn observe(&mut self, root: &Path, source: &Path, event: notify::Result<notify::Event>) {
        if self.retired {
            return;
        }
        let Ok(event) = event else {
            self.retired = true;
            return;
        };
        if event.need_rescan() {
            self.resync = true;
        } else if matches!(event.kind, notify::EventKind::Access(kind) if !matches!(kind, notify::event::AccessKind::Close(notify::event::AccessMode::Write)))
        {
            return;
        }
        for path in event.paths {
            let Ok(relative) = path.strip_prefix(root) else {
                self.resync = true;
                continue;
            };
            let Some(spelling) = relative.to_str() else {
                self.resync = true;
                continue;
            };
            if !relative.starts_with(source) || relative_entry(relative).is_err() {
                self.resync = true;
                continue;
            }
            // JSON escaping can occupy six bytes per authored byte.
            let charge = spelling.len().saturating_mul(6) + 8;
            if self.paths.contains(relative) {
                continue;
            }
            if self.paths.len() == MAX_WATCH_HINT_PATHS
                || self.bytes + charge > MAX_WATCH_HINT_BYTES
            {
                self.resync = true;
                break;
            }
            self.bytes += charge;
            self.paths.insert(relative.to_path_buf());
        }
    }
}

impl Entry {
    fn stop(self) -> mpsc::Receiver<()> {
        drop(self.watcher);
        self.finished
    }
}

impl Watches {
    pub(super) fn new() -> Self {
        Self {
            active: Mutex::new(State::default()),
            #[cfg(target_os = "macos")]
            children: Arc::new(tokio::sync::Semaphore::new(1024)),
        }
    }

    pub(super) fn dispatch(
        &self,
        root: &PackageDirectory,
        workspace: &Path,
        request: WatchRequest,
    ) -> WatchReply {
        match request {
            WatchRequest::Register { id, path } => match self.register(
                root,
                workspace,
                id,
                path,
                #[cfg(test)]
                |_| {},
            ) {
                Ok(()) => WatchReply::Registered { id },
                Err(error) if error.kind() == io::ErrorKind::TimedOut => WatchReply::Unknown { id },
                Err(error) => WatchReply::Refused {
                    id,
                    kind: read_error(&error),
                },
            },
            WatchRequest::Poll { id } => self.poll(root, id),
        }
    }

    fn register(
        &self,
        root: &PackageDirectory,
        workspace: &Path,
        id: WatchId,
        path: PathBuf,
        #[cfg(test)] mut checkpoint: impl FnMut(RegistrationPhase),
    ) -> io::Result<()> {
        relative_entry(&path).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        if path.as_os_str().len() > 4096 {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let before = root.entry_metadata(&path)?;
        #[cfg(test)]
        checkpoint(RegistrationPhase::BeforeOpen);
        let file = if before.is_file {
            root.file(&path)?
        } else if before.is_directory {
            File::from(root.descend(&path)?.descriptor().try_clone()?)
        } else {
            return Err(io::ErrorKind::PermissionDenied.into());
        };
        if FileMetadata::of(&file)?.identity != before.identity {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let delta = Arc::new(Mutex::new(Pending::default()));
        let pending = delta.clone();
        let (sender, finished) = mpsc::channel();
        let finished_sender = Finished {
            receipt: sender,
            #[cfg(all(target_os = "linux", feature = "test-support"))]
            retirement_gate: if id.is_configuration()
                && std::env::var_os("RUSTX_TEST_WATCH_RETIREMENT_PATH").as_deref()
                    == Some(path.as_os_str())
                && let Some(socket) = std::env::var_os("RUSTX_TEST_WATCH_RETIREMENT_SOCKET")
            {
                Some((file.try_clone()?, PathBuf::from(socket)))
            } else {
                None
            },
        };
        let event_root = workspace.to_path_buf();
        let event_path = path.clone();
        let mut state = self.active.lock().expect("watch registry");
        if state.closed {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let used = state
            .entries
            .keys()
            .chain(state.stopping.keys())
            .filter(|key| key.is_configuration() == id.is_configuration())
            .count();
        let limit = if id.is_configuration() {
            MAX_CONFIGURATION_WATCHES
        } else {
            MAX_WATCHES
        };
        if used >= limit || state.entries.contains_key(&id) || state.stopping.contains_key(&id) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }

        #[cfg(test)]
        checkpoint(RegistrationPhase::BeforeAttach);
        let watcher = native::Watcher::new(
            file,
            workspace.join(&path),
            move |event| {
                let _finished = &finished_sender;
                pending
                    .lock()
                    .expect("watch delta")
                    .observe(&event_root, &event_path, event);
            },
            #[cfg(target_os = "macos")]
            self.children.clone(),
        )?;
        #[cfg(test)]
        checkpoint(RegistrationPhase::AfterAttach);
        let same = root
            .entry_metadata(&path)
            .is_ok_and(|after| after.identity == before.identity);
        let expected = before.identity;
        let entry = Entry {
            watcher,
            finished,
            path,
            expected,
            delta,
        };
        if !same {
            state.stopping.insert(id, entry.stop());
            return if Self::await_stopped(&mut state, id, CLEANUP_BUDGET) {
                Err(io::ErrorKind::PermissionDenied.into())
            } else {
                Err(io::ErrorKind::TimedOut.into())
            };
        }
        state.entries.insert(id, entry);
        Ok(())
    }

    fn poll(&self, root: &PackageDirectory, id: WatchId) -> WatchReply {
        let (path, expected, delta) = {
            let mut state = self.active.lock().expect("watch registry");
            if state.stopping.contains_key(&id) {
                return Self::poll_retirement(&mut state, id);
            }
            let Some(entry) = state.entries.get(&id) else {
                return WatchReply::Refused {
                    id,
                    kind: ReadError::NotFound,
                };
            };
            (
                entry.path.clone(),
                entry.expected.clone(),
                entry.delta.clone(),
            )
        };
        let same = root
            .entry_metadata(&path)
            .is_ok_and(|meta| meta.identity == expected);
        let pending = std::mem::take(&mut *delta.lock().expect("watch delta"));
        if !same || pending.retired {
            let mut state = self.active.lock().expect("watch registry");
            Self::begin_stop(&mut state, id);
            return Self::poll_retirement(&mut state, id);
        }
        WatchReply::Changed(WatchChange {
            id,
            changed_paths: pending.paths.into_iter().collect(),
            resync_required: pending.resync,
            retired: false,
        })
    }

    pub(super) fn stop(&self, id: WatchId) -> bool {
        let mut state = self.active.lock().expect("watch registry");
        Self::begin_stop(&mut state, id);
        Self::await_stopped(&mut state, id, CLEANUP_BUDGET)
    }

    fn begin_stop(state: &mut State, id: WatchId) {
        if let Some(entry) = state.entries.remove(&id) {
            state.stopping.insert(id, entry.stop());
        }
    }

    fn poll_retirement(state: &mut State, id: WatchId) -> WatchReply {
        if let Some(pending) = state.stopping.get(&id) {
            match pending.try_recv() {
                Ok(()) => {
                    state.stopping.remove(&id);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    return WatchReply::Retiring { id };
                }
                Err(mpsc::TryRecvError::Disconnected) => return WatchReply::Unknown { id },
            }
        }
        // Concurrent control stop also removes the entry only after proof.
        WatchReply::Changed(WatchChange {
            id,
            changed_paths: Vec::new(),
            resync_required: true,
            retired: true,
        })
    }

    fn await_stopped(state: &mut State, id: WatchId, budget: Duration) -> bool {
        if let Some(pending) = state.stopping.get(&id) {
            if pending.recv_timeout(budget).is_err() {
                return false;
            }
            state.stopping.remove(&id);
        }
        true
    }

    pub(super) fn shutdown(&self) -> bool {
        let mut state = self.active.lock().expect("watch registry");
        state.closed = true;
        for (id, observer) in std::mem::take(&mut state.entries) {
            state.stopping.insert(id, observer.stop());
        }
        let deadline = std::time::Instant::now() + CLEANUP_BUDGET;
        for id in state.stopping.keys().copied().collect::<Vec<_>>() {
            if !Self::await_stopped(
                &mut state,
                id,
                deadline.saturating_duration_since(std::time::Instant::now()),
            ) {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum RegistrationPhase {
    BeforeOpen,
    BeforeAttach,
    AfterAttach,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::num::NonZeroU64;
    use std::os::unix::fs::symlink;
    use std::time::Instant;

    fn id(id: u64) -> WatchId {
        WatchId::new(NonZeroU64::new(id).unwrap())
    }

    fn wait_for_change(
        watches: &Watches,
        root: &PackageDirectory,
        id: WatchId,
        want: impl Fn(&WatchChange) -> bool,
    ) -> WatchChange {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let notice = watches.poll(root, id);
            match &notice {
                WatchReply::Changed(change) if want(change) => return change.clone(),
                WatchReply::Changed(_) | WatchReply::Retiring { .. } => (),
                _ => panic!("watch disappeared before a physical change was reported: {notice:?}"),
            }
            assert!(
                Instant::now() < deadline,
                "watch did not publish the expected change: {notice:?}"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn pending_native_retirement_keeps_independent_watches_live() {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let path = base.join("leaf");
        fs::write(&path, b"original").unwrap();
        fs::create_dir(base.join("directory")).unwrap();
        let root = PackageDirectory::open(&base).unwrap();
        let watches = Watches::new();
        let leaf = id(1);
        let directory = id(2);
        watches
            .register(&root, &base, leaf, "leaf".into(), |_| {})
            .unwrap();
        watches
            .register(&root, &base, directory, "directory".into(), |_| {})
            .unwrap();
        // Withhold the stopped observer's cleanup receipt after replacing its path.
        let (finish, pending) = mpsc::channel();
        watches
            .active
            .lock()
            .unwrap()
            .entries
            .get_mut(&leaf)
            .unwrap()
            .finished = pending;
        fs::rename(&path, base.join("previous")).unwrap();
        fs::write(&path, b"replacement").unwrap();
        let busy = WatchReply::Retiring { id: leaf };
        assert_eq!(watches.poll(&root, leaf), busy);
        assert_eq!(watches.poll(&root, leaf), busy);
        assert_eq!(
            watches
                .register(&root, &base, leaf, "leaf".into(), |_| {})
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );

        fs::write(base.join("directory/new"), b"independent change").unwrap();
        wait_for_change(&watches, &root, directory, |notice| {
            assert!(!notice.retired);
            notice
                .changed_paths
                .contains(&PathBuf::from("directory/new"))
                || cfg!(target_os = "macos") && notice.resync_required
        });

        finish.send(()).unwrap();
        assert_eq!(
            watches.poll(&root, leaf),
            WatchReply::Changed(WatchChange {
                id: leaf,
                changed_paths: Vec::new(),
                resync_required: true,
                retired: true,
            })
        );
        assert_eq!(
            watches.poll(&root, leaf),
            WatchReply::Refused {
                id: leaf,
                kind: ReadError::NotFound
            }
        );
        assert!(watches.stop(directory));
    }

    #[test]
    fn registration_keeps_the_captured_descriptor_across_name_replacement() {
        for (link, directory) in [(false, false), (true, false), (false, true), (true, true)] {
            let tmp = tempfile::tempdir().unwrap();
            let base = tmp.path().canonicalize().unwrap().join("workspace");
            fs::create_dir(&base).unwrap();
            let original = base.join("source");
            let saved = base.join("captured");
            let outside = tmp.path().join("outside");
            for path in [&original, &outside] {
                if directory {
                    fs::create_dir(path).unwrap();
                } else {
                    fs::write(path, b"initial").unwrap();
                }
            }
            let root = PackageDirectory::open(&base).unwrap();
            let watches = Watches::new();
            watches
                .register(&root, &base, id(1), "source".into(), |phase| match phase {
                    RegistrationPhase::BeforeOpen => {}
                    RegistrationPhase::BeforeAttach => {
                        fs::rename(&original, &saved).unwrap();
                        if link {
                            symlink(&outside, &original).unwrap();
                        } else {
                            fs::rename(&outside, &original).unwrap();
                        }
                    }
                    RegistrationPhase::AfterAttach => {
                        if link {
                            fs::remove_file(&original).unwrap();
                        } else {
                            fs::rename(&original, &outside).unwrap();
                        }
                        fs::rename(&saved, &original).unwrap();
                    }
                })
                .unwrap();
            let sentinel = if directory {
                fs::write(outside.join("private"), b"outside").unwrap();
                original.join("inside")
            } else {
                fs::write(&outside, b"outside").unwrap();
                original.clone()
            };
            fs::write(&sentinel, b"authorized").unwrap();
            let expected = sentinel.strip_prefix(&base).unwrap();
            wait_for_change(&watches, &root, id(1), |notice| {
                assert!(!notice.retired);
                assert!(
                    notice
                        .changed_paths
                        .iter()
                        .all(|path| { path.starts_with("source") && !path.ends_with("private") })
                );
                notice.changed_paths.iter().any(|path| path == expected)
                    || (cfg!(target_os = "macos")
                        && directory
                        && notice.resync_required
                        && notice
                            .changed_paths
                            .iter()
                            .any(|path| path == Path::new("source")))
            });
            assert!(watches.stop(id(1)));
            assert!(watches.active.lock().unwrap().entries.is_empty());
        }
    }

    #[test]
    fn replacing_the_source_before_descriptor_capture_refuses_registration() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap().join("workspace");
        fs::create_dir(&base).unwrap();
        fs::create_dir(base.join("source")).unwrap();
        fs::create_dir(dir.path().join("replacement")).unwrap();
        let root = PackageDirectory::open(&base).unwrap();
        let watches = Watches::new();
        let error = watches
            .register(&root, &base, id(1), "source".into(), |phase| {
                assert!(matches!(phase, RegistrationPhase::BeforeOpen));
                fs::rename(base.join("source"), base.join("saved")).unwrap();
                fs::rename(dir.path().join("replacement"), base.join("source")).unwrap();
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(matches!(
            watches.poll(&root, id(1)),
            WatchReply::Refused {
                kind: ReadError::NotFound,
                ..
            }
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_child_descriptors_are_bounded_through_refresh_and_retirement() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        fs::create_dir(base.join("source")).unwrap();
        fs::write(base.join("source/file"), b"initial").unwrap();
        fs::create_dir(base.join("source/directory")).unwrap();
        symlink(&base, base.join("source/link")).unwrap();
        let root = PackageDirectory::open(&base).unwrap();
        let watches = Watches::new();
        let reserved = watches
            .children
            .clone()
            .try_acquire_many_owned(1021)
            .unwrap();
        let register = |id| watches.register(&root, &base, id, "source".into(), |_| {});
        register(id(1)).unwrap();
        assert_eq!(watches.children.available_permits(), 1);
        fs::write(base.join("source/created"), b"new").unwrap();
        wait_for_change(&watches, &root, id(1), |notice| notice.resync_required);
        assert_eq!(watches.children.available_permits(), 0);
        fs::remove_file(base.join("source/created")).unwrap();
        wait_for_change(&watches, &root, id(1), |notice| notice.resync_required);
        assert_eq!(watches.children.available_permits(), 1);
        assert!(watches.stop(id(1)));
        assert_eq!(watches.children.available_permits(), 3);

        let held = watches.children.clone().try_acquire_owned().unwrap();
        fs::write(base.join("source/created"), b"third").unwrap();
        assert_eq!(
            register(id(2)).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(watches.children.available_permits(), 2);
        fs::remove_file(base.join("source/created")).unwrap();
        register(id(3)).unwrap();
        assert_eq!(watches.children.available_permits(), 0);
        fs::write(base.join("source/created"), b"overflow").unwrap();
        let retired = wait_for_change(&watches, &root, id(3), |notice| notice.retired);
        assert!(retired.resync_required && retired.changed_paths.is_empty());
        assert_eq!(watches.children.available_permits(), 2);
        drop((held, reserved));
        assert_eq!(watches.children.available_permits(), 1024);
    }
}
