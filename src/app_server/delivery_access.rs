//! The one native owner of committed `present` delivery access.
//!
//! Two transport-authenticated callers enter it: the private Product Host lane
//! (whose roots come from its current Workspace registrations) and a connection
//! holding delivery access (stdio owner or separate WebSocket credential).
//! Neither JSON fields, client names, coordinates nor paths create authority:
//! the caller passes a cancellation token minted by transport authentication.
//!
//! Each request is one [`Operation`] with one [`Publication`], from
//! registration to its single terminal decision:
//!
//! ```text
//! register (exact JSON-RPC id) -> native admission -> fences -> physical settlement
//!   -> response queued -> writer: [shared] decide + transport accepts -> wire
//! delivery/cancel (same connection, same id) -> Running => Cancelled, token cancelled
//! revocation (credential, connection, close, detach, drain) -> [exclusive] authority revoked
//! ```
//!
//! The publication linearization point is the transport's acceptance of the
//! response ([`Publication::poll_hand_off`]): the stdio pipe taking its first
//! bytes, or tungstenite taking its frame. The decision and that acceptance
//! run in one synchronous step that holds the operation's lock and the shared
//! side of the host's [`Revocations`], with no suspension between them, and
//! the request is settled only if the transport accepted. Every revocation of
//! delivery authority runs in the exclusive side. So a cancellation or
//! revocation that completes before the acceptance replaces the response with
//! its typed failure for the same id, one that overlaps it waits for it and is
//! ordered after it, and accepted bytes are never retracted.
use super::{
    connection::{Route, client_error, domain, host_error, manager_error},
    host::AppServerHost,
    protocol::{ErrorData, MethodResult, RequestId, Response, RpcError},
};
use crate::tools::session_files::SessionFileReadFailure as FileFailure;
use base64::Engine;
use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    task::{Poll, ready},
};
use tokio_util::sync::CancellationToken;

/// Which canonical roots may contain the original Session cwd.
pub(crate) enum Roots {
    /// The Product Host's current registrations, checked against the mapping.
    Registered(Vec<PathBuf>),
    /// The original Session's current native mapping alone. Delivery access
    /// is native-process authority; no external registration list applies.
    OriginalMapping,
}

#[derive(Clone, Copy)]
pub(crate) enum Access {
    /// Bounded original bytes, at most 512 KiB.
    Bytes,
    /// The verified absolute path and leaf identity; no bytes.
    Location,
}

pub(crate) fn check(authorization: &CancellationToken) -> io::Result<()> {
    if authorization.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "delivery access revoked",
        ));
    }
    Ok(())
}
pub(crate) fn check_rpc(authorization: &CancellationToken) -> Result<(), RpcError> {
    check(authorization).map_err(|_| unauthorized())
}
pub(crate) fn unauthorized() -> RpcError {
    domain(ErrorData::SessionFileRead {
        reason: FileFailure::Unauthorized,
    })
}

