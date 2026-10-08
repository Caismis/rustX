//! The one native owner of committed `present` delivery access.
//!
//! Two transport-authenticated callers enter it: the private Product Host lane
//! (whose roots come from its current Workspace registrations) and a connection
//! holding delivery access (stdio owner or separate WebSocket credential).
//! Neither JSON fields, client names, coordinates nor paths create authority:
//! the caller passes a cancellation token minted by transport authentication.
use super::{
    connection::{Route, client_error, domain, host_error, manager_error},
    host::AppServerHost,
    protocol::{ErrorData, MethodResult, RpcError},
};
use crate::tools::session_files::SessionFileReadFailure as FileFailure;
use base64::Engine;
use std::{io, path::PathBuf, sync::Arc};
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
            // Third fence: after the unchanged descriptor owner's leaf open,
            // immediately before it reads any bytes or publishes identity.
            #[cfg(test)]
            if fences.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 2 {
                probe.before_bytes.enter();
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
