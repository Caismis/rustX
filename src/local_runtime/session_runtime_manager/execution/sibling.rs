//! Launches one explicitly owned Session Client over its reserved Unix endpoint.

use super::{ExecutionListener, LocalExecutionEndpoint};
use rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT;
use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use tokio::process::{Child, Command};

pub(crate) async fn launch_sibling(
    endpoint: Arc<LocalExecutionEndpoint>,
) -> Result<SiblingClient, String> {
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

pub(crate) struct SiblingClient {
    child: Child,
    #[cfg(target_os = "linux")]
    lease: Option<tokio::process::ChildStdin>,
    #[cfg(target_os = "linux")]
    reports: tokio::io::BufReader<tokio::process::ChildStdout>,
    #[cfg(target_os = "linux")]
    final_report: Vec<u8>,
    #[cfg(target_os = "linux")]
    exit: Option<ExitStatus>,
}

impl SiblingClient {
    pub(crate) async fn wait(&mut self) -> io::Result<ExitStatus> {
        #[cfg(target_os = "linux")]
        if let Some(status) = self.exit {
            return Ok(status);
        }
        let status = self.child.wait().await?;
        #[cfg(target_os = "linux")]
        if status.success() {
            use std::os::unix::process::ExitStatusExt as _;
            let code = report(&mut self.reports, "settled ", &mut self.final_report).await?;
            let code = i32::try_from(code)
                .ok()
                .filter(|code| *code <= 255)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid Client exit status")
                })?;
            let status = ExitStatus::from_raw(code << 8);
            self.exit = Some(status);
            return Ok(status);
        }
        Ok(status)
    }

    pub(crate) async fn terminate(&mut self) -> io::Result<ExitStatus> {
        #[cfg(target_os = "linux")]
        drop(self.lease.take());
        #[cfg(not(target_os = "linux"))]
        if self.child.try_wait()?.is_none() {
            self.child.start_kill()?;
        }
        self.wait().await
    }
}

#[cfg(target_os = "linux")]
async fn report(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
    prefix: &str,
    bytes: &mut Vec<u8>,
) -> io::Result<u32> {
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _};
    let remaining = 64u64.saturating_sub(bytes.len() as u64);
    reader.take(remaining).read_until(b'\n', bytes).await?;
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|line| line.strip_prefix(prefix))
        .and_then(|value| value.strip_suffix('\n'))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Client supervisor report",
            )
        })
}

fn client_arguments(command: &mut Command, endpoint: &LocalExecutionEndpoint) {
    command
        .arg("--binding-file")
        .arg(endpoint.bootstrap_path())
        .arg("--workspace")
        .arg(endpoint.workspace())
        .current_dir("/")
        .env_clear();
    // Local deployment contributes its trusted runtime discovery paths.
    // The Session and model cannot add process environment entries here.
    for key in ["HOME", "PATH", "XDG_DATA_HOME"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
}

#[cfg(target_os = "linux")]
async fn spawn(sibling: &Path, endpoint: &LocalExecutionEndpoint) -> io::Result<SiblingClient> {
    use tokio::io::AsyncWriteExt as _;
    let guardian = rustx_process_supervisor::invocation::companion_binary("interactive-supervisor");
    let grant = endpoint.arm_local_domain()?;
    let mut command = Command::new(guardian);
    command
        .arg("client-domain")
        .arg(grant.receipt())
        .arg(sibling);
    client_arguments(&mut command, endpoint);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut lease = child.stdin.take().expect("piped domain lease");
    let mut reports = tokio::io::BufReader::new(child.stdout.take().expect("piped domain reports"));
    lease.write_all(grant.token()).await?;
    if report(&mut reports, "ready ", &mut Vec::new()).await? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid Client namespace init",
        ));
    }
    lease.write_all(b"G").await?;
    // The lease stays outside Child so polling wait during authentication
    // cannot close it. It is the only long-lived Server-owned writer.
    Ok(SiblingClient {
        child,
        lease: Some(lease),
        reports,
        final_report: Vec::new(),
        exit: None,
    })
}

#[cfg(not(target_os = "linux"))]
fn spawn(sibling: &Path, endpoint: &LocalExecutionEndpoint) -> io::Result<SiblingClient> {
    let mut command = Command::new(sibling);
    client_arguments(&mut command, endpoint);
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    Ok(SiblingClient { child })
}

async fn start_sibling(endpoint: Arc<LocalExecutionEndpoint>) -> Result<SiblingClient, String> {
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
    // Namespace setup and both authenticated lanes share a finite startup cut.
    let deadline = std::time::Instant::now() + LOCAL_HANDSHAKE_TIMEOUT;
    #[cfg(target_os = "linux")]
    let child = tokio::time::timeout_at(
        tokio::time::Instant::from_std(deadline),
        spawn(&sibling, &endpoint),
    )
    .await
    .map_err(|_| "local execution: Client supervisor did not report setup".to_owned())?;
    #[cfg(not(target_os = "linux"))]
    let child = spawn(&sibling, &endpoint);
    let mut child =
        child.map_err(|error| format!("local execution: client spawn failed: {error}"))?;
    loop {
        let accept =
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), listener.accept());
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
        match admit(endpoint.clone(), stream, deadline).await {
            Ok(crate::local_runtime::session_runtime_manager::EndpointAdmission::Live) => {
                let stop = endpoint.accept_stop.clone();
                let binding = Arc::downgrade(&endpoint);
                tokio::spawn(async move {
                    loop {
                        let accepted = tokio::select! {
                            biased;
                            () = stop.cancelled() => break,
                            accepted = listener.accept() => accepted,
                        };
                        let Ok((stream, _)) = accepted else { break };
                        let Some(endpoint) = binding.upgrade() else {
                            break;
                        };
                        let deadline = std::time::Instant::now() + LOCAL_HANDSHAKE_TIMEOUT;
                        let _ = admit(endpoint, stream, deadline).await;
                    }
                });
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

/// Release the exact owned Client after a failed admission. On Linux the
/// independent helper retains reap ownership even if this deadline expires.
async fn reap_failed(child: &mut SiblingClient) {
    let _ = tokio::time::timeout(LOCAL_HANDSHAKE_TIMEOUT, child.terminate()).await;
}
