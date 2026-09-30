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
//! `agent/transcript`/`agent/status`, never through this socket.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    stop: Arc<tokio::sync::Notify>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl LiveConversationInspectionServer {
    /// Binds the child-owned endpoint at the identity-derived local path.
    ///
    /// A path left by a process that died without cleanup is stale by
    /// construction: a child conversation identity cannot have two live
    /// physical incarnations. It is therefore safe to remove that exact
    /// socket and retry the bind.
    pub(crate) fn bind(path: PathBuf, host: RuntimeClientHost) -> std::io::Result<Self> {
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
        let stop = Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn(run_listener(listener, host, Arc::clone(&stop)));
        Ok(Self {
            path,
            stop,
            task: Some(task),
        })
    }

    /// Stops the accept loop, closes active inspection connections, and
    /// removes the exact process-routing socket.
    pub(crate) async fn shutdown(mut self) {
        self.stop.notify_one();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn run_listener(
    listener: UnixListener,
    host: RuntimeClientHost,
    stop: Arc<tokio::sync::Notify>,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            () = stop.notified() => break,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    break;
                };
                let host = host.clone();
                connections.spawn(async move {
                    let (reader, writer) = stream.into_split();
                    let endpoint = RuntimeClientEndpoint::new_read_only(&host);
                    let _ = serve_stdio_jsonl_with_io(endpoint, reader, writer).await;
                });
            }
        }
    }
    connections.abort_all();
    while connections.join_next().await.is_some() {}
}