/// Admit one access as a detached native operation on the exact attachment.
/// Dropping the caller's waiter never cancels it: physical settlement, not the
/// response, releases the native read permit.
pub(super) async fn request(
    host: AppServerHost,
    route: Arc<Route>,
    message_id: crate::runtime::identity::MessageId,
    delivery_index: usize,
    roots: Roots,
    access: Access,
    authorization: CancellationToken,
) -> Result<MethodResult, RpcError> {
    check_rpc(&authorization)?;
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
                    let result = resolve(
                        owner,
                        &route,
                        &authority.map_err(client_error)?,
                        sessions,
                        message_id,
                        delivery_index,
                        roots,
                        access,
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

/// Every fence runs here: authenticated admission, before lookup/allocation,
/// before open, after leaf open immediately before bytes or identity, after
/// edge verification, and before publication.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn resolve(
    host: AppServerHost,
    route: &Arc<Route>,
    authority: &Arc<crate::runtime_client::host::ClientInner>,
    sessions: crate::local_runtime::session_controller::SessionController,
    message_id: crate::runtime::identity::MessageId,
    delivery_index: usize,
    roots: Roots,
    access: Access,
    authorization: CancellationToken,
) -> Result<MethodResult, RpcError> {
    check_rpc(&authorization)?;
    #[cfg(test)]
    {
        *host
            .file_read_probe()
            .authority
            .lock()
            .expect("read authority probe") = Some(authorization.clone());
    }
    let failed = |reason| domain(ErrorData::SessionFileRead { reason });
    if delivery_index >= crate::tools::session_files::PRESENT_MAX_FILES
        || matches!(&roots, Roots::Registered(roots) if roots.is_empty()
            || roots.len() > 32
            || roots.iter().any(|p| !p.is_absolute() || p.as_os_str().len() > 4096))
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
    let access_lease = sessions
        .acquire_session(&session, Some(&node))
        .await
        .map_err(|_| failed(FileFailure::Unavailable))?;
    let reference = file.clone();
    let read_route = route.clone();
    let read_authorization = authorization.clone();
    let catalog = sessions.catalog.clone();
    #[cfg(test)]
    let probe = host.file_read_probe();
    let resolved = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _allocation = access_lease.allocation;
        let mapped_cwd = access_lease.settings.cwd;
        #[cfg(test)]
        let fences = std::sync::atomic::AtomicUsize::new(0);
        let authorized = || {
            // First call: admitted, before the descriptor walk opens anything.
            // Third: after the unchanged descriptor owner's leaf open,
            // immediately before it reads any bytes or publishes identity.
            #[cfg(test)]
            match fences.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => probe.before_open.enter(),
                2 => probe.before_bytes.enter(),
                _ => {}
            }
            check(&read_authorization)?;
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
        if let Roots::Registered(roots) = &roots
            && !roots.contains(&root)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "delivery Workspace not authorized by Product Host",
            ));
        }
        let resolved = match access {
            Access::Bytes => Resolved::Bytes(crate::tools::session_files::read_authorized(
                &root, &reference, authorized,
            )?),
            Access::Location => Resolved::Location(crate::tools::session_files::locate_authorized(
                &root, &reference, authorized,
            )?),
        };
        authorized()?;
        Ok::<_, std::io::Error>(resolved)
    })
    .await
    .map_err(|_| domain(ErrorData::OperationFailed))?
    .map_err(|error| failed(crate::tools::session_files::read_failure(&error)))?;
    route.attachment.read_authority().map_err(client_error)?;
    check_rpc(&authorization)?;
    Ok(match resolved {
        Resolved::Bytes(bytes) => MethodResult::SessionFileBytes {
            file,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        },
        Resolved::Location(location) => MethodResult::SessionFileLocation {
            file,
            path: location
                .path
                .into_os_string()
                .into_string()
                .map_err(|_| failed(FileFailure::Unavailable))?,
            device: location.device.to_string(),
            inode: location.inode.to_string(),
        },
    })
}

enum Resolved {
    Bytes(Vec<u8>),
    Location(crate::tools::session_files::SessionFileLocation),
}

/// Orders every revocation of delivery authority against every publication.
///
/// A delivery success may be published only while its authority is current:
/// the connection's (or Product Host socket's) authority token is not
/// cancelled and its exact attachment is attached. Every change of either to
/// revoked runs in [`Self::revoke`], the exclusive side: credential
/// replacement or removal, connection revocation, drain and close, attachment
/// detach, and Product Host disconnect or replacement. A publication holds
/// the shared side from its decision through the transport's acceptance
/// ([`Publication::poll_hand_off`]). Hence each revocation is linearized at
/// its exclusive section and each publication at its acceptance, and no
/// revocation can complete between a publication's check and its acceptance.
///
/// Neither side suspends or waits for I/O: the shared side covers one
/// non-suspending transport poll, the exclusive side a token cancellation or
/// an attachment detach. Publications share the lock, so they never wait for
/// one another, only for a revocation in progress.
#[derive(Default)]
pub(crate) struct Revocations {
    order: RwLock<()>,
    /// How many revocations found a publication in progress and waited.
    #[cfg(test)]
    pub(crate) waited: tokio::sync::watch::Sender<usize>,
}
impl std::fmt::Debug for Revocations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeliveryRevocations")
    }
}
impl Revocations {
    /// Revokes delivery authority: `revoke` runs once no publication is
    /// between its decision and its acceptance, and before any later one
    /// decides.
    pub(crate) fn revoke<T>(&self, revoke: impl FnOnce() -> T) -> T {
        #[cfg(test)]
        if self.order.try_write().is_err() {
            self.waited.send_modify(|waited| *waited += 1);
        }
        let _exclusive = self.order.write().expect("delivery revocations");
        revoke()
    }

