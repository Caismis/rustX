//! Explicit one-Session Execution Client composition — opt-in, never discovered.
//!
//! Only explicit trusted execution flags reach this module. Bare
//! `app-server` launches nothing. A minimal Session can be enrolled
//! before configuration and Client startup. A supplied Session identity
//! is resolved through the catalog claim, and the workspace must equal
//! the endpoint's reserved logical workspace byte-for-byte, and the Client
//! executable is only ever the trusted sibling of the running Server —
//! never PATH, HOME, or a caller-chosen path. On Linux a reaped local
//! domain can be replaced only after independent physical settlement.

use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::sync::Arc;

use rustx_execution_protocol::is_normalized_absolute_path;
use rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT;

use tokio_util::sync::CancellationToken;

use crate::local_runtime::session::SessionId;
use crate::local_runtime::session_runtime_manager::{
    ExecutionListener, LocalClient, LocalExecutionEndpoint, SessionRuntimeManager,
};

/// Publish just the durable Session identity for an explicit process owner.
pub(super) async fn enroll(
    manager: &SessionRuntimeManager,
    workspace: &Path,
) -> Result<SessionId, String> {
    if !is_normalized_absolute_path(workspace) {
        return Err("execution: workspace must be a normalized absolute path".into());
    }
    if manager.environment.retirement(workspace).is_none() {
        return Err("execution: workspace is outside configured environment roots".into());
    }
    let session = manager
        .session_controller()
        .create_session(
            crate::local_runtime::session::SessionPersistentState::from_input(
                &crate::local_runtime::SessionConfigInput::new(workspace.to_path_buf()),
            ),
        )
        .await
        .map_err(|error| format!("execution: {error}"))?
        .session
        .id;
    eprintln!("rustx app-server enrolled execution Session {session}");
    Ok(session)
}

/// Reserves the Session's local execution binding and launches the sibling
/// Execution Client, returning the live child only after real admission.
///
/// Every gate runs before transport readiness, in a fail-fast order that
/// spawns nothing until each earlier check passed: Session resolution and
/// catalog claim, normalized-absolute workspace validation, byte-exact
/// equality with the endpoint's reserved workspace, sibling presence, then
/// spawn and one bounded admission. The accept deadline is the shared
/// handshake budget; the handshake's own absolute deadline still applies
/// inside admission.
///
/// # Errors
///
/// Returns a static-prefixed `local execution:` message for a malformed or
/// unknown Session, a workspace that is not normalized absolute or not the
/// Session's reserved workspace, a missing sibling executable, a spawn or
/// accept failure, an early Client exit, a deadline lapse, or a refused
/// admission. A spawned child is always killed and reaped before `Err`.
pub(super) async fn launch(
    manager: &SessionRuntimeManager,
    id: &str,
    workspace: &Path,
) -> Result<LocalClient, String> {
    let session = SessionId::parse(id).map_err(|error| format!("local execution: {error}"))?;
    if !is_normalized_absolute_path(workspace) {
        return Err("local execution: workspace must be a normalized absolute path".into());
    }
    let endpoint = manager
        .reserve_local_execution_for(&session, Some(workspace))
        .await
        .map_err(|error| format!("local execution: {error}"))?;
    LocalClient::launch(session, endpoint).await
}

