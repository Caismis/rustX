//! Bounded connection-owned filesystem hints; reads remain authoritative.
use super::protocol::{ErrorData, NotificationMethod, RequestId, RpcError};
use super::rpc_error::domain;
mod native;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Changed {
    pub watch_id: String,
    pub changed_paths: Vec<std::path::PathBuf>,
    pub resync_required: bool,
    pub retired: bool,
}
struct Entry {
    watcher: Option<native::Watcher>,
    stop: CancellationToken,
    publishing: Arc<Mutex<bool>>,
    generation: Arc<()>,
    #[cfg(test)]
    retirement_queued: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Entry {
    fn stop_publication(&self) {
        let mut active = self.publishing.lock().expect("watch publication");
        *active = false;
        self.stop.cancel();
    }
}
#[derive(Default)]
struct State {
    closed: bool,
    active: BTreeMap<String, Entry>,
}
#[derive(Default)]
pub(super) struct Watches(Mutex<State>);
impl Watches {
    #[cfg(test)]
    pub(crate) fn observe_retirement(&self, watch_id: &str) -> tokio::sync::oneshot::Receiver<()> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.0
            .lock()
            .unwrap()
            .active
            .get_mut(watch_id)
            .unwrap()
            .retirement_queued = Some(sender);
        receiver
    }
    pub fn close(&self) {
        let mut state = self.0.lock().expect("filesystem watches");
        state.closed = true;
        for entry in state.active.values() {
            entry.stop_publication();
        }
        let active = std::mem::take(&mut state.active);
        drop(state);
        drop(active);
    }
    pub fn unwatch(&self, watch_id: &str) -> Result<(), RpcError> {
        let mut state = self.0.lock().expect("filesystem watches");
        state
            .active
            .get(watch_id)
            .ok_or_else(|| domain(ErrorData::EnvironmentResourceNotFound))?
            .stop_publication();
        // Reusing the identity is possible only after the old publication
        // gate and transport fence have both closed. Native Drop stays outside.
        let entry = state.active.remove(watch_id);
        drop(state);
        drop(entry);
        Ok(())
    }
    pub async fn watch(
        self: &Arc<Self>,
        watch_id: String,
        path: String,
        environment: Arc<crate::local_runtime::environment::Environment>,
        events: Arc<super::service_events::Events>,
        capacity: Arc<tokio::sync::Semaphore>,
        owner_lease: super::host::ServerOperation,
    ) -> Result<(), RpcError> {
        if watch_id.is_empty() || !RequestId::is_valid_string(&watch_id) {
            return Err(domain(ErrorData::InvalidParams));
        }
        let permit = capacity
            .try_acquire_owned()
            .map_err(|_| domain(ErrorData::EnvironmentCapacityOrConflict))?;
        let owner = self.clone();
        tokio::task::spawn_blocking(move || {
            let _owner_lease=owner_lease;
            let capacity=Arc::new(permit);
            environment.with_paths(&[&path], |paths| {
                let (absolute, root, relative) = &paths[0];
                #[cfg(test)]
                registration_phase(absolute, 0);
                let (source, before) = crate::local_runtime::environment::files::watch_source(root, relative)?;
                let root_retired = root.cancelled.clone();
                let source_retired = CancellationToken::new();
                let stop = CancellationToken::new();
                let publishing = Arc::new(Mutex::new(true));
                let publication = publishing.clone();
                let stopped = stop.clone();
                let identity = watch_id.clone();
                let output = events.clone();
                let authorized = absolute.clone();
                let native = environment.clone();
                let expected = before.identity.clone();
                let invalidated = source_retired.clone();
                let armed = Arc::new(AtomicBool::new(false));
                let active = armed.clone();
                let retained_capacity=capacity.clone();
                let recheck = Arc::new(tokio::sync::Notify::new());
                let pending_recheck = recheck.clone();
                let recheck_environment = environment.clone();
                let recheck_path = authorized.clone();
                let recheck_identity = expected.clone();
                #[cfg(test)]
                registration_phase(absolute, 1);
                let watcher = native::Watcher::new(source, absolute.clone(), move |event: notify::Result<notify::Event>| {
                    let _retained_capacity=&retained_capacity;
                    if stopped.is_cancelled() || root_retired.is_cancelled() { return; }
                    if event.is_err() { invalidated.cancel(); return; }
                    if !active.load(Ordering::Acquire) { return; }
                    // Metadata authority checks open directories themselves. Open/read/
                    // close notifications must not feed those checks back into notify.
                    if event.as_ref().is_ok_and(|event| !relevant(event)) {
                        #[cfg(test)]
                        ignored_access(&authorized);
                        return;
                    }
                    #[cfg(test)]
                    let observed_change = native_change(&authorized, event.as_ref().expect("native event"));
                    match native.try_metadata(authorized.to_str().expect("validated UTF-8 path")) {
                        Ok(metadata) if metadata.identity==expected=>{},
                        Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=> {
                            // One pending permit coalesces native events. The
                            // existing watch owner revisits identity after I/O,
                            // even if this inode never produces another event.
                            pending_recheck.notify_one();
                            #[cfg(test)]
                            recheck_scheduled(&authorized);
                            publish_change(&publication, &stopped, &output, Changed {watch_id:identity.clone(),changed_paths:Vec::new(),resync_required:true,retired:false});
                            #[cfg(test)]
                            if let Some(completed) = observed_change { let _ = completed.send(()); }
                            return;
                        },
                        _=> {invalidated.cancel();return;},
                    }
                    #[cfg(test)]
                    let publication_completed = before_publication(&authorized);
                    let (changed_paths, resync_required) = match event {
                        Ok(event) => {
                            let rescan = event.need_rescan() || event.paths.len() > 16 || event.paths.iter().any(|path|path.to_str().is_none() || !path.starts_with(&authorized));
                            (event.paths.into_iter().filter(|path| path.to_str().is_some() && path.starts_with(&authorized)).take(16).collect(), rescan)
                        },
                        Err(_) => (Vec::new(), true),
                    };
                    publish_change(&publication, &stopped, &output, Changed { watch_id: identity.clone(), changed_paths, resync_required, retired: false });
                    #[cfg(test)]
                    if let Some(completed) = publication_completed { let _ = completed.send(()); }
                    #[cfg(test)]
                    if let Some(completed) = observed_change { let _ = completed.send(()); }
                }, #[cfg(target_os = "macos")] environment.watch_files.clone())?;
                #[cfg(test)]
                registration_phase(absolute, 2);
                // Registration uses the retained descriptor, so even a pathname
                // replacement restored before this check cannot redirect it.
                if crate::local_runtime::environment::files::metadata(root,relative)?.identity != before.identity { return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,"watch source replaced")); }
                let generation = Arc::new(());
                let mut state = owner.0.lock().expect("filesystem watches");
                if state.closed || events.closed.is_cancelled() { return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "connection closed")); }
                if state.active.len() >= 8 || state.active.contains_key(&watch_id) { return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "watch capacity or identity conflict")); }
                state.active.insert(watch_id.clone(), Entry { watcher: Some(watcher), stop: stop.clone(), publishing: publishing.clone(), generation:generation.clone(), #[cfg(test)] retirement_queued: None });
                drop(state);
                armed.store(true,Ordering::Release);
                let root_retired = root.cancelled.clone();
                let registry = Arc::downgrade(&owner);
                tokio::runtime::Handle::current().spawn(async move {
                    let _capacity=capacity.clone();
                    tokio::select! {
                        () = stop.cancelled() => return,
                        () = events.closed.cancelled() => return,
                        () = root_retired.cancelled() => {},
                        () = source_retired.cancelled() => {},
                        () = source_identity_lost(recheck_environment, recheck_path, recheck_identity, recheck, capacity) => {},
                    }
                    retire_watch(registry, watch_id, generation, publishing, stop, events).await;
                });
                Ok(())
            }).map_err(super::connection::environment_error)
        }).await.map_err(|_| domain(ErrorData::OutcomeUnknown))?
    }
}