    fn publish<T>(&self, publish: impl FnOnce() -> T) -> T {
        let _shared = self.order.read().expect("delivery revocations");
        publish()
    }
}

/// WebSocket clients offer this beside the ordinary transport credential.
pub(crate) const CREDENTIAL_PREFIX: &str = "rustx-delivery-access.";

/// The separate WebSocket delivery credential. Distinct from the ordinary
/// transport token and the Product Host secret; never serialized. Dropping or
/// replacing it synchronously revokes every connection it admitted.
pub(crate) struct Grant {
    credential: super::transport::websocket::Credential,
    revoked: CancellationToken,
}
impl std::fmt::Debug for Grant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeliveryAccessGrant")
    }
}
impl Grant {
    pub(crate) fn new(credential: super::transport::websocket::Credential) -> Self {
        Self {
            credential,
            revoked: CancellationToken::new(),
        }
    }
    pub(crate) fn authenticate(&self, offered: &[&str]) -> Option<CancellationToken> {
        (self.credential.offered(offered, CREDENTIAL_PREFIX) && !self.revoked.is_cancelled())
            .then(|| self.revoked.child_token())
    }
}
impl Drop for Grant {
    fn drop(&mut self) {
        self.revoked.cancel();
    }
}

/// Where one delivery request stands relative to its publication.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Running,
    /// `delivery/cancel` won: the terminal response is `delivery_cancelled`.
    Cancelled,
    /// The transport accepted the decided response; nothing applies any more.
    Settled,
}

/// One in-flight delivery request.
pub(super) struct Operation {
    state: Mutex<State>,
    /// A child of the delivery authority. Cancelling it (by `delivery/cancel`,
    /// credential revocation or close) fails every native fence the request
    /// has not passed yet.
    token: CancellationToken,
    #[cfg(test)]
    probe: Arc<super::product_host::ReadProbe>,
}

/// One connection's delivery requests, keyed by their exact JSON-RPC ids.
/// Only that connection can name them: a cancel from anywhere else finds an
/// unrelated (or empty) table.
#[derive(Default)]
pub(super) struct Operations(Mutex<std::collections::HashMap<String, Arc<Operation>>>);

fn key(id: &RequestId) -> String {
    serde_json::to_string(id).expect("request id serializes")
}

impl Operations {
    /// Registers one request before its native admission. An id already in
    /// flight on this connection is refused, so a cancel names exactly one
    /// operation.
    pub(super) fn register(
        self: &Arc<Self>,
        id: &RequestId,
        authority: &CancellationToken,
        route: Arc<Route>,
        revocations: Arc<Revocations>,
        #[cfg(test)] probe: Arc<super::product_host::ReadProbe>,
    ) -> Option<Publication> {
        let operation = Arc::new(Operation {
            state: Mutex::new(State::Running),
            token: authority.child_token(),
            #[cfg(test)]
            probe,
        });
        let mut table = self.0.lock().expect("delivery operations");
        if table.contains_key(&key(id)) {
            return None;
        }
        table.insert(key(id), operation.clone());
        Some(Publication {
            operations: self.clone(),
            id: id.clone(),
            operation,
            authority: authority.clone(),
            route,
            revocations,
        })
    }

    /// Cancels this connection's exact running request. `true` means the
    /// cancellation won: that request's only response is `delivery_cancelled`.
    /// `false` means no such request is running here (unknown, never
    /// registered, or its publication was already accepted). A cancel that
    /// arrives during a publication's hand-off waits for it, so it is
    /// refused exactly when the transport accepted.
    pub(super) fn cancel(&self, id: &RequestId) -> bool {
        let Some(operation) = self
            .0
            .lock()
            .expect("delivery operations")
            .get(&key(id))
            .cloned()
        else {
            return false;
        };
        #[cfg(test)]
        if operation.state.try_lock().is_err() {
            operation.probe.cancel_waited.send_replace(true);
        }
        let mut state = operation.state.lock().expect("delivery operation");
        match *state {
            State::Running => {
                *state = State::Cancelled;
                operation.token.cancel();
                true
            }
            State::Cancelled => true,
            State::Settled => false,
        }
    }
}

