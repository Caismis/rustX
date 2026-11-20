//! Server ownership of the canonical `WorkspaceChanges` projection.
use crate::local_runtime::session_runtime_manager::SessionExecutionScope;
use crate::runtime::cancellation::{CancellationSignal, ExecutionCancellation};
use crate::runtime::identity::AttemptId;
use crate::runtime::types::CancellationReason;
use crate::tools::workspace::Workspace;
use rustx_execution_protocol::changes::{ChangeReply, ChangeRequest};
pub use rustx_execution_protocol::changes::{ChangesSummary, FileComparison};
use rustx_execution_protocol::process::{OperationId, ProcessOperationKey};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_COMPLETED: usize = 16;

#[derive(Default)]
pub(crate) struct ChangesStore {
    records: Mutex<BTreeMap<String, Arc<Record>>>,
}

struct Record {
    source: Arc<ClientRecord>,
    summary: ChangesSummary,
    comparison: tokio::sync::Mutex<()>,
}

impl ChangesStore {
    pub(crate) fn summary(&self, attempt: &AttemptId) -> Option<ChangesSummary> {
        self.records
            .lock()
            .expect("Workspace changes")
            .get(attempt.as_str())
            .map(|record| record.summary.clone())
    }

    pub(crate) async fn comparison(
        &self,
        attempt: &AttemptId,
        index: usize,
    ) -> Result<Option<FileComparison>, String> {
        let record = self
            .records
            .lock()
            .expect("Workspace changes")
            .get(attempt.as_str())
            .cloned();
        let Some(record) = record else {
            return Ok(None);
        };
        if index >= record.summary.files.len() {
            return Err("invalid changed file index".to_owned());
        }
        let _comparison = record.comparison.lock().await;
        let source = &record.source;
        let cancellation = cancellation(CancellationSignal::new());
        if source
            .send(
                ChangeRequest::Compare {
                    key: source.key.clone(),
                    index,
                },
                &cancellation,
            )
            .await
            != Some(ChangeReply::Accepted)
        {
            return Err("Workspace comparison unavailable".to_owned());
        }
        match source
            .wait(
                || ChangeRequest::Comparison {
                    key: source.key.clone(),
                    index,
                },
                &cancellation,
            )
            .await
        {
            Some(ChangeReply::Comparison {
                index: echoed,
                comparison,
            }) if index == echoed => Ok(comparison),
            _ => Err("Workspace comparison unavailable".to_owned()),
        }
    }

    fn record(&self, attempt: &AttemptId, record: Record) {
        let mut records = self.records.lock().expect("Workspace changes");
        if records.len() == MAX_COMPLETED {
            records.pop_first();
        }
        records.insert(attempt.as_str().to_owned(), Arc::new(record));
    }
}

struct ClientRecord {
    scope: SessionExecutionScope,
    key: ProcessOperationKey,
}

impl ClientRecord {
    async fn send(
        &self,
        request: ChangeRequest,
        cancellation: &ExecutionCancellation,
    ) -> Option<ChangeReply> {
        self.scope
            .workspace_changes(request, cancellation)
            .await
            .ok()
    }

    async fn wait(
        &self,
        request: impl Fn() -> ChangeRequest,
        cancellation: &ExecutionCancellation,
    ) -> Option<ChangeReply> {
        tokio::time::timeout(
            rustx_execution_protocol::transport::PAGE_REQUEST_TIMEOUT,
            async {
                loop {
                    match self.send(request(), cancellation).await {
                        Some(ChangeReply::Pending) => tokio::select! {
                            biased;
                            () = cancellation.cancelled() => return None,
                            () = tokio::time::sleep(POLL_INTERVAL) => {}
                        },
                        reply => return reply,
                    }
                }
            },
        )
        .await
        .ok()
        .flatten()
    }
}

impl Drop for ClientRecord {
    fn drop(&mut self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let scope = self.scope.clone();
        let key = self.key.clone();
        handle.spawn(async move {
            let _ = scope
                .workspace_changes(
                    ChangeRequest::Release { key },
                    &cancellation(CancellationSignal::new()),
                )
                .await;
        });
    }
}

fn cancellation(signal: CancellationSignal) -> ExecutionCancellation {
    ExecutionCancellation::detached(signal, CancellationReason::UserRequested)
}

#[cfg(test)]
type FinalObservationGate = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

