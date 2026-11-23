//! UI-owned subscriptions to bounded change hints from the bound Execution Client.
use super::protocol::{ErrorData, NotificationMethod, RequestId, RpcError};
use super::rpc_error::domain;
use crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint;
use crate::runtime::CancellationSignal;
use rustx_execution_protocol::watch::WatchId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

const POLL_PERIOD: std::time::Duration = std::time::Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Changed {
    pub watch_id: String,
    pub changed_paths: Vec<PathBuf>,
    pub resync_required: bool,
    pub retired: bool,
}

pub(super) struct WatchSource {
    pub endpoint: Arc<LocalExecutionEndpoint>,
    pub root_grant: CancellationSignal,
    pub path: PathBuf,
}

#[derive(Clone)]
struct Publication {
    stop: CancellationToken,
    open: Arc<Mutex<bool>>,
}

impl Publication {
    fn new() -> Self {
        Self {
            stop: CancellationToken::new(),
            open: Arc::new(Mutex::new(true)),
        }
    }

    fn stop(&self) {
        *self.open.lock().expect("watch publication") = false;
        self.stop.cancel();
    }

    fn emit(&self, events: &super::service_events::Events, change: Changed) {
        let active = self.open.lock().expect("watch publication");
        if *active && !self.stop.is_cancelled() {
            let _ = events.emit_fenced(
                NotificationMethod::FsChanged(change),
                Some(self.stop.clone()),
            );
        }
    }
}

struct Entry {
    publication: Publication,
    settled: watch::Receiver<Option<Result<(), ErrorData>>>,
    #[cfg(test)]
    retirement_queued: Option<oneshot::Sender<()>>,
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
    pub(crate) fn observe_retirement(&self, watch_id: &str) -> oneshot::Receiver<()> {
        let (sender, receiver) = oneshot::channel();
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
        for entry in std::mem::take(&mut state.active).into_values() {
            entry.publication.stop();
        }
    }

    pub async fn unwatch(&self, watch_id: &str) -> Result<(), RpcError> {
        let mut entry = {
            let mut state = self.0.lock().expect("filesystem watches");
            let entry = state
                .active
                .remove(watch_id)
                .ok_or_else(|| domain(ErrorData::EnvironmentResourceNotFound))?;
            entry.publication.stop();
            entry
        };
        entry
            .settled
            .wait_for(Option::is_some)
            .await
            .map_err(|_| domain(ErrorData::OutcomeUnknown))?
            .clone()
            .expect("settled observer")
            .map_err(domain)
    }

    pub async fn watch(
        self: &Arc<Self>,
        watch_id: String,
        source: WatchSource,
        events: Arc<super::service_events::Events>,
        host: super::host::AppServerHost,
        owner: super::host::ServerOperation,
    ) -> Result<(), RpcError> {
        if watch_id.is_empty()
            || !RequestId::is_valid_string(&watch_id)
            || source.path.as_os_str().is_empty()
            || source.path.as_os_str().len() > 4096
        {
            return Err(domain(ErrorData::InvalidParams));
        }
        let capacity = host
            .file_observers()
            .try_acquire_owned()
            .map_err(|_| domain(ErrorData::EnvironmentCapacityOrConflict))?;
        let id = WatchId::file().ok_or_else(|| domain(ErrorData::EnvironmentCapacityOrConflict))?;
        let publication = Publication::new();
        let (settled, observation) = watch::channel(None);
        {
            let mut state = self.0.lock().expect("filesystem watches");
            if state.closed || events.closed.is_cancelled() {
                return Err(domain(ErrorData::ConnectionClosed));
            }
            if state.active.len() >= 8 || state.active.contains_key(&watch_id) {
                return Err(domain(ErrorData::EnvironmentCapacityOrConflict));
            }
            state.active.insert(
                watch_id.clone(),
                Entry {
                    publication: publication.clone(),
                    settled: observation,
                    #[cfg(test)]
                    retirement_queued: None,
                },
            );
        }
        let (admitted, receipt) = oneshot::channel();
        let operation = Observation {
            registry: Arc::downgrade(self),
            watch_id,
            id,
            source,
            events,
            publication,
            settled,
            host,
        };
        tokio::spawn(async move {
            operation.run(owner, admitted, capacity).await;
        });
        receipt
            .await
            .map_err(|_| domain(ErrorData::OutcomeUnknown))?
    }
}

