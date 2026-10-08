//! Private Product Host file-read seam. Never an ordinary App Server Method.
//! Transport authentication creates the read authority; JSON cannot create it.
use super::{
    connection::{client_error, domain, host_error, manager_error},
    delivery_access,
    host::AppServerHost,
    protocol::{
        AttachmentTarget, ErrorData, Failure, JsonRpcVersion, MethodResult, RequestId, Response,
        RpcError, Success,
    },
};
use crate::tools::session_files::SessionFileReadFailure as FileFailure;
use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf};
use tokio_util::sync::CancellationToken;

pub(crate) const PATH: &str = "/product-host/file-read";
pub(crate) const SUBPROTOCOL: &str = "rustx.product-host.file-read.v2";

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct ReadProbe {
    pub before_bytes: std::sync::Arc<crate::runtime::conversation_runtime::Gate>,
    pub completed: tokio::sync::watch::Sender<Option<bool>>,
    pub retirement_waiting: tokio::sync::watch::Sender<bool>,
    pub authority: std::sync::Mutex<Option<CancellationToken>>,
}
#[cfg(test)]
impl Default for ReadProbe {
    fn default() -> Self {
        Self {
            before_bytes: std::sync::Arc::default(),
            completed: tokio::sync::watch::channel(None).0,
            retirement_waiting: tokio::sync::watch::channel(false).0,
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
/// A close acknowledgement is emitted only after its admitted read retires.
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
        () = shutdown.cancelled() => None,
        () = authorization.cancelled() => None,
        _ = reader.next() => None, // close, EOF, error or a second payload revokes this single operation
        result = &mut read => Some(result),
    };
    let Some(result) = result else {
        authorization.cancel();
        // Admission owns a detached native operation and possibly a blocking
        // descriptor read. Dropping its receiver cannot release that permit.
        // Do not poll the socket (and flush an automatic close reply) until the
        // exact operation has retired, even after its publication is revoked.
        let _ = std::future::poll_fn(|cx| {
            let result = std::future::Future::poll(read.as_mut(), cx);
            #[cfg(test)]
            if result.is_pending() {
                host.file_read_probe().retirement_waiting.send_replace(true);
            }
            result
        })
        .await;
        return tokio::time::timeout(super::transport::WRITE_TIMEOUT, writer.close())
            .await
            .map_err(io::Error::other)?
            .map_err(io::Error::other);
    };
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
        () = shutdown.cancelled() => (),
        () = authorization.cancelled() => (),
        result = tokio::time::timeout(super::transport::WRITE_TIMEOUT, writer.send(Message::Text(record.into()))) => {
            result.map_err(io::Error::other)?.map_err(io::Error::other)?;
        },
    }
    // A read already retired before cancellation can acknowledge the same clean
    // close, without turning a known settlement into a transport-loss outcome.
    tokio::time::timeout(super::transport::WRITE_TIMEOUT, writer.close())
        .await
        .map_err(io::Error::other)?
        .map_err(io::Error::other)
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
pub(crate) async fn read(
    host: AppServerHost,
    request: FileRead,
    authorization: CancellationToken,
) -> Result<MethodResult, RpcError> {
    delivery_access::check_rpc(&authorization)?;
    let route = host
        .file_route(&request.target)
        .ok_or_else(|| domain(ErrorData::StaleAttachment))?;
    let FileRead { source, roots, .. } = request;
    let artifact_id = match source {
        ReadSource::SessionFile {
            message_id,
            delivery_index,
        } => {
            return delivery_access::request(
                host,
                route,
                message_id,
                delivery_index,
                delivery_access::Roots::Registered(roots),
                delivery_access::Access::Bytes,
                authorization,
            )
            .await;
        }
        ReadSource::Artifact { artifact_id } => artifact_id,
    };
    let client = route.client.clone();
    let owner = host.clone();
    let receiver = host
        .admit_request(|request_owner| {
            client.start_operation(move || {
                let authority = route.attachment.read_authority();
                async move {
                    let _request = request_owner;
                    let authority = authority.map_err(client_error)?;
                    delivery_access::check_rpc(&authorization)?;
                    let _permit = owner.file_reads().try_acquire_owned().map_err(|_| {
                        domain(ErrorData::SessionFileRead {
                            reason: FileFailure::Capacity,
                        })
                    })?;
                    let data = authority
                        .artifact_read(&artifact_id)
                        .map_err(client_error)?;
                    route.attachment.read_authority().map_err(client_error)?;
                    delivery_access::check_rpc(&authorization)?;
                    Ok(MethodResult::ArtifactBytes { data })
                }
            })
        })
        .map_err(host_error)?
        .map_err(manager_error)?;
    receiver
        .await
        .map_err(|_| domain(ErrorData::OperationFailed))?
}
