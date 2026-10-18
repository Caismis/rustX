//! The child-owned live read-only inspection endpoint.
//!
//! A running subagent child owns its Runtime Client projection in the child
//! process. This module binds the bounded local IPC seam through which a
//! read-only attachment reads that projection: a Unix socket at the
//! identity-derived path, serving strict Runtime Client JSONL through
//! [`RuntimeClientEndpoint::new_read_only`], so every mutation is refused
//! before dispatch and no attachment can activate the child or start a turn.
//! Closing a connection detaches it without cancelling child work, and child
//! shutdown stops the accept loop, settles every connection task and removes
//! the socket. The socket pathname and the locked liveness sidecar are process
//! routing state, never conversation history and never a discovery registry.
//! External product access to child history goes through App Server
//! `agent/observe` (bounded checkpoints/native cursor events), `agent/transcript`
//! and bounded agent domain reads, never through this socket.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

#[cfg(test)]
use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use tokio::net::UnixListener;
use tokio::task::JoinSet;

use crate::runtime_client::endpoint::RuntimeClientEndpoint;
use crate::runtime_client::host::RuntimeClientHost;
use crate::runtime_client::transport::stdio::serve_stdio_jsonl_with_io;

/// Test-only bind fault injection used by the real child-process regression.
/// It is an environment seam because the child is a separately spawned rustX
/// process; ordinary production launches never set it.
pub(crate) const TEST_FAIL_BIND_ENV: &str = "RUSTX_TEST_LIVE_INSPECTION_BIND_FAILURE";

/// The disposable process-local lease for one running child conversation.
///
/// The file is kept beside the stable child store only as a routing marker.
/// The exclusive OS lock is the liveness signal: it disappears automatically
/// if the child is killed, and the stable lock inode is retained across normal shutdown. No conversation or observation state is written here.
pub(crate) struct LiveConversationInspectionLease {
    _lock: Flock<File>,
    _lifecycle: crate::runtime::local_storage::ConversationAccess,
}

impl LiveConversationInspectionLease {
    /// Acquires the child-owned liveness lease at the identity-derived path.
    pub(crate) fn acquire(
        root: &Path,
        session_id: &crate::runtime::identity::SessionId,
        conversation_id: &crate::runtime::identity::ConversationId,
    ) -> std::io::Result<Self> {
        let root = crate::runtime::local_storage::ProductRoot::existing(root)?;
        if !crate::runtime::subagent::is_safe_child_conversation_component(conversation_id) {
            return Err(std::io::Error::other("invalid child Conversation identity"));
        }
        let path = crate::runtime::subagent::child_conversation_inspection_liveness_path(
            root.root(),
            session_id,
            conversation_id,
        );
        let lifecycle = crate::runtime::local_storage::ConversationAccess::existing(
            &root,
            path.parent()
                .ok_or_else(|| std::io::Error::other("missing child allocation"))?,
        )?;
        let path = lifecycle.confined(&path)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        let lock = Flock::lock(file, FlockArg::LockExclusiveNonblock).map_err(|(_, error)| {
            std::io::Error::other(format!("lock {}: {error}", path.display()))
        })?;
        Ok(Self {
            _lock: lock,
            _lifecycle: lifecycle,
        })
    }
}

/// Reports whether the identity-derived liveness lease is currently held.
/// Only the deletion regressions read the lease; production readers retired
/// with the standalone inspector process.
/// `Ok(None)` means that no marker exists; `Ok(Some(false))` means that a
/// stale marker was found but its lock is no longer held.
#[cfg(test)]
pub(crate) fn probe_liveness(path: &Path) -> std::io::Result<Option<bool>> {
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    match Flock::lock(file, FlockArg::LockSharedNonblock) {
        Ok(lock) => {
            drop(lock);
            Ok(Some(false))
        }
        Err((file, error)) if error == Errno::EWOULDBLOCK => {
            drop(file);
            Ok(Some(true))
        }
        Err((file, error)) => {
            drop(file);
            Err(std::io::Error::other(format!(
                "probe {}: {error}",
                path.display()
            )))
        }
    }
}

