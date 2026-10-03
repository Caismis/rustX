//! Short-lived binary carrier. This owner never creates or cleans upload files.
use super::{connection::Route, host::AppServerHost};
use crate::local_runtime::{
    session::uploads::{
        UPLOAD_POLICY, UploadFile, UploadMetadata, UploadOutcome, validate_metadata,
        validate_operation,
    },
    session_controller::SessionAccess,
};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
};
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
        protocol::WebSocketConfig,
    },
};
use tokio_util::sync::CancellationToken;

pub(crate) const PREFIX: &str = "/session-upload/";
const PROTOCOL: &str = "rustx.session-upload.v1";
const TTL: Duration = Duration::from_secs(60);
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadDescriptor {
    pub path: String,
    pub loopback_port: Option<u16>,
    pub expires_in_seconds: u16,
}
struct Transfer {
    route: Arc<Route>,
    operation: String,
    files: Vec<UploadMetadata>,
    expires: Instant,
    consumed: AtomicBool,
    access: SessionAccess,
    revoked: CancellationToken,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
pub(crate) struct UploadTransfers {
    #[cfg(test)]
    pub(crate) settled: tokio::sync::Notify,
    #[cfg(test)]
    pub(crate) listener_settled: tokio::sync::Notify,
    #[cfg(test)]
    pub(crate) reply_gate: Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>,
    transfers: Mutex<BTreeMap<String, Arc<Transfer>>>,
    capacity: Arc<tokio::sync::Semaphore>,
    remote: AtomicBool,
}
impl Default for UploadTransfers {
    fn default() -> Self {
        Self {
            #[cfg(test)]
            settled: tokio::sync::Notify::new(),
            #[cfg(test)]
            listener_settled: tokio::sync::Notify::new(),
            #[cfg(test)]
            reply_gate: Mutex::default(),
            transfers: Mutex::default(),
            capacity: Arc::new(tokio::sync::Semaphore::new(
                UPLOAD_POLICY.max_concurrent_transfers,
            )),
            remote: AtomicBool::new(false),
        }
    }
}
impl UploadTransfers {
    pub(crate) fn is_empty(&self) -> bool {
        self.transfers.lock().expect("upload transfers").is_empty()
    }
    pub(crate) fn revoke_prepared(&self) {
        self.remove_prepared(None);
    }
    pub(crate) fn expire_prepared(&self, now: Instant) {
        self.remove_prepared(Some(now));
    }
    fn remove_prepared(&self, expired: Option<Instant>) {
        let removed = {
            let mut transfers = self.transfers.lock().expect("upload transfers");
            let paths: Vec<_> = transfers
                .iter()
                .filter(|(_, t)| {
                    !t.consumed.load(Ordering::SeqCst) && expired.is_none_or(|now| t.expires <= now)
                })
                .map(|(path, _)| path.clone())
                .collect();
            paths
                .into_iter()
                .filter_map(|path| transfers.remove(&path))
                .collect::<Vec<_>>()
        };
        // Dropping a route can release host attachment ownership. Never do that
        // while holding the transfer mutex (admission takes host then transfer).
        for transfer in removed {
            transfer.revoked.cancel();
        }
    }
    pub(crate) fn serve_remote(&self) {
        self.remote.store(true, Ordering::Relaxed);
    }
    fn in_progress(&self, session: &crate::runtime::identity::SessionId, operation: &str) -> bool {
        self.transfers
            .lock()
            .expect("upload transfers")
            .values()
            .any(|t| &t.route.target.session_id == session && t.operation == operation)
    }
    fn take(&self, path: &str) -> Option<Arc<Transfer>> {
        let transfers = self.transfers.lock().expect("upload transfers");
        let transfer = transfers.get(path)?;
        transfer.route.attachment.operation_authority().ok()?;
        if transfer.expires <= Instant::now() || transfer.consumed.swap(true, Ordering::SeqCst) {
            return None;
        }
        Some(transfer.clone())
    }
}
fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}

pub(crate) async fn status(
    host: &AppServerHost,
    session: &crate::runtime::identity::SessionId,
    operation: &str,
) -> io::Result<UploadOutcome> {
    validate_operation(operation)?;
    // Inspect transport first: if settlement races this read, unresolved is safe;
    // absent must never be returned from a catalog snapshot preceding settlement.
    let active = host.uploads().in_progress(session, operation);
    let outcome = host
        .manager()
        .session_controller()
        .upload_status(session, operation)
        .await
        .map_err(io::Error::other)?;
    Ok(if active && outcome == UploadOutcome::Absent {
        UploadOutcome::Unresolved
    } else {
        outcome
    })
}

