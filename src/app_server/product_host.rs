//! Private Product Host file-read seam. Never an ordinary App Server Method.
//! Transport authentication creates the read authority; JSON cannot create it.
use super::{
    connection::{Route, client_error, domain, host_error, manager_error},
    host::AppServerHost,
    protocol::{
        AttachmentTarget, ErrorData, Failure, JsonRpcVersion, MethodResult, RequestId, Response,
        RpcError, Success,
    },
};
use crate::tools::session_files::SessionFileReadFailure as FileFailure;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

pub(crate) const PATH: &str = "/product-host/file-read";
pub(crate) const SUBPROTOCOL: &str = "rustx.product-host.file-read.v2";

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct ReadProbe {
    pub before_bytes: Arc<crate::runtime::conversation_runtime::Gate>,
    pub completed: tokio::sync::watch::Sender<Option<bool>>,
    pub authority: std::sync::Mutex<Option<CancellationToken>>,
}
#[cfg(test)]
impl Default for ReadProbe {
    fn default() -> Self {
        Self {
            before_bytes: Arc::default(),
            completed: tokio::sync::watch::channel(None).0,
            authority: std::sync::Mutex::default(),
        }
    }
}

pub(crate) struct Authority {
    credential: super::transport::websocket::Credential,
    revoked: CancellationToken,
}
impl std::fmt::Debug for Authority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProductHostAuthority")
    }
}
impl Authority {
    pub(crate) fn new(credential: super::transport::websocket::Credential) -> Self {
        Self {
            credential,
            revoked: CancellationToken::new(),
        }
    }
    pub(crate) fn authenticate(&self, offered: &[&str]) -> Option<CancellationToken> {
        (offered.contains(&SUBPROTOCOL)
            && self.credential.offered(offered, "rustx-product-host.")
            && !self.revoked.is_cancelled())
        .then(|| self.revoked.child_token())
    }
}
impl Drop for Authority {
    fn drop(&mut self) {
        self.revoked.cancel();
    }
}

/// One authenticated host-only socket owns one read. Disconnect, authority
/// replacement and process shutdown synchronously cancel its publication fence.
pub(crate) async fn serve<S>(
    socket: tokio_tungstenite::WebSocketStream<S>,
    host: AppServerHost,
    authorization: CancellationToken,
    shutdown: CancellationToken,
) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let _retire = authorization.clone().drop_guard();
    let (mut writer, mut reader) = socket.split();
    let request = tokio::select! {
        () = shutdown.cancelled() => return Ok(()),
        () = authorization.cancelled() => return Ok(()),
        request = tokio::time::timeout(super::transport::websocket::HANDSHAKE_TIMEOUT, reader.next()) => request.map_err(io::Error::other)?,
    };
    let Some(Ok(Message::Text(text))) = request else {
        return Err(io::Error::other("expected Product Host read"));
    };
    let request: FileRead =
        serde_json::from_str(&text).map_err(|_| io::Error::other("invalid Product Host read"))?;
    let read = read(host.clone(), request, authorization.clone());
    tokio::pin!(read);
    let result = tokio::select! {
        biased;
        () = shutdown.cancelled() => return Ok(()),
        () = authorization.cancelled() => return Ok(()),
        _ = reader.next() => return Ok(()), // close, EOF, error or a second payload revokes this single operation
        result = &mut read => result,
    };
    check_authorization(&authorization)?;
    let response = match result {
        Ok(result) => Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(0),
            result,
        })),
        Err(error) => Response::Failure(Failure {
            jsonrpc: JsonRpcVersion::V2,
            id: Some(RequestId::Integer(0)),
            error,
        }),
    };
    let record = super::transport::serialize_record(&response)?;
    tokio::select! {
        biased;
        () = shutdown.cancelled() => Ok(()),
        () = authorization.cancelled() => Ok(()),
        result = tokio::time::timeout(super::transport::WRITE_TIMEOUT, async {
            writer.send(Message::Text(record.into())).await.map_err(io::Error::other)?;
            writer.close().await.map_err(io::Error::other)
        }) => result.map_err(io::Error::other)?,
    }
}

/// Internal one-operation payload, accepted only after host-only authentication.
/// No credential is carried in JSON, URLs, history or ordinary protocol logs.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(remote = "Self", deny_unknown_fields)]
pub(crate) struct FileRead {
    pub target: AttachmentTarget,
    pub source: ReadSource,
    pub roots: Vec<PathBuf>,
}
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ReadSource {
    SessionFile {
        message_id: crate::runtime::identity::MessageId,
        delivery_index: usize,
    },
    Artifact {
        artifact_id: crate::runtime::ArtifactId,
    },
}
fn check_authorization(authorization: &CancellationToken) -> io::Result<()> {
    if authorization.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Product Host authority revoked",
        ));
    }
    Ok(())
}
fn check_rpc_authorization(authorization: &CancellationToken) -> Result<(), RpcError> {
    check_authorization(authorization).map_err(|_| {
        domain(ErrorData::SessionFileRead {
            reason: crate::tools::session_files::SessionFileReadFailure::Unauthorized,
        })
    })
}

