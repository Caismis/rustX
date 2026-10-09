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
    /// Delivery operation registered, before native admission.
    pub before_admission: Pause,
    /// Admitted and holding a permit, before the descriptor walk opens anything.
    pub before_open: std::sync::Arc<crate::runtime::conversation_runtime::Gate>,
    pub before_bytes: std::sync::Arc<crate::runtime::conversation_runtime::Gate>,
    pub completed: tokio::sync::watch::Sender<Option<bool>>,
    pub retirement_waiting: tokio::sync::watch::Sender<bool>,
    /// A `delivery/cancel` found its request's publication in progress and waited.
    pub cancel_waited: tokio::sync::watch::Sender<bool>,
    /// A credential rotation found its slot held by an authentication and waited.
    pub credential_waited: tokio::sync::watch::Sender<bool>,
    /// Inside an authentication, holding its credential slot.
    pub authenticating: std::sync::Arc<crate::runtime::conversation_runtime::Gate>,
    /// Delivery credentials in the order rotations installed them.
    pub rotations: Rotations,
    pub authority: std::sync::Mutex<Option<CancellationToken>>,
}
#[cfg(test)]
impl Default for ReadProbe {
    fn default() -> Self {
        Self {
            before_admission: Pause::default(),
            before_open: std::sync::Arc::default(),
            before_bytes: std::sync::Arc::default(),
            completed: tokio::sync::watch::channel(None).0,
            retirement_waiting: tokio::sync::watch::channel(false).0,
            cancel_waited: tokio::sync::watch::channel(false).0,
            credential_waited: tokio::sync::watch::channel(false).0,
            authenticating: std::sync::Arc::default(),
            rotations: Rotations::default(),
            authority: std::sync::Mutex::default(),
        }
    }
}

/// What each delivery credential rotation installed, in its linearization
/// order. Secrets are never printed.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Rotations(std::sync::Mutex<Vec<Option<super::transport::websocket::Credential>>>);
#[cfg(test)]
impl std::fmt::Debug for Rotations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rotations")
    }
}
#[cfg(test)]
impl Rotations {
    pub(crate) fn record(&self, installed: Option<super::transport::websocket::Credential>) {
        self.0.lock().expect("rotations").push(installed);
    }
    pub(crate) fn take(&self) -> Vec<Option<super::transport::websocket::Credential>> {
        std::mem::take(&mut self.0.lock().expect("rotations"))
    }
}

/// A one-shot async test boundary. Unarmed, it never waits.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Pause {
    armed: std::sync::atomic::AtomicBool,
    entered: tokio::sync::watch::Sender<bool>,
    proceed: tokio::sync::watch::Sender<bool>,
}
#[cfg(test)]
impl Default for Pause {
    fn default() -> Self {
        Self {
            armed: std::sync::atomic::AtomicBool::new(false),
            entered: tokio::sync::watch::channel(false).0,
            proceed: tokio::sync::watch::channel(false).0,
        }
    }
}
#[cfg(test)]
impl Pause {
    /// The next [`Pause::enter`] parks until [`Pause::release`].
    pub(crate) fn arm(&self) {
        self.entered.send_replace(false);
        self.proceed.send_replace(false);
        self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub(crate) async fn enter(&self) {
        if !self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let mut proceed = self.proceed.subscribe();
        self.entered.send_replace(true);
        let _ = proceed.wait_for(|proceed| *proceed).await;
    }
    pub(crate) async fn wait_entered(&self) {
        let _ = self.entered.subscribe().wait_for(|entered| *entered).await;
    }
    pub(crate) fn release(&self) {
        self.proceed.send_replace(true);
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
///
/// The response is published like an ordinary-lane delivery response: its
/// [`delivery_access::Publication`] decides at tungstenite's acceptance of the
/// frame, against the same revocation order.
pub(crate) async fn serve<S>(
    socket: tokio_tungstenite::WebSocketStream<S>,
    host: AppServerHost,
    authorization: CancellationToken,
    shutdown: CancellationToken,
) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    use tokio_tungstenite::tungstenite::Message;
    let _retire = authorization.clone().drop_guard();
    let socket = super::transport::websocket::Socket::new(socket);
    let request = tokio::select! {
        () = shutdown.cancelled() => return Ok(()),
        () = authorization.cancelled() => return Ok(()),
        request = tokio::time::timeout(super::transport::websocket::HANDSHAKE_TIMEOUT, socket.next()) => request.map_err(io::Error::other)?,
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
        _ = socket.next() => None, // close, EOF, error or a second payload revokes this single operation
        result = &mut read => Some(result),
    };
    let Some((result, publication)) = result else {
        host.revocations().revoke(|| authorization.cancel());
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
        return tokio::time::timeout(super::transport::WRITE_TIMEOUT, socket.close())
            .await
            .map_err(io::Error::other)?;
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
    let outbound = super::transport::Outbound::reply(super::connection::Reply {
        response,
        publication,
    })?;
    tokio::select! {
        biased;
        () = shutdown.cancelled() => (),
        () = authorization.cancelled() => (),
        result = tokio::time::timeout(super::transport::WRITE_TIMEOUT, socket.send(outbound)) => {
            result.map_err(io::Error::other)??;
        },
    }
    // A read already retired before cancellation can acknowledge the same clean
    // close, without turning a known settlement into a transport-loss outcome.
    tokio::time::timeout(super::transport::WRITE_TIMEOUT, socket.close())
        .await
        .map_err(io::Error::other)?
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
/// The read and, once its route is found, the publication that decides its
/// response.
async fn read(
    host: AppServerHost,
    request: FileRead,
    authorization: CancellationToken,
) -> (
    Result<MethodResult, RpcError>,
    Option<delivery_access::Publication>,
) {
    if let Err(error) = delivery_access::check_rpc(&authorization) {
        return (Err(error), None);
    }
    let Some(route) = host.file_route(&request.target) else {
        return (Err(domain(ErrorData::StaleAttachment)), None);
    };
    // A socket's one operation: its own table, under the fixed id 0.
    let publication = std::sync::Arc::new(delivery_access::Operations::default())
        .register(
            &RequestId::Integer(0),
            &authorization,
            route.clone(),
            host.revocations().clone(),
            #[cfg(test)]
            host.file_read_probe(),
        )
        .expect("a fresh table has no operation");
    let token = publication.token();
    let FileRead { source, roots, .. } = request;
    let result = match source {
        ReadSource::SessionFile {
            message_id,
            delivery_index,
        } => {
            delivery_access::request(
                host,
                route,
                message_id,
                delivery_index,
                delivery_access::Roots::Registered(roots),
                delivery_access::Access::Bytes,
                token,
            )
            .await
        }
        ReadSource::Artifact { artifact_id } => artifact(host, route, artifact_id, token).await,
    };
    (result, Some(publication))
}

async fn artifact(
    host: AppServerHost,
    route: std::sync::Arc<super::connection::Route>,
    artifact_id: crate::runtime::ArtifactId,
    authorization: CancellationToken,
) -> Result<MethodResult, RpcError> {
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