struct Observation {
    registry: Weak<Watches>,
    watch_id: String,
    id: WatchId,
    source: WatchSource,
    events: Arc<super::service_events::Events>,
    publication: Publication,
    settled: watch::Sender<Option<Result<(), ErrorData>>>,
    host: super::host::AppServerHost,
}

impl Observation {
    async fn run(
        self,
        owner: super::host::ServerOperation,
        admitted: oneshot::Sender<Result<(), RpcError>>,
        capacity: tokio::sync::OwnedSemaphorePermit,
    ) {
        let relative = self.register().await;
        drop(owner);
        let relative = match relative {
            Ok(relative) => relative,
            Err(error) => {
                self.remove();
                let settled = if error.data == Some(ErrorData::OutcomeUnknown) {
                    self.host.record_unconfirmed_file_observer();
                    Err(ErrorData::OutcomeUnknown)
                } else {
                    Ok(())
                };
                drop(capacity);
                self.settled.send_replace(Some(settled));
                let _ = admitted.send(Err(error));
                return;
            }
        };
        let _ = admitted.send(Ok(()));
        let ended_on_client = self.observe(&relative).await;
        if !ended_on_client && self.host.server_draining() {
            // The drain may already be physically retiring this observer
            // through a binding-wide revoke. Use that receipt before
            // deciding whether the individual stop is unconfirmed.
            let _ = self.host.settle_execution().await;
        }
        let physically_settled = if ended_on_client {
            Ok(())
        } else {
            let endpoint = self.source.endpoint.clone();
            let id = self.id;
            match tokio::task::spawn_blocking(move || endpoint.stop_watch(id)).await {
                Ok(Ok(())) => Ok(()),
                _ => Err(ErrorData::OutcomeUnknown),
            }
        };
        if !self.publication.stop.is_cancelled() && !self.events.closed.is_cancelled() {
            self.retired().await;
        }
        self.remove();
        if physically_settled.is_err() {
            self.host.record_unconfirmed_file_observer();
        }
        drop(capacity);
        self.settled.send_replace(Some(physically_settled));
    }

    async fn register(&self) -> Result<PathBuf, RpcError> {
        let endpoint = self.source.endpoint.clone();
        let path = self.source.path.clone();
        let stop = self.publication.stop.clone();
        let closed = self.events.closed.clone();
        let grant = self.source.root_grant.clone();
        let id = self.id;
        tokio::task::spawn_blocking(move || {
            endpoint.register_watch(id, &path, || {
                if closed.is_cancelled() || stop.is_cancelled() {
                    Err(ErrorKind::ConnectionAborted.into())
                } else if grant.is_cancelled() {
                    Err(ErrorKind::PermissionDenied.into())
                } else {
                    Ok(())
                }
            })
        })
        .await
        .map_err(|_| domain(ErrorData::OutcomeUnknown))?
        .map_err(|error| match error.kind() {
            ErrorKind::ConnectionAborted => domain(ErrorData::ConnectionClosed),
            ErrorKind::Unsupported | ErrorKind::NotConnected => {
                domain(ErrorData::ExecutionUnavailable)
            }
            ErrorKind::WouldBlock => domain(ErrorData::EnvironmentCapacityOrConflict),
            _ => super::connection::environment_error(error),
        })
    }

