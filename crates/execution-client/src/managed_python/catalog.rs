//! A binding owns frozen Python sources until their Server candidate releases them.

use super::{PythonToolPackage, PythonToolStore, discover_package, validate_identifier};
use crate::process::Operations;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rustx_confined_fs::PackageDirectory;
use rustx_execution_protocol::{
    ReadError,
    mcp::{PythonSourceReply, PythonSourceRequest, PythonSourceScope},
    process::{OperationId, ProcessOperationKey},
};
use rustx_process_supervisor::{
    interactive_process::InteractiveProcessSpec, signal::ProcessSignal,
};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicU64},
};

const COLLECTION_CAP: usize = 128;
const MAX_CAPTURE_OWNERS: usize = 256;
const MAX_CAPTURE_WORKERS: usize = 4;

pub(crate) struct Catalog {
    workspace: PathBuf,
    user: Option<PathBuf>,
    store_root: Option<PathBuf>,
    store: Arc<Mutex<Option<PythonToolStore>>>,
    captures: SourceCleanup,
    capture_workers: Arc<tokio::sync::Semaphore>,
    build_workers: Arc<tokio::sync::Semaphore>,
}

struct Capture {
    scope: PythonSourceScope,
    name: String,
    signal: ProcessSignal,
    result: Arc<Mutex<Option<Result<Frozen, ReadError>>>>,
    preparation: Arc<Mutex<Preparation>>,
}

#[derive(Default)]
struct Captures {
    closed: bool,
    entries: BTreeMap<OperationId, Arc<Capture>>,
}

#[derive(Clone, Default)]
pub(crate) struct SourceCleanup {
    captures: Arc<Mutex<Captures>>,
    bytes: Arc<AtomicU64>,
}

impl SourceCleanup {
    pub(crate) fn close(&self) -> bool {
        let entries = {
            let mut state = self.captures.lock().expect("Python sources");
            state.closed = true;
            std::mem::take(&mut state.entries)
        };
        drop(entries);
        self.bytes.load(std::sync::atomic::Ordering::Acquire) == 0
    }

    fn get(&self, id: OperationId) -> Option<Arc<Capture>> {
        self.captures
            .lock()
            .expect("Python sources")
            .entries
            .get(&id)
            .cloned()
    }

    fn release(&self, id: OperationId) {
        let removed = self
            .captures
            .lock()
            .expect("Python sources")
            .entries
            .remove(&id);
        drop(removed);
    }
}

#[derive(Default)]
enum Preparation {
    #[default]
    Idle,
    Running,
    Failed,
    Ready(InteractiveProcessSpec),
}

struct Frozen {
    package: PythonToolPackage,
    permissions: Option<String>,
}

struct SourceLaunch {
    package: PythonToolPackage,
    store_root: PathBuf,
    workspace: PathBuf,
    store: Arc<Mutex<Option<PythonToolStore>>>,
}

impl SourceLaunch {
    async fn prepare(
        self,
        lifetime: &ProcessSignal,
    ) -> Result<InteractiveProcessSpec, super::PythonToolError> {
        let Self {
            package,
            store_root,
            workspace,
            store,
        } = self;
        let execution_cwd = workspace.clone();
        let store = tokio::task::spawn_blocking(move || {
            let mut slot = store.lock().expect("Managed Python Store");
            if let Some(store) = slot.as_ref() {
                return Ok(store.clone());
            }
            let directory = super::private_store_path(&store_root, &workspace)?;
            let store = PythonToolStore::new(directory)?;
            *slot = Some(store.clone());
            Ok::<_, super::PythonToolError>(store)
        })
        .await
        .map_err(super::io_error)??;
        let prepared = store.ensure_prepared(&package, lifetime).await?;
        Ok(prepared.launch(execution_cwd))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.signal.cancel();
    }
}