/// The publication owner of one delivery response. It travels with the
/// serialized response to the transport writer, which settles it when the
/// transport accepts the response (the in-process caller settles it when it
/// returns). Dropping it unsettled publishes nothing and unregisters the
/// request.
pub(super) struct Publication {
    operations: Arc<Operations>,
    id: RequestId,
    operation: Arc<Operation>,
    authority: CancellationToken,
    route: Arc<Route>,
    revocations: Arc<Revocations>,
}

impl Publication {
    /// The request's cancellation token, passed to every native fence.
    pub(super) fn token(&self) -> CancellationToken {
        self.operation.token.clone()
    }

    #[cfg(test)]
    pub(super) fn probe(&self) -> &super::product_host::ReadProbe {
        &self.operation.probe
    }

    /// What publishing now would mean. `Ok` publishes the response as
    /// produced; `Err` is the typed terminal failure that replaces it.
    ///
    /// A cancellation accepted earlier always wins. A success (bytes or a
    /// native path) additionally requires its authority to be current: the
    /// authority token and the exact attachment. A failure carries nothing
    /// sensitive and keeps its own reason. Called only with the operation's
    /// lock and the shared side of [`Revocations`] held, so neither can
    /// change before the caller's acceptance.
    fn decide(&self, state: State, success: bool) -> Result<(), RpcError> {
        match state {
            State::Cancelled => Err(domain(ErrorData::DeliveryCancelled)),
            State::Settled | State::Running if !success => Ok(()),
            State::Settled | State::Running => check_rpc(&self.authority).and_then(|()| {
                self.route
                    .attachment
                    .read_authority()
                    .map(|_| ())
                    .map_err(client_error)
            }),
        }
    }

    /// Commits a typed response for the in-process caller, whose return is
    /// its acceptance.
    pub(super) fn publish(&self, response: Response) -> Response {
        let success = matches!(response, Response::Success(_));
        let decision = self.revocations.publish(|| {
            let mut state = self.operation.state.lock().expect("delivery operation");
            let decision = self.decide(*state, success);
            *state = State::Settled;
            decision
        });
        match decision {
            Ok(()) => response,
            Err(error) => super::connection::failure(Some(self.id.clone()), error),
        }
    }

    /// The publication linearization point for a transport writer.
    ///
    /// `accept` is the transport's non-suspending acceptance of a record:
    /// `Pending` means it took nothing. Holding the operation's lock and the
    /// shared side of [`Revocations`], this decides which record to publish
    /// (`record` as produced, or the same id's typed failure), offers it to
    /// `accept`, and settles the request only when `accept` is ready. A
    /// cancellation or revocation either completed before, and is observed
    /// by the decision, or waits until the transport has accepted. Returns
    /// the replacement record, if any, and what `accept` returned.
    pub(super) fn poll_hand_off<T>(
        &self,
        record: &str,
        success: bool,
        accept: impl FnOnce(&str) -> Poll<io::Result<T>>,
    ) -> Poll<io::Result<(Option<String>, T)>> {
        self.revocations.publish(|| {
            let mut state = self.operation.state.lock().expect("delivery operation");
            let replacement = match self.decide(*state, success) {
                Ok(()) => None,
                Err(error) => Some(super::transport::serialize_record(
                    &super::connection::failure(Some(self.id.clone()), error),
                )?),
            };
            let accepted = ready!(accept(replacement.as_deref().unwrap_or(record)));
            *state = State::Settled;
            Poll::Ready(accepted.map(|accepted| (replacement, accepted)))
        })
    }
}

impl Drop for Publication {
    fn drop(&mut self) {
        let mut table = self.operations.0.lock().expect("delivery operations");
        if table
            .get(&key(&self.id))
            .is_some_and(|current| Arc::ptr_eq(current, &self.operation))
        {
            table.remove(&key(&self.id));
        }
    }
}
