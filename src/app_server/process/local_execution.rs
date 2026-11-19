//! Explicit one-Session Execution Client composition — opt-in, never discovered.
//!
//! Only the paired `--local-execution-session`/`--local-execution-workspace`
//! flags reach this module: bare `app-server` launches nothing. The Session
//! is resolved through the trusted catalog claim, the workspace must equal
//! the endpoint's reserved logical workspace byte-for-byte, and the Client
//! executable is only ever the trusted sibling of the running Server —
//! never PATH, HOME, or a caller-chosen path. There are no retries and no
//! replacement clients.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use rustx_execution_protocol::is_normalized_absolute_path;
use rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT;

use crate::local_runtime::session::SessionId;
use crate::local_runtime::session_runtime_manager::{
    LocalExecutionEndpoint, SessionRuntimeManager,
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
    let std_listener = endpoint
        .listener()
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
