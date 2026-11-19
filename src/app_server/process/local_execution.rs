//! Explicit one-Session Execution Client composition — opt-in, never discovered.
//!
//! Only the paired `--local-execution-session`/`--local-execution-workspace`
//! flags reach this module: bare `app-server` launches nothing. The Session
//! is resolved through the trusted catalog claim, the workspace must equal
//! the endpoint's reserved logical workspace byte-for-byte, and the Client
//! executable is only ever the trusted sibling of the running Server —
//! never PATH, HOME, or a caller-chosen path. There are no retries and no
//! replacement clients.

use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use rustx_execution_protocol::is_normalized_absolute_path;
use rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT;

use tokio_util::sync::CancellationToken;

use crate::local_runtime::session::SessionId;
use crate::local_runtime::session_runtime_manager::{
    ExecutionListener, LocalExecutionEndpoint, SessionRuntimeManager,
};

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
) -> Result<tokio::process::Child, String> {
    let session = SessionId::parse(id).map_err(|error| format!("local execution: {error}"))?;
    let endpoint = manager
        .reserve_local_execution(&session)
        .await
        .map_err(|error| format!("local execution: {error}"))?;
    if !is_normalized_absolute_path(workspace) {
        return Err("local execution: workspace must be a normalized absolute path".into());
    }
    if workspace.as_os_str() != endpoint.workspace().as_os_str() {
        return Err("local execution: workspace is not the Session's reserved workspace".into());
    }
    // Trusted resolution only: the executable shipped beside this Server.
    let sibling = std::env::current_exe()
        .map_err(|error| format!("local execution: {error}"))?
        .with_file_name("rustx-execution-client");
    if !sibling.is_file() {
        return Err("local execution: client executable is absent beside the server".into());
    }
    let ExecutionListener::Unix(std_listener) = endpoint.listener() else {
        return Err("local execution: the Session's endpoint is not a Unix listener".into());
    };
    let std_listener = std_listener
        .try_clone()
        .map_err(|error| format!("local execution: {error}"))?;
    std_listener
        .set_nonblocking(true)
        .map_err(|error| format!("local execution: {error}"))?;
    let listener = tokio::net::UnixListener::from_std(std_listener)
        .map_err(|error| format!("local execution: {error}"))?;
    // Empty environment, root working directory, sealed stdio: the child
    // receives only the binding file and the workspace on its command line.
    let mut child = tokio::process::Command::new(&sibling)
        .arg("--binding-file")
        .arg(endpoint.bootstrap_path())
        .arg("--workspace")
        .arg(workspace)
        .current_dir("/")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("local execution: client spawn failed: {error}"))?;
    // One bounded admission: an early Client exit is a failure, and the
    // accept wait is capped by the shared handshake deadline.
    let stream = match tokio::select! {
        status = child.wait() => Err(format!(
            "local execution: client exited before admission ({status:?})"
        )),
        accepted = tokio::time::timeout(LOCAL_HANDSHAKE_TIMEOUT, listener.accept()) => {
            match accepted {
                Ok(Ok((stream, _))) => Ok(stream),
                Ok(Err(error)) => Err(format!("local execution: client accept failed: {error}")),
                Err(_) => Err(
                    "local execution: client did not connect within the handshake deadline".into(),
                ),
            }
        }
    } {
        Ok(stream) => stream,
        Err(error) => {
            reap_failed(&mut child).await;
            return Err(error);
        }
    };
    // The handshake budget is armed the instant the stream exists, before
    // the admission task can wait in the blocking pool's unbounded queue.
    let deadline = std::time::Instant::now() + LOCAL_HANDSHAKE_TIMEOUT;
    let admitted = admit(endpoint, stream, deadline).await;
    match admitted {
        Ok(()) => Ok(child),
        Err(error) => {
            reap_failed(&mut child).await;
            Err(error)
        }
    }
}

/// Runs the blocking authenticated admission off the reactor under the
/// caller-armed absolute deadline. The same deadline bounds the join, so
/// a saturated blocking pool cannot delay admission indefinitely; the
/// in-flight handshake then fails its own armed deadline.
async fn admit(
    endpoint: Arc<LocalExecutionEndpoint>,
    stream: tokio::net::UnixStream,
    deadline: std::time::Instant,
) -> Result<(), String> {
    let outcome = tokio::time::timeout_at(
        tokio::time::Instant::from_std(deadline),
        tokio::task::spawn_blocking(move || {
            endpoint.admit_local_stream_deadline(stream.into_std()?, deadline)
        }),
    )
    .await;
    match outcome {
        Ok(Ok(Ok(Ok(())))) => Ok(()),
        Ok(Ok(Ok(Err(reject)))) => Err(format!("local execution: admission rejected: {reject}")),
        Ok(Ok(Err(error))) => Err(format!("local execution: admission failed: {error}")),
        Ok(Err(error)) => Err(format!("local execution: admission task failed: {error}")),
        Err(_) => Err("local execution: admission exceeded the handshake deadline".into()),
    }
}

/// Kills and reaps a spawned Client unless it already exited. The child
/// carries `kill_on_drop`, so this only bounds an honest peer's wait.
async fn reap_failed(child: &mut tokio::process::Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.start_kill();
    }
    let _ = child.wait().await;
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
/// handshake until the first successful admission or drain — there is no
/// replacement after the live connection ends. In-flight upgrades are
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
    let (admitted, mut admitted_rx) = tokio::sync::watch::channel(());
    loop {
        tokio::select! {
            () = stop.cancelled() => break,
            changed = admitted_rx.changed() => {
                debug_assert!(changed.is_ok(), "sender lives in this task");
                break;
            }
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
                let admitted = admitted.clone();
                let stopping = stop.clone();
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    // Accepted before drain but still queued: a drain that
                    // won the race must not be followed by an admission.
                    if stopping.is_cancelled() {
                        return;
                    }
                    let Ok(stream) = stream.into_std() else { return };
                    let admitted_peer = endpoint
                        .admit_websocket_stream_deadline(stream, deadline)
                        .is_ok_and(|result| result.is_ok());
                    if admitted_peer {
                        let _ = admitted.send(());
                    }
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
