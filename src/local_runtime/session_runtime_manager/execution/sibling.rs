//! Launches one explicitly owned Session Client over its reserved Unix endpoint.

use super::{ExecutionListener, LocalExecutionEndpoint};
use rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT;
use std::process::Stdio;
use std::sync::Arc;

pub(crate) async fn launch_sibling(
    endpoint: Arc<LocalExecutionEndpoint>,
) -> Result<tokio::process::Child, String> {
    match start_sibling(endpoint.clone()).await {
        Ok(child) => Ok(child),
        Err(error) => match endpoint.settle().await {
            Ok(()) => Err(error),
            Err(settlement) => Err(format!(
                "{error}; local execution settlement is unconfirmed: {settlement}"
            )),
        },
    }
}

async fn start_sibling(
    endpoint: Arc<LocalExecutionEndpoint>,
) -> Result<tokio::process::Child, String> {
    // Trusted resolution only: the executable shipped beside this Server.
    let sibling = rustx_process_supervisor::invocation::companion_binary("rustx-execution-client");
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
        .arg(endpoint.workspace())
        .current_dir("/")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("local execution: client spawn failed: {error}"))?;
    // One bounded paired admission: the data lane's `Hello` earns a
    // pairing challenge and `Pending`; the Client's control lane — a
    // second connection carrying `ControlHello` with the challenge
    // nonce — commits the pair `Live`. Both lanes share one absolute
    // budget armed at the first accept, and an early Client exit fails
    // the launch at any point.
    let mut deadline = None;
    loop {
        let accept = match deadline {
            // The budget is armed the instant the first stream exists —
            // the same `Instant` forwarded into the binding — never one
            // deadline per lane.
            Some(deadline) => {
                tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), listener.accept())
            }
            None => tokio::time::timeout(LOCAL_HANDSHAKE_TIMEOUT, listener.accept()),
        };
        let stream = match tokio::select! {
            status = child.wait() => Err(format!(
                "local execution: client exited before admission ({status:?})"
            )),
            accepted = accept => {
                match accepted {
                    Ok(Ok((stream, _))) => Ok(stream),
                    Ok(Err(error)) => {
                        Err(format!("local execution: client accept failed: {error}"))
                    }
                    Err(_) => Err(
                        "local execution: client did not connect within the handshake deadline"
                            .into(),
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
        let deadline =
            *deadline.get_or_insert_with(|| std::time::Instant::now() + LOCAL_HANDSHAKE_TIMEOUT);
        match admit(endpoint.clone(), stream, deadline).await {
            Ok(crate::local_runtime::session_runtime_manager::EndpointAdmission::Live) => {
                return Ok(child);
            }
            // Data half held — the pairing control lane is still to come
            // inside the same budget.
            Ok(crate::local_runtime::session_runtime_manager::EndpointAdmission::Pending) => {}
            Ok(crate::local_runtime::session_runtime_manager::EndpointAdmission::Rejected(
                reject,
            )) => {
                reap_failed(&mut child).await;
                return Err(format!("local execution: admission rejected: {reject}"));
            }
            Err(error) => {
                reap_failed(&mut child).await;
                return Err(error);
            }
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
) -> Result<crate::local_runtime::session_runtime_manager::EndpointAdmission, String> {
    let outcome = tokio::time::timeout_at(
        tokio::time::Instant::from_std(deadline),
        tokio::task::spawn_blocking(move || {
            endpoint.admit_local_stream_deadline(stream.into_std()?, deadline)
        }),
    )
    .await;
    match outcome {
        Ok(Ok(Ok(admission))) => Ok(admission),
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