fn publish_change(
    publishing: &Mutex<bool>,
    stop: &CancellationToken,
    events: &super::service_events::Events,
    change: Changed,
) {
    let active = publishing.lock().expect("watch publication");
    if *active && !stop.is_cancelled() {
        // Keep the short gate through queue publication. Retirement closes
        // this gate before its final notice; existing events remain FIFO.
        let _ = events.emit_fenced(NotificationMethod::FsChanged(change), Some(stop.clone()));
    }
}

async fn retire_watch(
    registry: std::sync::Weak<Watches>,
    watch_id: String,
    generation: Arc<()>,
    publishing: Arc<Mutex<bool>>,
    stop: CancellationToken,
    events: Arc<super::service_events::Events>,
) {
    // Closing callback publication is separate from withdrawing the terminal
    // transport fence. Native Drop and receipt waits never hold this gate.
    *publishing.lock().expect("watch publication") = false;
    if let Some(owner) = registry.upgrade() {
        let watcher = {
            let mut state = owner.0.lock().expect("filesystem watches");
            state
                .active
                .get_mut(&watch_id)
                .filter(|entry| Arc::ptr_eq(&entry.generation, &generation))
                .and_then(|entry| entry.watcher.take())
        };
        drop(watcher);
    }
    if let Ok(receipt) = events.emit_fenced(
        NotificationMethod::FsChanged(Changed {
            watch_id: watch_id.clone(),
            changed_paths: Vec::new(),
            resync_required: true,
            retired: true,
        }),
        Some(stop.clone()),
    ) {
        #[cfg(test)]
        if let Some(owner) = registry.upgrade() {
            let queued = owner
                .0
                .lock()
                .unwrap()
                .active
                .get_mut(&watch_id)
                .filter(|entry| Arc::ptr_eq(&entry.generation, &generation))
                .and_then(|entry| entry.retirement_queued.take());
            if let Some(queued) = queued {
                let _ = queued.send(());
            }
        }
        // Unwatch withdraws this fenced notice. It must also release its
        // receipt wait when the live client no longer reads notifications.
        tokio::select! {
            () = stop.cancelled() => {},
            _ = events.emitted(receipt) => {},
        }
    }
    if let Some(owner) = registry.upgrade() {
        let removed = {
            let mut state = owner.0.lock().expect("filesystem watches");
            if state
                .active
                .get(&watch_id)
                .is_some_and(|entry| Arc::ptr_eq(&entry.generation, &generation))
            {
                state.active.remove(&watch_id)
            } else {
                None
            }
        };
        if let Some(entry) = removed {
            entry.stop.cancel();
        }
    }
}