/// Reserves the Session's WebSocket execution binding — publishing the
/// `0600` bootstrap bundle for the operator — and returns the bounded
/// accept task that runs until the first real admission or drain.
///
/// No Client is launched: the operator independently provisions the
/// bundle and starts `rustx-execution-client` on the remote host. The
/// reservation itself resolves the Session's workspace from the trusted
/// catalog, identical to the local path — nothing remote ever supplies
/// workspace authority.
///
/// # Errors
///
/// Returns a static-prefixed `execution:` message for a malformed Session,
/// a fenced or conflicting claim, a non-loopback listen address, an
/// invalid public URL, or an untrusted/colliding bootstrap path.
pub(super) async fn launch_remote(
    manager: &SessionRuntimeManager,
    id: &str,
    listen: std::net::SocketAddr,
    bootstrap: std::path::PathBuf,
    public_url: Option<String>,
    settlement_grant: Option<std::path::PathBuf>,
    stop: CancellationToken,
) -> Result<tokio::task::JoinHandle<()>, String> {
    let session = SessionId::parse(id).map_err(|error| format!("execution: {error}"))?;
    // The operator-facing bundle directory is created owner-only when it
    // does not exist; an existing one must already be private — the
    // server's own require_private_child enforces the exact check again
    // at publication.
    if let Some(parent) = bootstrap.parent() {
        match std::fs::metadata(parent) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err("execution: bootstrap parent is not a directory".into());
            }
            Err(_) => {
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)
                    .map_err(|error| format!("execution: bootstrap directory: {error}"))?;
            }
        }
    }
    let endpoint = manager
        .reserve_websocket_execution(&session, listen, bootstrap, public_url)
        .await
        .map_err(|error| format!("execution: {error}"))?;
    if let Some(path) = settlement_grant {
        let owner = endpoint.clone();
        let published =
            tokio::task::spawn_blocking(move || owner.publish_external_settlement_grant(&path))
                .await
                .map_err(std::io::Error::other)
                .and_then(std::convert::identity);
        if let Err(error) = published {
            let settled = endpoint.settle().await;
            return match settled {
                Ok(()) => Err(format!(
                    "execution: settlement grant publication failed: {error}"
                )),
                Err(settlement) => Err(format!(
                    "execution: settlement grant publication failed: {error}; {settlement}"
                )),
            };
        }
    }
    let ExecutionListener::WebSocket(std_listener) = endpoint.listener() else {
        return Err("execution: the Session's endpoint is not a WebSocket listener".into());
    };
    let std_listener = std_listener
        .try_clone()
        .map_err(|error| format!("execution: {error}"))?;
    std_listener
        .set_nonblocking(true)
        .map_err(|error| format!("execution: {error}"))?;
    let listener = tokio::net::TcpListener::from_std(std_listener)
        .map_err(|error| format!("execution: {error}"))?;
    Ok(tokio::spawn(accept_remote(endpoint, listener, stop)))
}

/// The loopback accept loop: keeps accepting after a failed or invalid
/// handshake until drain. An authenticated same-instance recovery still
/// enters through this listener. In-flight upgrades are
/// bounded by the permit budget and each runs off the reactor on the
/// blocking pool, never under a global lock; a job accepted but not yet
/// running sees `stop` before it can admit after drain.
async fn accept_remote(
    endpoint: Arc<LocalExecutionEndpoint>,
    listener: tokio::net::TcpListener,
    stop: CancellationToken,
) {
    // A handful of concurrent upgrades is plenty for a real Client and
    // bounds a flood of half-open handshakes.
    const MAX_IN_FLIGHT: u32 = 4;
    let in_flight = Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT as usize));
    loop {
        tokio::select! {
            () = stop.cancelled() => break,
            accepted = listener.accept() => {
                // A persistent accept failure must not spin the
                // reactor: bounded backoff, then accept again.
                let Ok((stream, _)) = accepted else {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                };
                // The handshake deadline is armed at accept time — a peer
                // queued behind a full worker pool still gets only the
                // budget measured from when its upgrade arrived.
                let deadline = std::time::Instant::now() + LOCAL_HANDSHAKE_TIMEOUT;
                let Ok(permit) = in_flight.clone().try_acquire_owned() else {
                    continue;
                };
                let endpoint = endpoint.clone();
                let stopping = stop.clone();
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    // Accepted before drain but still queued: a drain that
                    // won the race must not be followed by an admission.
                    if stopping.is_cancelled() {
                        return;
                    }
                    let Ok(stream) = stream.into_std() else { return };
                    let _ = endpoint.admit_websocket_stream_deadline(stream, deadline);
                });
            }
        }
    }
    // Bounded settlement: wait for every in-flight worker to finish its
    // handshake — each already carries its own accept-time deadline —
    // but never longer than that budget plus the loop margin.
    let _ = tokio::time::timeout(
        LOCAL_HANDSHAKE_TIMEOUT + std::time::Duration::from_secs(1),
        in_flight.acquire_many(MAX_IN_FLIGHT),
    )
    .await;
}
