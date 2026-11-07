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
    generation: Arc<()>,
}
#[derive(Default)]
struct State {
    closed: bool,
    active: BTreeMap<String, Entry>,
}
#[derive(Default)]
pub(super) struct Watches(Mutex<State>);
impl Watches {
    pub fn close(&self) {
        let mut state = self.0.lock().expect("filesystem watches");
        state.closed = true;
        let active = std::mem::take(&mut state.active);
        drop(state);
        for (_, entry) in active {
            entry.stop.cancel();
        }
    }
    pub fn unwatch(&self, watch_id: &str) -> Result<(), RpcError> {
        let mut state = self.0.lock().expect("filesystem watches");
        let entry = state
            .active
            .remove(watch_id)
            .ok_or_else(|| domain(ErrorData::EnvironmentResourceNotFound))?;
        drop(state);
        entry.stop.cancel();
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
                    match native.try_metadata(authorized.to_str().expect("validated UTF-8 path")) {
                        Ok(metadata) if metadata.identity==expected=>{},
                        Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=> {
                            let _=output.emit_fenced(NotificationMethod::FsChanged(Changed {watch_id:identity.clone(),changed_paths:Vec::new(),resync_required:true,retired:false}),Some(stopped.clone()));
                            return;
                        },
                        _=> {invalidated.cancel();return;},
                    }
                    let (changed_paths, resync_required) = match event {
                        Ok(event) => {
                            let rescan = event.need_rescan() || event.paths.len() > 16 || event.paths.iter().any(|path|path.to_str().is_none() || !path.starts_with(&authorized));
                            (event.paths.into_iter().filter(|path| path.to_str().is_some() && path.starts_with(&authorized)).take(16).collect(), rescan)
                        },
                        Err(_) => (Vec::new(), true),
                    };
                    let _ = output.emit_fenced(NotificationMethod::FsChanged(Changed { watch_id: identity.clone(), changed_paths, resync_required, retired: false }),Some(stopped.clone()));
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
                state.active.insert(watch_id.clone(), Entry { watcher: Some(watcher), stop: stop.clone(), generation:generation.clone() });
                drop(state);
                armed.store(true,Ordering::Release);
                let root_retired = root.cancelled.clone();
                let registry = Arc::downgrade(&owner);
                tokio::runtime::Handle::current().spawn(async move {
                    let _capacity=capacity;
                    tokio::select! { () = stop.cancelled() => return, () = events.closed.cancelled() => return, () = root_retired.cancelled() => {}, () = source_retired.cancelled() => {} }
                    if let Some(owner) = registry.upgrade() {
                        let watcher = {
                            let mut state = owner.0.lock().expect("filesystem watches");
                            state.active.get_mut(&watch_id).filter(|entry|Arc::ptr_eq(&entry.generation,&generation)).and_then(|entry|entry.watcher.take())
                        };
                        drop(watcher);
                    }
                    if let Ok(receipt) = events.emit_fenced(NotificationMethod::FsChanged(Changed { watch_id: watch_id.clone(), changed_paths: Vec::new(), resync_required: true, retired: true }),Some(stop.clone())) { let _ = events.emitted(receipt).await; }
                    if let Some(owner) = registry.upgrade() {
                        let removed = {
                            let mut state = owner.0.lock().expect("filesystem watches");
                            if state.active.get(&watch_id).is_some_and(|entry|Arc::ptr_eq(&entry.generation,&generation)) { state.active.remove(&watch_id) } else { None }
                        };
                        if let Some(entry) = removed { entry.stop.cancel(); }
                    }
                });
                Ok(())
            }).map_err(super::connection::environment_error)
        }).await.map_err(|_| domain(ErrorData::OutcomeUnknown))?
    }
}

fn relevant(event: &notify::Event) -> bool {
    event.need_rescan()
        || !matches!(event.kind, notify::EventKind::Access(kind) if !matches!(kind, notify::event::AccessKind::Close(notify::event::AccessMode::Write)))
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