    async fn observe(&self, relative: &Path) -> bool {
        loop {
            let endpoint = self.source.endpoint.clone();
            let path = relative.to_path_buf();
            let id = self.id;
            let result = tokio::select! {
                biased;
                () = self.publication.stop.cancelled() => return false,
                () = self.events.closed.cancelled() => return false,
                () = self.source.root_grant.cancelled() => return false,
                result = async {
                    tokio::time::sleep(POLL_PERIOD).await;
                    tokio::task::spawn_blocking(move || endpoint.poll_watch(id, &path)).await
                } => result,
            };
            let change = match result {
                Ok(Ok(change)) if change.retired => return true,
                Ok(Ok(change)) => change,
                Ok(Err(error)) if error.kind() == io::ErrorKind::WouldBlock => continue,
                _ => return false,
            };
            if change.changed_paths.is_empty() && !change.resync_required {
                continue;
            }
            let changed_paths = change
                .changed_paths
                .iter()
                .map(|path| self.source.endpoint.workspace().join(path))
                .collect();
            let change = Changed {
                watch_id: self.watch_id.clone(),
                changed_paths,
                resync_required: change.resync_required,
                retired: false,
            };
            #[cfg(test)]
            if test_pause(
                &self.source.endpoint.workspace().join(relative),
                &self.publication,
                &self.events,
                &change,
            ) {
                continue;
            }
            self.publication.emit(&self.events, change);
        }
    }

    async fn retired(&self) {
        *self.publication.open.lock().expect("watch publication") = false;
        let receipt = self.events.emit_fenced(
            NotificationMethod::FsChanged(Changed {
                watch_id: self.watch_id.clone(),
                changed_paths: Vec::new(),
                resync_required: true,
                retired: true,
            }),
            Some(self.publication.stop.clone()),
        );
        if let Ok(receipt) = receipt {
            #[cfg(test)]
            if let Some(owner) = self.registry.upgrade() {
                let queued = owner
                    .0
                    .lock()
                    .unwrap()
                    .active
                    .get_mut(&self.watch_id)
                    .filter(|entry| Arc::ptr_eq(&entry.publication.open, &self.publication.open))
                    .and_then(|entry| entry.retirement_queued.take());
                if let Some(queued) = queued {
                    let _ = queued.send(());
                }
            }
            tokio::select! {
                () = self.publication.stop.cancelled() => {},
                _ = self.events.emitted(receipt) => {},
            }
        }
    }

    fn remove(&self) {
        if let Some(owner) = self.registry.upgrade() {
            let mut state = owner.0.lock().expect("filesystem watches");
            if state
                .active
                .get(&self.watch_id)
                .is_some_and(|entry| Arc::ptr_eq(&entry.publication.open, &self.publication.open))
            {
                state.active.remove(&self.watch_id);
                self.publication.stop.cancel();
            }
        }
    }
}

#[cfg(test)]
struct PublicationPause {
    entered: oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
    completed: oneshot::Sender<()>,
}

#[cfg(test)]
static PUBLICATIONS: std::sync::LazyLock<Mutex<BTreeMap<PathBuf, PublicationPause>>> =
    std::sync::LazyLock::new(Mutex::default);

#[cfg(test)]
pub(crate) fn pause_publication(
    path: PathBuf,
) -> (
    oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
    oneshot::Receiver<()>,
) {
    let (entered, at_publication) = oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let (completed, after_publication) = oneshot::channel();
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
fn test_pause(
    path: &Path,
    publication: &Publication,
    events: &Arc<super::service_events::Events>,
    change: &Changed,
) -> bool {
    let Some(pause) = PUBLICATIONS.lock().unwrap().remove(path) else {
        return false;
    };
    let publication = publication.clone();
    let events = events.clone();
    let change = change.clone();
    tokio::task::spawn_blocking(move || {
        let _ = pause.entered.send(());
        let _ = pause.release.recv();
        publication.emit(&events, change);
        let _ = pause.completed.send(());
    });
    true
}