/// The live read-only Runtime Client endpoint of one child process.
pub(crate) struct LiveConversationInspectionServer {
    path: PathBuf,
    /// Shutdown's linearization point: cancelling it stops the accept loop and
    /// every connection's socket I/O at once, so no response is delivered
    /// after shutdown begins. Admitted physical Source work still keeps its
    /// connection task until the listener joins it.
    stop: tokio_util::sync::CancellationToken,
    task: Option<tokio::task::JoinHandle<Result<(), InspectionShutdownFailure>>>,
    shutdown_failure: Option<InspectionShutdownFailure>,
    #[cfg(test)]
    workset: tokio::sync::watch::Receiver<usize>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum InspectionShutdownFailure {
    Cancelled,
    Panicked,
    ConnectionCancelled,
    ConnectionPanicked,
}
impl std::fmt::Display for InspectionShutdownFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "live inspection listener was cancelled before physical join",
            Self::Panicked => "live inspection listener panicked before physical join",
            Self::ConnectionCancelled => {
                "live inspection connection was cancelled before physical join"
            }
            Self::ConnectionPanicked => "live inspection connection panicked before physical join",
        })
    }
}
impl std::error::Error for InspectionShutdownFailure {}

impl LiveConversationInspectionServer {
    /// Binds the child-owned endpoint at the identity-derived local path.
    ///
    /// A path left by a process that died without cleanup is stale by
    /// construction: a child conversation identity cannot have two live
    /// physical incarnations. It is therefore safe to remove that exact
    /// socket and retry the bind.
    pub(crate) fn bind(
        path: PathBuf,
        host: RuntimeClientHost,
        activation_id: crate::runtime::identity::SubagentId,
    ) -> std::io::Result<Self> {
        if host.inner.observation_activation_id() != Some(activation_id) {
            return Err(std::io::Error::other(
                "inspection host does not own the admitted activation",
            ));
        }
        if std::env::var_os(TEST_FAIL_BIND_ENV).is_some() {
            return Err(std::io::Error::other(
                "test-injected live inspection endpoint bind failure",
            ));
        }
        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                std::fs::remove_file(&path)?;
                UnixListener::bind(&path)?
            }
            Err(error) => return Err(error),
        };
        let stop = tokio_util::sync::CancellationToken::new();
        #[cfg(test)]
        let (workset, workset_rx) = tokio::sync::watch::channel(0);
        let task = tokio::spawn(run_listener(
            listener,
            host,
            stop.clone(),
            #[cfg(test)]
            workset,
        ));
        Ok(Self {
            path,
            stop,
            task: Some(task),
            shutdown_failure: None,
            #[cfg(test)]
            workset: workset_rx,
        })
    }

    /// Physically owned tasks at the listener's admission/reap boundary.
    #[cfg(test)]
    pub(crate) fn physical_workset(&self) -> tokio::sync::watch::Receiver<usize> {
        self.workset.clone()
    }

    /// Stops the accept loop, closes active inspection connections, and
    /// removes the exact process-routing socket.
    pub(crate) async fn shutdown(&mut self) -> Result<(), InspectionShutdownFailure> {
        self.stop.cancel();
        if let Some(task) = self.task.as_mut() {
            let result = task.await;
            self.task.take();
            self.shutdown_failure = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error),
                Err(error) => Some(if error.is_cancelled() {
                    InspectionShutdownFailure::Cancelled
                } else {
                    InspectionShutdownFailure::Panicked
                }),
            };
        }
        if let Some(error) = self.shutdown_failure {
            return Err(error);
        }
        let _ = std::fs::remove_file(&self.path);
        Ok(())
    }
}