pub(crate) async fn read(
    host: AppServerHost,
    request: FileRead,
    authorization: CancellationToken,
) -> Result<MethodResult, RpcError> {
    check_rpc_authorization(&authorization)?;
    let route = host
        .file_route(&request.target)
        .ok_or_else(|| domain(ErrorData::StaleAttachment))?;
    let client = route.client.clone();
    let sessions = host.manager().session_controller();
    let owner = host.clone();
    let receiver = host
        .admit_request(|request_owner| {
            client.start_operation(move || {
                let authority = route.attachment.read_authority();
                async move {
                    let _request = request_owner;
                    #[cfg(test)]
                    let probe = owner.file_read_probe();
                    let result = read_admitted(
                        owner,
                        request,
                        route,
                        authority.map_err(client_error)?,
                        sessions,
                        authorization,
                    )
                    .await;
                    #[cfg(test)]
                    probe.completed.send_replace(Some(result.is_ok()));
                    result
                }
            })
        })
        .map_err(host_error)?
        .map_err(manager_error)?;
    receiver
        .await
        .map_err(|_| domain(ErrorData::OperationFailed))?
}
#[allow(clippy::too_many_lines)] // Existing native file owner; admission and every read fence stay together.
async fn read_admitted(
    host: AppServerHost,
    request: FileRead,
    route: Arc<Route>,
    authority: Arc<crate::runtime_client::host::ClientInner>,
    sessions: crate::local_runtime::session_controller::SessionController,
    authorization: CancellationToken,
) -> Result<MethodResult, RpcError> {
    check_rpc_authorization(&authorization)?;
    #[cfg(test)]
    {
        *host
            .file_read_probe()
            .authority
            .lock()
            .expect("read authority probe") = Some(authorization.clone());
    }
    let FileRead {
        source,
        roots: allowed_roots,
        ..
    } = request;
    let (message_id, delivery_index) = match source {
        ReadSource::SessionFile {
            message_id,
            delivery_index,
        } => (message_id, delivery_index),
        ReadSource::Artifact { artifact_id } => {
            let _permit = host.file_reads().try_acquire_owned().map_err(|_| {
                domain(ErrorData::SessionFileRead {
                    reason: FileFailure::Capacity,
                })
            })?;
            let data = authority
                .artifact_read(&artifact_id)
                .map_err(client_error)?;
            route.attachment.read_authority().map_err(client_error)?;
            check_rpc_authorization(&authorization)?;
            return Ok(MethodResult::ArtifactBytes { data });
        }
    };
    let failed = |reason| domain(ErrorData::SessionFileRead { reason });
    if delivery_index >= crate::tools::session_files::PRESENT_MAX_FILES
        || allowed_roots.is_empty()
        || allowed_roots.len() > 32
        || allowed_roots
            .iter()
            .any(|p| !p.is_absolute() || p.as_os_str().len() > 4096)
    {
        return Err(domain(ErrorData::InvalidParams));
    }
    let permit = host
        .file_reads()
        .try_acquire_owned()
        .map_err(|_| failed(FileFailure::Capacity))?;
    route.attachment.read_authority().map_err(client_error)?;
    let file = authority
        .session_file_reference(&message_id, delivery_index)
        .map_err(|_| failed(FileFailure::Unavailable))?;
    let (session, node) = sessions
        .catalog
        .lock()
        .await
        .file_source(&file.scope.conversation_id)
        .map_err(|_| failed(FileFailure::Unavailable))?;
    // Native allocation access excludes Session deletion while the read
    // is owned; acquiring it does not compose or start an Agent.
    let access = sessions
        .acquire_session(&session, Some(&node))
        .await
        .map_err(|_| failed(FileFailure::Unavailable))?;
    let reference = file.clone();
    let read_route = route.clone();
    let read_authorization = authorization.clone();
    let catalog = sessions.catalog.clone();
    #[cfg(test)]
    let probe = host.file_read_probe();
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _allocation = access.allocation;
        let mapped_cwd = access.settings.cwd;
        #[cfg(test)]
        let fences = std::sync::atomic::AtomicUsize::new(0);
        let authorized = || {
            // Third fence: after the unchanged descriptor owner's leaf open,
            // immediately before it reads any bytes.
            #[cfg(test)]
            if fences.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 2 {
                probe.before_bytes.enter();
            }
            check_authorization(&read_authorization)?;
            let mapping_current = catalog.blocking_lock().file_mapping_matches(
                &reference.scope.conversation_id,
                &session,
                &node,
                &mapped_cwd,
            );
            if !mapping_current {
                return Err(crate::tools::session_files::unavailable());
            }
            read_route
                .attachment
                .read_authority()
                .map(|_| ())
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "delivery attachment revoked",
                    )
                })
        };
        authorized()?;
        // Current native mapping only. No textual Host-path fallback.
        let root = std::fs::canonicalize(&mapped_cwd)
            .map_err(|_| crate::tools::session_files::unavailable())?;
        if !allowed_roots.contains(&root) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "delivery Workspace not authorized by Product Host",
            ));
        }
        let bytes = crate::tools::session_files::read_authorized(&root, &reference, authorized)?;
        authorized()?;
        Ok::<_, std::io::Error>(bytes)
    })
    .await
    .map_err(|_| domain(ErrorData::OperationFailed))?
    .map_err(|error| failed(crate::tools::session_files::read_failure(&error)))?;
    route.attachment.read_authority().map_err(client_error)?;
    check_rpc_authorization(&authorization)?;
    Ok(MethodResult::SessionFileBytes {
        file,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}
