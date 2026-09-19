//! Authenticated RPC preparation mints one short-lived, single-use download
//! capability. The transport credential never enters a URL. Bytes are native.
use super::host::AppServerHost;
use crate::runtime::identity::SessionId;
use crate::session_archive::SessionArchiveCut;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

const TTL: Duration = Duration::from_mins(1);
const MAX_PREPARED: usize = 16;
const PREFIX: &str = "/session-archive/";

/// Resolve path against the selected App Server's HTTP(S) origin. Only an
/// owned stdio child supplies a loopback port; never a server filesystem path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveDownloadDescriptor {
    pub path: String,
    pub filename: String,
    pub expires_in_seconds: u16,
    pub loopback_port: Option<u16>,
}
struct Prepared {
    _permit: tokio::sync::OwnedSemaphorePermit,
    cut: SessionArchiveCut,
    expires: Instant,
}
pub(crate) struct ArchiveDownloads {
    prepared: Mutex<BTreeMap<String, Prepared>>,
    remote: AtomicBool,
    capacity: std::sync::Arc<tokio::sync::Semaphore>,
}
impl Default for ArchiveDownloads {
    fn default() -> Self {
        Self {
            prepared: Mutex::default(),
            remote: AtomicBool::new(false),
            capacity: std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_PREPARED)),
        }
    }
}
impl std::fmt::Debug for ArchiveDownloads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArchiveDownloads")
    }
}
impl ArchiveDownloads {
    pub(crate) fn serve_remote(&self) {
        self.remote.store(true, Ordering::Relaxed);
    }
    fn take(&self, path: &str) -> Option<Prepared> {
        let mut prepared = self.prepared.lock().expect("archive mutex");
        prepared.retain(|_, p| p.expires > Instant::now());
        prepared.remove(path)
    }
}

pub(crate) async fn prepare(
    host: &AppServerHost,
    session: SessionId,
) -> io::Result<ArchiveDownloadDescriptor> {
    let permit = host
        .archives()
        .capacity
        .clone()
        .try_acquire_owned()
        .map_err(io::Error::other)?;
    let cancel = CancellationToken::new();
    let _on_drop = cancel.clone().drop_guard();
    let cut = host
        .manager()
        .session_controller()
        .prepare_archive(session, cancel)
        .await?;
    let filename = cut.filename();
    let mut secret = [0; 32];
    getrandom::fill(&mut secret).map_err(io::Error::other)?;
    let path = format!(
        "{PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
    );
    let mut loopback_port = None;
    let listener = if host.archives().remote.load(Ordering::Relaxed) {
        None
    } else {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        loopback_port = Some(listener.local_addr()?.port());
        Some(listener)
    };
    {
        let mut prepared = host.archives().prepared.lock().expect("archive mutex");
        prepared.retain(|_, p| p.expires > Instant::now());
        prepared.insert(
            path.clone(),
            Prepared {
                _permit: permit,
                cut,
                expires: Instant::now() + TTL,
            },
        );
    }
    let owner = host.clone();
    let expiry_path = path.clone();
    tokio::spawn(async move {
        tokio::time::sleep(TTL).await;
        owner
            .archives()
            .prepared
            .lock()
            .expect("archive mutex")
            .remove(&expiry_path);
    });
    if let Some(listener) = listener {
        let owner = host.clone();
        let local_path = path.clone();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + TTL;
            loop {
                let Ok(Ok((socket, _))) =
                    tokio::time::timeout_at(deadline, listener.accept()).await
                else {
                    break;
                };
                let _ = local_download(socket, owner.clone()).await;
                if !owner
                    .archives()
                    .prepared
                    .lock()
                    .expect("archive mutex")
                    .contains_key(&local_path)
                {
                    break;
                }
            }
        });
    }
    Ok(ArchiveDownloadDescriptor {
        path,
        filename,
        expires_in_seconds: 60,
        loopback_port,
    })
}