#[allow(clippy::too_many_lines)] // One bounded capability admission and lifetime.
pub(super) async fn prepare(
    host: &AppServerHost,
    route: Arc<Route>,
    operation: String,
    files: Vec<UploadMetadata>,
) -> io::Result<UploadDescriptor> {
    validate_operation(&operation)?;
    validate_metadata(&files)?;
    route
        .attachment
        .operation_authority()
        .map_err(|_| invalid("stale upload attachment"))?;
    let permit = host
        .uploads()
        .capacity
        .clone()
        .try_acquire_owned()
        .map_err(|_| invalid("upload transfer capacity exhausted"))?;
    let sessions = host.manager().session_controller();
    let node = sessions
        .catalog
        .lock()
        .await
        .conversation_lineage(&route.target.session_id, &route.target.conversation_id)
        .map_err(io::Error::other)?
        .0;
    let access = sessions
        .acquire_session(&route.target.session_id, Some(&node.id))
        .await
        .map_err(io::Error::other)?;
    if sessions
        .upload_status(&route.target.session_id, &operation)
        .await
        .map_err(io::Error::other)?
        != UploadOutcome::Absent
    {
        return Err(invalid("operation already allocated; read status"));
    }
    let mut secret = [0; 32];
    getrandom::fill(&mut secret).map_err(io::Error::other)?;
    let path = format!(
        "{PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
    );
    let listener = if host.uploads().remote.load(Ordering::Relaxed) {
        None
    } else {
        Some(TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?)
    };
    let loopback_port = listener
        .as_ref()
        .map(TcpListener::local_addr)
        .transpose()?
        .map(|a| a.port());
    let revoked = CancellationToken::new();
    host.prepare_upload(|| {
        let mut transfers = host.uploads().transfers.lock().expect("upload transfers");
        if transfers.values().any(|t| {
            t.route.target.session_id == route.target.session_id && t.operation == operation
        }) {
            return Err(invalid("operation already prepared"));
        }
        transfers.insert(
            path.clone(),
            Arc::new(Transfer {
                route,
                operation,
                files,
                expires: Instant::now() + TTL,
                consumed: AtomicBool::new(false),
                access,
                revoked: revoked.clone(),
                _permit: permit,
            }),
        );
        Ok(())
    })
    .map_err(|_| invalid("server draining"))??;
    let owner = host.clone();
    tokio::spawn(async move {
        tokio::time::sleep(TTL).await;
        owner.uploads().expire_prepared(Instant::now());
    });
    if let Some(listener) = listener {
        let owner = host.clone();
        let expected_path = path.clone();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + TTL;
            'accept: while let Ok(Ok((socket, _))) = tokio::select! {
                () = revoked.cancelled() => break 'accept,
                result = tokio::time::timeout_at(deadline, listener.accept()) => result,
            } {
                let _ = serve(socket, owner.clone(), Some(&expected_path)).await;
                if !owner
                    .uploads()
                    .transfers
                    .lock()
                    .expect("upload transfers")
                    .contains_key(&expected_path)
                {
                    break;
                }
            }
            drop(listener);
            #[cfg(test)]
            owner.uploads().listener_settled.notify_one();
        });
    }
    Ok(UploadDescriptor {
        path,
        loopback_port,
        expires_in_seconds: 60,
    })
}

struct Settlement {
    host: AppServerHost,
    path: String,
    _operation: super::host::ServerOperation,
}
impl Drop for Settlement {
    fn drop(&mut self) {
        let removed = {
            self.host
                .uploads()
                .transfers
                .lock()
                .expect("upload transfers")
                .remove(&self.path)
        };
        drop(removed);
        #[cfg(test)]
        self.host.uploads().settled.notify_one();
    }
}