impl Catalog {
    pub(crate) fn new(workspace: &Path) -> Self {
        let user = crate::client_user_root();
        let store_root = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                user.as_deref()
                    .and_then(Path::parent)
                    .map(|home| home.join(".local/share"))
            })
            .map(|data| data.join("rustx/environments/python-tools"));
        Self {
            workspace: workspace.to_path_buf(),
            user,
            store_root,
            store: Arc::default(),
            captures: SourceCleanup::default(),
            capture_workers: Arc::new(tokio::sync::Semaphore::new(MAX_CAPTURE_WORKERS)),
            build_workers: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }

    pub(crate) fn cleanup(&self) -> SourceCleanup {
        self.captures.clone()
    }

    fn root(&self, scope: PythonSourceScope) -> Option<PathBuf> {
        match scope {
            PythonSourceScope::User => Some(self.user.as_ref()?.join(".agents/tools")),
            PythonSourceScope::Workspace => Some(self.workspace.join(".agents/tools")),
        }
    }

    fn names(&self, scope: PythonSourceScope) -> PythonSourceReply {
        let Some(root) = self.root(scope) else {
            return PythonSourceReply::Names(Vec::new());
        };
        let directory = match PackageDirectory::open(&root) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return PythonSourceReply::Names(Vec::new());
            }
            Err(error) if error.kind() == io::ErrorKind::NotADirectory => {
                return PythonSourceReply::CollectionInvalid;
            }
            Err(_) => return PythonSourceReply::Refused(ReadError::Unavailable),
        };
        let Ok(entries) = directory.names(COLLECTION_CAP) else {
            return PythonSourceReply::Refused(ReadError::Unavailable);
        };
        PythonSourceReply::Names(
            entries
                .into_iter()
                .filter_map(|path| {
                    let name = path.to_str()?;
                    (name.len() <= 128 && validate_identifier(name).is_ok())
                        .then(|| name.to_owned())
                })
                .collect(),
        )
    }

    pub(crate) fn exchange(
        &mut self,
        request: PythonSourceRequest,
        operations: &Arc<Operations>,
    ) -> PythonSourceReply {
        match request {
            PythonSourceRequest::List { scope } => self.names(scope),
            PythonSourceRequest::Capture { key, scope, name } if operations.own_key(&key) => {
                self.capture(key.operation, scope, name, operations)
            }
            PythonSourceRequest::State { key } if operations.own_key(&key) => {
                self.captures.get(key.operation).as_deref().map_or(
                    PythonSourceReply::Refused(ReadError::NotFound),
                    Capture::observe,
                )
            }
            PythonSourceRequest::Prepare { key } if operations.own_key(&key) => {
                self.prepare(key.operation, operations)
            }
            PythonSourceRequest::Release { key } if operations.own_key(&key) => {
                self.captures.release(key.operation);
                PythonSourceReply::Released
            }
            _ => PythonSourceReply::Refused(ReadError::Invalid),
        }
    }

    fn capture(
        &mut self,
        id: OperationId,
        scope: PythonSourceScope,
        name: String,
        operations: &Arc<Operations>,
    ) -> PythonSourceReply {
        if name.len() > 128 || validate_identifier(&name).is_err() {
            return PythonSourceReply::Refused(ReadError::Invalid);
        }
        let mut captures = self.captures.captures.lock().expect("Python sources");
        if captures.closed {
            return PythonSourceReply::Refused(ReadError::Unavailable);
        }
        if let Some(existing) = captures.entries.get(&id) {
            return if existing.scope == scope && existing.name == name {
                existing.observe()
            } else {
                PythonSourceReply::Refused(ReadError::Invalid)
            };
        }
        if captures.entries.len() >= MAX_CAPTURE_OWNERS {
            return PythonSourceReply::Refused(ReadError::Busy);
        }
        let Ok(worker_capacity) = self.capture_workers.clone().try_acquire_owned() else {
            return PythonSourceReply::Refused(ReadError::Busy);
        };
        let Some(root) = self.root(scope) else {
            return PythonSourceReply::Refused(ReadError::Unavailable);
        };
        let signal = operations.auxiliary_signal();
        let result = Arc::new(Mutex::new(None));
        captures.entries.insert(
            id,
            Arc::new(Capture {
                scope,
                name: name.clone(),
                signal: signal.clone(),
                result: result.clone(),
                preparation: Arc::default(),
            }),
        );
        drop(captures);
        let quota = self.captures.bytes.clone();
        #[cfg(test)]
        let hold = operations.take_capture_hold(id);
        if !operations.start_auxiliary_owned(move |mut worker| async move {
            worker.physical_settlement(false);
            let _ = tokio::task::spawn_blocking(move || {
                let outcome = match discover_package(
                    &root.join(&name),
                    &name,
                    &signal,
                    quota,
                    #[cfg(test)]
                    hold,
                ) {
                    Ok((package, permissions)) => Ok(Frozen {
                        package,
                        permissions: permissions.map(|document| STANDARD.encode(document)),
                    }),
                    Err(super::PythonToolError::InvalidPackage(_)) => Err(ReadError::Invalid),
                    Err(_) => Err(ReadError::Unavailable),
                };
                *result.lock().expect("Python source capture") = Some(outcome);
                drop(result);
                drop(worker_capacity);
                worker.physical_settlement(true);
            })
            .await;
        }) {
            self.captures.release(id);
            return PythonSourceReply::Refused(ReadError::Unavailable);
        }
        PythonSourceReply::Capturing
    }

    fn prepare(&self, id: OperationId, operations: &Operations) -> PythonSourceReply {
        let Some(capture) = self.captures.get(id) else {
            return PythonSourceReply::Refused(ReadError::NotFound);
        };
        let mut preparation = capture.preparation.lock().expect("Python preparation");
        match &*preparation {
            Preparation::Ready(_) => return PythonSourceReply::Prepared,
            Preparation::Running => return PythonSourceReply::Preparing,
            Preparation::Failed => return PythonSourceReply::Refused(ReadError::Unavailable),
            Preparation::Idle => {}
        }
        let Ok(worker_capacity) = self.build_workers.clone().try_acquire_owned() else {
            return PythonSourceReply::Refused(ReadError::Busy);
        };
        let Some(store_root) = self.store_root.clone() else {
            return PythonSourceReply::Refused(ReadError::Unavailable);
        };
        let package = match capture
            .result
            .lock()
            .expect("Python source capture")
            .as_ref()
        {
            Some(Ok(frozen)) => frozen.package.clone(),
            Some(Err(error)) => return PythonSourceReply::Refused(*error),
            None => return PythonSourceReply::Capturing,
        };
        let launch = SourceLaunch {
            package,
            store_root,
            workspace: self.workspace.clone(),
            store: self.store.clone(),
        };
        let owner = capture.preparation.clone();
        let lifetime = operations.auxiliary_signal();
        *preparation = Preparation::Running;
        drop(preparation);
        if operations.start_auxiliary_owned(move |mut guard| async move {
            let _worker_capacity = worker_capacity;
            guard.physical_settlement(false);
            let prepared = launch.prepare(&lifetime).await;
            guard.physical_settlement(!matches!(
                prepared,
                Err(super::PythonToolError::Unconfirmed(_))
            ));
            *owner.lock().expect("Python preparation") = match prepared {
                Ok(spec) => Preparation::Ready(spec),
                Err(_) => Preparation::Failed,
            };
        }) {
            PythonSourceReply::Preparing
        } else {
            *capture.preparation.lock().expect("Python preparation") = Preparation::Failed;
            PythonSourceReply::Refused(ReadError::Unavailable)
        }
    }

    pub(crate) fn prepared_launch(
        &self,
        key: &ProcessOperationKey,
        operations: &Operations,
    ) -> Option<InteractiveProcessSpec> {
        if !operations.own_key(key) {
            return None;
        }
        match &*self.captures.get(key.operation)?.preparation.lock().ok()? {
            Preparation::Ready(spec) => Some(spec.clone()),
            _ => None,
        }
    }
}

impl Capture {
    fn observe(&self) -> PythonSourceReply {
        match self.result.lock().expect("Python source capture").as_ref() {
            Some(Ok(frozen)) => PythonSourceReply::Captured {
                digest: frozen.package.source_digest.clone(),
                permissions_base64: frozen.permissions.clone(),
                watch_directories: frozen.package.watch_directories.clone(),
            },
            Some(Err(error)) => PythonSourceReply::Refused(*error),
            None => PythonSourceReply::Capturing,
        }
    }
}