pub(crate) struct ChangeRecording {
    attempt: AttemptId,
    workspace: PathBuf,
    cancellation: ExecutionCancellation,
    store: Arc<ChangesStore>,
    source: tokio::sync::OnceCell<Option<Arc<ClientRecord>>>,
    preparation:
        tokio::sync::Mutex<Option<tokio_util::task::AbortOnDropHandle<Option<Arc<ClientRecord>>>>>,
    used: AtomicBool,
    failed: AtomicBool,
    #[cfg(test)]
    final_observation_gate: Mutex<Option<FinalObservationGate>>,
}

impl ChangeRecording {
    pub(crate) fn start(
        attempt: AttemptId,
        workspace: &Workspace,
        store: Arc<ChangesStore>,
        signal: CancellationSignal,
    ) -> Arc<Self> {
        let cancellation = cancellation(signal);
        let preparation = workspace.execution_scope().cloned().map(|scope| {
            let cancellation = cancellation.clone();
            tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
                let key = scope
                    .endpoint()
                    .ok()?
                    .process_key(OperationId::new())
                    .ok()?;
                let record = Arc::new(ClientRecord { scope, key });
                (record
                    .send(
                        ChangeRequest::Begin {
                            key: record.key.clone(),
                        },
                        &cancellation,
                    )
                    .await
                    == Some(ChangeReply::Accepted))
                .then_some(record)
            }))
        });
        Arc::new(Self {
            attempt,
            workspace: workspace.root().to_path_buf(),
            cancellation,
            store,
            source: tokio::sync::OnceCell::new(),
            preparation: tokio::sync::Mutex::new(preparation),
            used: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            #[cfg(test)]
            final_observation_gate: Mutex::new(None),
        })
    }

    async fn source(&self) -> Option<&ClientRecord> {
        self.source
            .get_or_init(|| async {
                let preparation = self.preparation.lock().await.take()?;
                preparation.await.ok().flatten()
            })
            .await
            .as_deref()
    }

    #[cfg(test)]
    pub(crate) fn hold_final_observation(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, waiting) = tokio::sync::oneshot::channel();
        *self.final_observation_gate.lock().unwrap() = Some((entered, waiting));
        (started, release)
    }

    pub(crate) async fn before_tools(&self) {
        if let Some(source) = self.source().await {
            let reply = source
                .wait(
                    || ChangeRequest::Baseline {
                        key: source.key.clone(),
                    },
                    &self.cancellation,
                )
                .await;
            self.used
                .store(reply == Some(ChangeReply::BaselineReady), Ordering::Relaxed);
        }
    }

    pub(crate) async fn capture(&self, path: &Path) {
        if !self.used.load(Ordering::Relaxed) {
            return;
        }
        if let Some(source) = self.source().await
            && let Ok(path) = path.strip_prefix(&self.workspace)
        {
            let captured = source
                .send(
                    ChangeRequest::Capture {
                        key: source.key.clone(),
                        path: path.to_path_buf(),
                    },
                    &self.cancellation,
                )
                .await;
            if captured != Some(ChangeReply::Accepted) {
                self.failed.store(true, Ordering::Relaxed);
            }
        }
    }

    pub(crate) async fn finish(&self) {
        if self.cancellation.is_cancelled()
            || self.failed.load(Ordering::Relaxed)
            || !self.used.load(Ordering::Relaxed)
        {
            return;
        }
        #[cfg(test)]
        {
            let gate = self.final_observation_gate.lock().unwrap().take();
            if let Some((entered, waiting)) = gate {
                let _ = entered.send(());
                tokio::select! {
                    biased;
                    () = self.cancellation.cancelled() => return,
                    _ = waiting => {}
                }
            }
        }
        let Some(source) = self.source.get().and_then(Option::as_ref).cloned() else {
            return;
        };
        if source
            .send(
                ChangeRequest::Finish {
                    key: source.key.clone(),
                },
                &self.cancellation,
            )
            .await
            != Some(ChangeReply::Accepted)
        {
            return;
        }
        let Some(ChangeReply::Summary(Some(summary))) = source
            .wait(
                || ChangeRequest::Summary {
                    key: source.key.clone(),
                },
                &self.cancellation,
            )
            .await
        else {
            return;
        };
        if summary.files.iter().any(|file| {
            let path = Path::new(&file.path);
            !rustx_execution_protocol::is_normalized_absolute_path(path)
                || !path.starts_with(&self.workspace)
        }) {
            return;
        }
        let _ = self.cancellation.with_active(|| {
            self.store.record(
                &self.attempt,
                Record {
                    source,
                    summary,
                    comparison: tokio::sync::Mutex::new(()),
                },
            );
        });
    }
}