async fn source_identity_lost(
    environment: Arc<crate::local_runtime::environment::Environment>,
    path: std::path::PathBuf,
    expected: String,
    pending: Arc<tokio::sync::Notify>,
    capacity: Arc<tokio::sync::OwnedSemaphorePermit>,
) {
    loop {
        pending.notified().await;
        let native = environment.clone();
        let path = path.clone();
        let retained = capacity.clone();
        let checked = tokio::task::spawn_blocking(move || {
            // A cancelled observer still owns this physical check until it
            // returns; Host teardown waits for the same capacity permit.
            let _retained = retained;
            native.with_paths(&[path.to_str().expect("validated UTF-8 path")], |paths| {
                #[cfg(test)]
                recheck_locked(&path);
                crate::local_runtime::environment::files::metadata(&paths[0].1, &paths[0].2)
            })
        })
        .await;
        if !matches!(checked, Ok(Ok(metadata)) if metadata.identity == expected) {
            return;
        }
    }
}

fn relevant(event: &notify::Event) -> bool {
    event.need_rescan()
        || !matches!(event.kind, notify::EventKind::Access(kind) if !matches!(kind, notify::event::AccessKind::Close(notify::event::AccessMode::Write)))
}

#[cfg(test)]
struct PublicationPause {
    entered: tokio::sync::oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
    completed: tokio::sync::oneshot::Sender<()>,
}
#[cfg(test)]
static PUBLICATIONS: std::sync::LazyLock<Mutex<BTreeMap<std::path::PathBuf, PublicationPause>>> =
    std::sync::LazyLock::new(Mutex::default);
#[cfg(test)]
pub(crate) fn pause_publication(
    path: std::path::PathBuf,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
) {
    let (entered, at_publication) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let (completed, after_publication) = tokio::sync::oneshot::channel();
    PUBLICATIONS.lock().unwrap().insert(
        path,
        PublicationPause {
            entered,
            release: released,
            completed,
        },
    );
    (at_publication, release, after_publication)
}
#[cfg(test)]
fn before_publication(path: &std::path::Path) -> Option<tokio::sync::oneshot::Sender<()>> {
    let pause = PUBLICATIONS.lock().unwrap().remove(path)?;
    let _ = pause.entered.send(());
    // Dropping a failed test's sender also releases the physical callback.
    let _ = pause.release.recv();
    Some(pause.completed)
}

#[cfg(test)]
type NativeChanges =
    BTreeMap<std::path::PathBuf, (std::path::PathBuf, tokio::sync::oneshot::Sender<()>)>;
#[cfg(test)]
static NATIVE_CHANGES: std::sync::LazyLock<Mutex<NativeChanges>> =
    std::sync::LazyLock::new(Mutex::default);
#[cfg(test)]
pub(crate) fn observe_native_change(
    source: std::path::PathBuf,
    changed: std::path::PathBuf,
) -> tokio::sync::oneshot::Receiver<()> {
    let (published, publication) = tokio::sync::oneshot::channel();
    NATIVE_CHANGES
        .lock()
        .unwrap()
        .insert(source, (changed, published));
    publication
}
#[cfg(test)]
fn native_change(
    source: &std::path::Path,
    event: &notify::Event,
) -> Option<tokio::sync::oneshot::Sender<()>> {
    let mut changes = NATIVE_CHANGES.lock().unwrap();
    let (changed, _) = changes.get(source)?;
    if !event.paths.contains(changed) {
        return None;
    }
    changes.remove(source).map(|(_, published)| published)
}

#[cfg(test)]
type RecheckHooks = BTreeMap<std::path::PathBuf, tokio::sync::oneshot::Sender<()>>;
#[cfg(test)]
static RECHECKS: std::sync::LazyLock<Mutex<RecheckHooks>> =
    std::sync::LazyLock::new(Mutex::default);