/// Cancels socket readiness without cancelling an admitted source request.
/// The connection task retains its `JoinSet` capacity until that request's real
/// synchronous/blocking work returns; shutdown joins those tasks instead of
/// abandoning their physical workers.
struct InspectionIo<T> {
    inner: T,
    stopped: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
}
impl<T> InspectionIo<T> {
    fn new(inner: T, stop: tokio_util::sync::CancellationToken) -> Self {
        Self {
            inner,
            stopped: Box::pin(stop.cancelled_owned()),
        }
    }
}
impl<T: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for InspectionIo<T> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.stopped.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(Ok(()));
        }
        std::pin::Pin::new(&mut this.inner).poll_read(cx, buffer)
    }
}
impl<T: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for InspectionIo<T> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if this.stopped.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        std::pin::Pin::new(&mut this.inner).poll_write(cx, bytes)
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.stopped.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        std::pin::Pin::new(&mut this.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::pin::Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

async fn run_listener(
    listener: UnixListener,
    host: RuntimeClientHost,
    stop: tokio_util::sync::CancellationToken,
    #[cfg(test)] workset: tokio::sync::watch::Sender<usize>,
) -> Result<(), InspectionShutdownFailure> {
    let mut connections = JoinSet::new();
    let mut connection_failure = None;
    let mut endpoints: Vec<crate::runtime_client::endpoint::EndpointRetirement> = Vec::new();
    loop {
        tokio::select! {
            biased;
            () = stop.cancelled() => break,
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(result) = joined {
                    record_connection_failure(&mut connection_failure, result);
                }
                endpoints.retain(crate::runtime_client::endpoint::EndpointRetirement::is_alive);
                #[cfg(test)]
                workset.send_replace(connections.len());
            },
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    break;
                };
                // The JoinSet owns physical capacity through actual task exit.
                // No separate permit may be recycled when a waiter is dropped.
                if connections.len() >= 8 {
                    drop(stream);
                    continue;
                }
                let endpoint = RuntimeClientEndpoint::new_read_only(&host);
                endpoints.push(endpoint.retirement());
                let socket_stop = stop.clone();
                connections.spawn(async move {
                    let (reader, writer) = stream.into_split();
                    let _ = serve_stdio_jsonl_with_io(endpoint, InspectionIo::new(reader,socket_stop.clone()), InspectionIo::new(writer,socket_stop)).await;
                });
                #[cfg(test)]
                workset.send_replace(connections.len());
            }
        }
    }
    // Shutdown already stopped every connection's socket I/O. Retire actual
    // admitted observation waits, including a decoded request racing to
    // subscribe. Physical Source work still owns its JoinSet task.
    for endpoint in endpoints {
        endpoint.retire();
    }
    drain_connections(&mut connections, &mut connection_failure).await;
    #[cfg(test)]
    workset.send_replace(0);
    connection_failure.map_or(Ok(()), Err)
}

fn record_connection_failure(
    failure: &mut Option<InspectionShutdownFailure>,
    result: Result<(), tokio::task::JoinError>,
) {
    if let Err(error) = result
        && failure.is_none()
    {
        *failure = Some(if error.is_cancelled() {
            InspectionShutdownFailure::ConnectionCancelled
        } else {
            InspectionShutdownFailure::ConnectionPanicked
        });
    }
}

async fn drain_connections(
    connections: &mut JoinSet<()>,
    failure: &mut Option<InspectionShutdownFailure>,
) {
    while let Some(result) = connections.join_next().await {
        record_connection_failure(failure, result);
    }
}

#[cfg(test)]
type TraceReadGate = (
    tokio::sync::oneshot::Sender<()>,
    std::sync::mpsc::Receiver<()>,
);
#[cfg(test)]
static TRACE_READ_GATES: std::sync::LazyLock<
    std::sync::Mutex<
        std::collections::BTreeMap<crate::runtime::identity::ConversationId, TraceReadGate>,
    >,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::BTreeMap::new()));
#[cfg(test)]
pub(crate) fn install_trace_read_gate(
    conversation: crate::runtime::identity::ConversationId,
    gate: TraceReadGate,
) {
    assert!(
        TRACE_READ_GATES
            .lock()
            .unwrap()
            .insert(conversation, gate)
            .is_none()
    );
}
#[cfg(test)]
pub(crate) fn trace_read_gate(conversation: &crate::runtime::identity::ConversationId) {
    let gate = TRACE_READ_GATES.lock().unwrap().remove(conversation);
    if let Some((entered, release)) = gate {
        let _ = entered.send(());
        release.recv().unwrap();
    }
}

#[cfg(test)]
mod shutdown_tests;