#[allow(clippy::too_many_lines)] // Handshake, finite framing and native handoff share one settlement guard.
pub(crate) async fn serve<S: AsyncRead + AsyncWrite + Unpin + Send>(
    socket: S,
    host: AppServerHost,
    expected_path: Option<&str>,
) -> io::Result<()> {
    let expected_path = expected_path.map(str::to_owned);
    let selected = Arc::new(Mutex::new(None));
    let admitted = selected.clone();
    let owner = host.clone();
    #[allow(clippy::result_large_err)]
    let callback = move |request: &Request, mut response: Response| {
        let offered = request
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|h| h.to_str().ok());
        if expected_path
            .as_ref()
            .is_none_or(|path| path == request.uri().path())
            && request.uri().query().is_none()
            && offered == Some(PROTOCOL)
            && let Ok(Some((transfer, operation))) = owner.admit_upload(|operation| {
                let transfer = owner.uploads().take(request.uri().path())?;
                Some((transfer, operation))
            })
        {
            *admitted.lock().expect("upload handshake") =
                Some((request.uri().path().to_owned(), transfer, operation));
            response.headers_mut().insert(
                "sec-websocket-protocol",
                http::HeaderValue::from_static(PROTOCOL),
            );
            return Ok(response);
        }
        Err(http::Response::builder()
            .status(401)
            .body(Some("Unauthorized".into()))
            .expect("constant response"))
    };
    let config = WebSocketConfig::default()
        .max_message_size(Some(UPLOAD_POLICY.max_chunk_bytes))
        .max_frame_size(Some(UPLOAD_POLICY.max_chunk_bytes))
        .read_buffer_size(UPLOAD_POLICY.max_chunk_bytes)
        .write_buffer_size(0)
        .max_write_buffer_size(UPLOAD_POLICY.max_chunk_bytes + 1024);
    let handshake = tokio::time::timeout(
        Duration::from_secs(5),
        accept_hdr_async_with_config(socket, callback, Some(config)),
    )
    .await;
    let admission = selected.lock().expect("upload handshake").take();
    let (transfer, _settlement) = match admission {
        Some((path, transfer, operation)) => (
            Some(transfer),
            Some(Settlement {
                host: host.clone(),
                path,
                _operation: operation,
            }),
        ),
        None => (None, None),
    };
    let mut socket = handshake
        .map_err(io::Error::other)?
        .map_err(io::Error::other)?;
    let transfer = transfer.ok_or_else(|| invalid("missing upload admission"))?;
    let receive = async {
        let mut files = Vec::with_capacity(transfer.files.len());
        for metadata in &transfer.files {
            let mut bytes = Vec::with_capacity(metadata.size);
            while bytes.len() < metadata.size {
                socket
                    .send(Message::Text("next".into()))
                    .await
                    .map_err(io::Error::other)?;
                let message = socket
                    .next()
                    .await
                    .ok_or_else(|| invalid("truncated upload"))?
                    .map_err(io::Error::other)?;
                let Message::Binary(chunk) = message else {
                    return Err(invalid("expected upload binary chunk"));
                };
                if chunk.is_empty() || chunk.len() > metadata.size - bytes.len() {
                    return Err(invalid("upload length mismatch"));
                }
                bytes.extend_from_slice(&chunk);
            }
            files.push(UploadFile {
                name: metadata.name.clone(),
                bytes,
            });
        }
        socket
            .send(Message::Text("finish".into()))
            .await
            .map_err(io::Error::other)?;
        if socket.next().await.transpose().map_err(io::Error::other)?
            != Some(Message::Text("finish".into()))
        {
            return Err(invalid("upload has trailing or missing bytes"));
        }
        Ok(files)
    };
    let files = tokio::time::timeout(TTL, receive)
        .await
        .map_err(io::Error::other)??;
    // No cancellation from here: native settlement owns its allocation even if
    // this socket disappears. The permit and Session exclusion survive the ACK.
    let outcome = host
        .manager()
        .session_controller()
        .upload_correlated(
            &transfer.route.target.session_id,
            Some(&transfer.access.node.id),
            transfer.operation.clone(),
            files,
        )
        .await;
    #[cfg(test)]
    {
        let gate = host.uploads().reply_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            tokio::task::spawn_blocking(move || gate.enter())
                .await
                .unwrap();
        }
    }
    let message = if outcome.is_ok() { "settled" } else { "check" };
    socket
        .send(Message::Text(message.into()))
        .await
        .map_err(io::Error::other)?;
    socket.close(None).await.map_err(io::Error::other)
}

impl std::fmt::Debug for UploadTransfers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UploadTransfers")
    }
}