#[cfg(test)]
pub(crate) fn observe_recheck(path: std::path::PathBuf, sender: tokio::sync::oneshot::Sender<()>) {
    RECHECKS.lock().unwrap().insert(path, sender);
}
#[cfg(test)]
fn recheck_scheduled(path: &std::path::Path) {
    let sender = RECHECKS.lock().unwrap().remove(path);
    if let Some(sender) = sender {
        let _ = sender.send(());
    }
}

#[cfg(test)]
static RECHECK_LOCKED: std::sync::LazyLock<Mutex<BTreeMap<std::path::PathBuf, RegistrationHook>>> =
    std::sync::LazyLock::new(Mutex::default);
#[cfg(test)]
pub(crate) fn hold_recheck(path: std::path::PathBuf, hook: RegistrationHook) {
    RECHECK_LOCKED.lock().unwrap().insert(path, hook);
}
#[cfg(test)]
fn recheck_locked(path: &std::path::Path) {
    let hook = RECHECK_LOCKED.lock().unwrap().remove(path);
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(test)]
static IGNORED_ACCESS: std::sync::LazyLock<
    std::sync::Mutex<
        std::collections::BTreeMap<std::path::PathBuf, tokio::sync::oneshot::Sender<()>>,
    >,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::BTreeMap::new()));
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn observe_access(path: std::path::PathBuf, sender: tokio::sync::oneshot::Sender<()>) {
    IGNORED_ACCESS.lock().unwrap().insert(path, sender);
}
#[cfg(test)]
fn ignored_access(path: &std::path::Path) {
    let sender = IGNORED_ACCESS.lock().unwrap().remove(path);
    if let Some(sender) = sender {
        let _ = sender.send(());
    }
}

#[cfg(test)]
type RegistrationHook = Box<dyn FnOnce() + Send>;
#[cfg(test)]
type RegistrationHooks = BTreeMap<std::path::PathBuf, [Option<RegistrationHook>; 3]>;
#[cfg(test)]
static REGISTRATIONS: std::sync::LazyLock<Mutex<RegistrationHooks>> =
    std::sync::LazyLock::new(Mutex::default);
#[cfg(test)]
pub(crate) fn registration_hooks(
    path: std::path::PathBuf,
    before: RegistrationHook,
    after: RegistrationHook,
) {
    REGISTRATIONS
        .lock()
        .unwrap()
        .insert(path, [None, Some(before), Some(after)]);
}
#[cfg(test)]
pub(crate) fn capture_hook(path: &std::path::Path, capture: RegistrationHook) {
    REGISTRATIONS.lock().unwrap().get_mut(path).unwrap()[0] = Some(capture);
}
#[cfg(test)]
fn registration_phase(path: &std::path::Path, phase: usize) {
    let hook = {
        let mut hooks = REGISTRATIONS.lock().unwrap();
        let hook = hooks.get_mut(path).and_then(|hooks| hooks[phase].take());
        if phase == 2 {
            hooks.remove(path);
        }
        hook
    };
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(test)]
static EXPECTED_SOURCES: std::sync::LazyLock<
    Mutex<BTreeMap<std::path::PathBuf, std::fs::Metadata>>,
> = std::sync::LazyLock::new(Mutex::default);
#[cfg(test)]
pub(crate) fn expect_native_source(path: std::path::PathBuf, metadata: std::fs::Metadata) {
    EXPECTED_SOURCES.lock().unwrap().insert(path, metadata);
}
#[cfg(test)]
fn native_registered(path: &std::path::Path, source: &std::fs::File) {
    use std::os::unix::fs::MetadataExt;
    if let Some(expected) = EXPECTED_SOURCES.lock().unwrap().remove(path) {
        let actual = source.metadata().unwrap();
        assert_eq!(
            (actual.dev(), actual.ino()),
            (expected.dev(), expected.ino()),
            "the descriptor passed to native registration must name the authorized source"
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn authority_inspection_access_events_do_not_feed_watchers() {
        use notify::{
            Event, EventKind,
            event::{AccessKind, AccessMode, CreateKind},
        };
        for kind in [
            AccessKind::Open(AccessMode::Read),
            AccessKind::Read,
            AccessKind::Close(AccessMode::Read),
        ] {
            assert!(!super::relevant(&Event::new(EventKind::Access(kind))));
        }
        assert!(super::relevant(&Event::new(EventKind::Access(
            AccessKind::Close(AccessMode::Write)
        ))));
        assert!(super::relevant(&Event::new(EventKind::Create(
            CreateKind::File
        ))));
        let event =
            Event::new(EventKind::Access(AccessKind::Any)).set_flag(notify::event::Flag::Rescan);
        assert!(super::relevant(&event));
    }
}