async fn local_download(mut socket: tokio::net::TcpStream, host: AppServerHost) -> io::Result<()> {
    let path = tokio::time::timeout(Duration::from_secs(5), async {
        let line = read_line(&mut socket).await?;
        let text = std::str::from_utf8(&line).map_err(io::Error::other)?;
        let path = text
            .strip_prefix("GET ")
            .and_then(|s| s.strip_suffix(" HTTP/1.1\r\n"))
            .ok_or_else(|| io::Error::other("invalid archive request"))?
            .to_owned();
        read_headers(&mut socket).await?;
        Ok::<_, io::Error>(path)
    })
    .await
    .map_err(io::Error::other)??;
    download(&mut socket, &host, &path).await
}
async fn read_headers(socket: &mut (impl AsyncRead + Unpin)) -> io::Result<()> {
    let mut total = 0;
    loop {
        let header = read_line(socket).await?;
        total += header.len();
        if total > 16384 {
            return Err(io::Error::other("headers too large"));
        }
        if header == b"\r\n" {
            return Ok(());
        }
    }
}

/// Classify one bounded HTTP request line and replay it untouched for the
/// existing authenticated WebSocket handshake. Slow clients have a deadline.
pub(crate) async fn dispatch(
    mut socket: tokio::net::TcpStream,
    host: AppServerHost,
    credential: super::transport::websocket::Credential,
    shutdown: CancellationToken,
) -> io::Result<()> {
    let line = tokio::select! {
        () = shutdown.cancelled() => return Ok(()),
        result = tokio::time::timeout(Duration::from_secs(5), read_line(&mut socket)) => result.map_err(io::Error::other)??,
    };
    let text = std::str::from_utf8(&line).map_err(io::Error::other)?;
    if text.starts_with(&format!("GET {PREFIX}")) {
        let path = text
            .strip_prefix("GET ")
            .and_then(|s| s.strip_suffix(" HTTP/1.1\r\n"))
            .ok_or_else(|| io::Error::other("invalid archive request"))?
            .to_owned();
        tokio::select! {
            () = shutdown.cancelled() => Ok(()),
            result = async {
                tokio::time::timeout(Duration::from_secs(5), read_headers(&mut socket)).await.map_err(io::Error::other)??;
                download(&mut socket, &host, &path).await
            } => result,
        }
    } else {
        // Preserve the established WebSocket endpoint's cooperative shutdown.
        super::transport::websocket::connection(
            Replay(std::io::Cursor::new(line).chain(socket)),
            host,
            credential,
            shutdown,
        )
        .await
    }
}

async fn read_line(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    while line.len() < 4096 {
        let b = reader.read_u8().await?;
        line.push(b);
        if b == b'\n' {
            return Ok(line);
        }
    }
    Err(io::Error::other("request line too large"))
}
async fn download(
    socket: &mut (impl AsyncWrite + Unpin),
    host: &AppServerHost,
    path: &str,
) -> io::Result<()> {
    let Some(Prepared { cut, _permit, .. }) = host.archives().take(path) else {
        return socket.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n").await;
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; filename=\"{}\"\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        cut.filename()
    );
    socket.write_all(header.as_bytes()).await?;
    let mut stream = cut.stream();
    while let Some(bytes) = stream.recv().await {
        let bytes = bytes?;
        if bytes.is_empty() {
            continue;
        }
        let write = async {
            socket
                .write_all(format!("{:x}\r\n", bytes.len()).as_bytes())
                .await?;
            socket.write_all(&bytes).await?;
            socket.write_all(b"\r\n").await
        };
        tokio::time::timeout(Duration::from_secs(30), write)
            .await
            .map_err(io::Error::other)??;
    }
    // Only successful producer EOF receives the terminating HTTP chunk.
    socket.write_all(b"0\r\n\r\n").await
}

struct Replay(tokio::io::Chain<std::io::Cursor<Vec<u8>>, tokio::net::TcpStream>);
impl AsyncRead for Replay {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_read(cx, buf)
    }
}
impl AsyncWrite for Replay {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::pin::Pin::new(self.0.get_mut().1).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(self.0.get_mut().1).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(self.0.get_mut().1).poll_shutdown(cx)
    }
}
