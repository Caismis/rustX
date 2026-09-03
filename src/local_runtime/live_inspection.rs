//! Local live Runtime Client inspection routing.
//!
//! A running child owns its Runtime Client projection in the child process.
//! This module provides only the bounded local IPC seam needed to attach a
//! read-only client to that projection. The stable socket pathname is process
//! routing state, never conversation history and never a discovery registry.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;

use crate::runtime_client::endpoint::RuntimeClientEndpoint;
use crate::runtime_client::host::RuntimeClientHost;
use crate::runtime_client::transport::stdio::{
    StdioSessionEnd, StdioTransportError, serve_stdio_jsonl_with_io,
};

/// The live read-only Runtime Client endpoint of one child process.
pub(crate) struct LiveConversationInspectionServer {
    path: PathBuf,
    stop: Arc<tokio::sync::Notify>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl LiveConversationInspectionServer {
    /// Binds the child-owned endpoint at the identity-derived semantic path.
    ///
    /// A path left by a process that died without cleanup is stale by
    /// construction: a child conversation identity cannot have two live
    /// physical incarnations. It is therefore safe to remove that exact
    /// socket and retry the bind.
    pub(crate) fn bind(path: PathBuf, host: RuntimeClientHost) -> std::io::Result<Self> {
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
                    let endpoint = RuntimeClientEndpoint::new_read_only(host);
                    let _ = serve_stdio_jsonl_with_io(endpoint, reader, writer).await;
                });
            }
        }
    }
    connections.abort_all();
    while connections.join_next().await.is_some() {}
}

/// Probes the identity-derived live endpoint.
pub(crate) async fn connect_live(path: &Path) -> std::io::Result<UnixStream> {
    UnixStream::connect(path).await
}

/// Proxies a live Runtime Client byte stream to the inspector process's
/// stdio. The remote Runtime Client transport remains the framing/semantic
/// owner; this process only forwards bytes and returns cleanly when the live
/// child endpoint closes.
pub(crate) async fn serve_live_stdio(
    stream: UnixStream,
) -> Result<StdioSessionEnd, StdioTransportError> {
    serve_live_with_io(stream, tokio::io::stdin(), tokio::io::stdout()).await
}

/// Proxies a live Runtime Client byte stream over arbitrary async byte
/// streams. The process-stdio adapter and deterministic in-process routing
/// tests use this exact byte-for-byte path; JSONL framing remains owned by the
/// child Runtime Client endpoint.
pub(crate) async fn serve_live_with_io<R, W>(
    stream: UnixStream,
    reader: R,
    writer: W,
) -> Result<StdioSessionEnd, StdioTransportError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (mut remote_reader, mut remote_writer) = stream.into_split();
    let mut reader = reader;
    let mut writer = writer;
    tokio::select! {
        result = tokio::io::copy(&mut reader, &mut remote_writer) => {
            match result {
                Ok(_) => {
                    let _ = remote_writer.shutdown().await;
                    Ok(StdioSessionEnd::InputEof)
                }
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                    Ok(StdioSessionEnd::OutputBrokenPipe)
                }
                Err(error) => Err(StdioTransportError::InputIo(error)),
            }
        }
        result = tokio::io::copy(&mut remote_reader, &mut writer) => {
            match result {
                Ok(_) => Ok(StdioSessionEnd::InputEof),
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                    Ok(StdioSessionEnd::OutputBrokenPipe)
                }
                Err(error) if is_peer_close(&error) => Ok(StdioSessionEnd::InputEof),
                Err(error) => Err(StdioTransportError::OutputIo(error)),
            }
        }
    }
}

fn is_peer_close(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::UnexpectedEof
    )
}
