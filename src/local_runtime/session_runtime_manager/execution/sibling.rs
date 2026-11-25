//! Launches one explicitly owned Session Client over its reserved Unix endpoint.

use super::super::super::session::SessionId;
#[cfg(target_os = "linux")]
use super::super::SessionRuntimeManager;
use super::super::{RuntimeManagerError, error};
use super::{ExecutionListener, LocalExecutionEndpoint};
use rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT;
use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use tokio::process::{Child, Command};
#[cfg(target_os = "linux")]
use tokio_util::sync::CancellationToken;

pub(crate) async fn launch_sibling(
    endpoint: Arc<LocalExecutionEndpoint>,
) -> Result<SiblingClient, RuntimeManagerError> {
    #[cfg(test)]
    endpoint.reach_startup_hook("before_local_client_startup");
    let startup = endpoint.begin_client_startup()?;
    let mut unconfirmed = None;
    let result = start_sibling(endpoint.clone(), &mut unconfirmed).await;
    startup.finish(unconfirmed);
    match result {
        Ok(child) => Ok(child),
        Err(failure) => match endpoint.settle().await {
            Ok(()) => Err(failure),
            Err(settlement) => Err(error(format!(
                "{failure}; local execution settlement is unconfirmed: {settlement}"
            ))),
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
fn spawn(
    sibling: &Path,
    endpoint: &LocalExecutionEndpoint,
) -> io::Result<(SiblingClient, rustx_execution_server::LocalDomainGrant)> {
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
    let lease = child.stdin.take().expect("piped domain lease");
    let stdout = child.stdout.take().expect("piped domain reports");
    // The lease stays outside Child so polling wait during authentication
    // cannot close it. It is the only long-lived Server-owned writer.
    Ok((
        SiblingClient {
            child,
            lease: Some(lease),
            reports: tokio::io::BufReader::new(stdout),
            final_report: Vec::new(),
            exit: None,
        },
        grant,
    ))
}

#[cfg(target_os = "linux")]
async fn start_supervisor(
    endpoint: &LocalExecutionEndpoint,
    child: &mut SiblingClient,
    grant: &rustx_execution_server::LocalDomainGrant,
) -> Result<(), RuntimeManagerError> {
    use tokio::io::AsyncWriteExt as _;
    let lease = child.lease.as_mut().expect("local startup lease");
    lease.write_all(grant.token()).await.map_err(error)?;
    if report(&mut child.reports, "ready ", &mut Vec::new())
        .await
        .map_err(error)?
        == 0
    {
        return Err(error("invalid Client namespace init"));
    }
    #[cfg(test)]
    endpoint.reach_startup_hook("after_local_supervisor_ready");
    // The startup owner prevents settlement from removing the domain proof
    // before the supervisor has reaped, even if retirement wins here.
    if !endpoint.dispatch_open() {
        return Err(RuntimeManagerError::LoadingCancelled);
    }
    lease.write_all(b"G").await.map_err(error)
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

async fn start_sibling(
    endpoint: Arc<LocalExecutionEndpoint>,
    unconfirmed: &mut Option<Arc<io::Error>>,
) -> Result<SiblingClient, RuntimeManagerError> {
    // Trusted resolution only: the executable shipped beside this Server.
    let sibling = rustx_process_supervisor::invocation::companion_binary("rustx-execution-client");
    if !sibling.is_file() {
        return Err(error(
            "local execution: client executable is absent beside the server",
        ));
    }
    let ExecutionListener::Unix(std_listener) = endpoint.listener() else {
        return Err(error(
            "local execution: the Session's endpoint is not a Unix listener",
        ));
    };
    let std_listener = std_listener
        .try_clone()
        .map_err(|failure| error(format!("local execution: {failure}")))?;
    std_listener
        .set_nonblocking(true)
        .map_err(|failure| error(format!("local execution: {failure}")))?;
    let listener = tokio::net::UnixListener::from_std(std_listener)
        .map_err(|failure| error(format!("local execution: {failure}")))?;
    // Namespace setup and both authenticated lanes share a finite startup cut.
    let deadline = std::time::Instant::now() + LOCAL_HANDSHAKE_TIMEOUT;
    #[cfg(target_os = "linux")]
    let (mut child, grant) = spawn(&sibling, &endpoint)
        .map_err(|failure| error(format!("local execution: client spawn failed: {failure}")))?;
    #[cfg(not(target_os = "linux"))]
    let mut child = spawn(&sibling, &endpoint)
        .map_err(|failure| error(format!("local execution: client spawn failed: {failure}")))?;
    #[cfg(target_os = "linux")]
    {
        let setup = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            start_supervisor(&endpoint, &mut child, &grant),
        )
        .await
        .unwrap_or_else(|_| {
            Err(error(
                "local execution: Client supervisor did not report setup",
            ))
        });
        if let Err(failure) = setup {
            return Err(reap_failed(&mut child, failure, unconfirmed).await);
        }
    }
    if let Err(failure) = authenticate_sibling(&endpoint, &listener, &mut child, deadline).await {
        return Err(reap_failed(&mut child, failure, unconfirmed).await);
    }
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
    Ok(child)
}

async fn authenticate_sibling(
    endpoint: &Arc<LocalExecutionEndpoint>,
    listener: &tokio::net::UnixListener,
    child: &mut SiblingClient,
    deadline: std::time::Instant,
) -> Result<(), RuntimeManagerError> {
    loop {
        if !endpoint.dispatch_open() {
            return Err(RuntimeManagerError::LoadingCancelled);
        }
        let accept =
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), listener.accept());
        let stream = tokio::select! {
            status = child.wait() => Err(format!(
                "local execution: client exited before admission ({status:?})"
            )),
            accepted = accept => match accepted {
                Ok(Ok((stream, _))) => Ok(stream),
                Ok(Err(error)) => Err(format!("local execution: client accept failed: {error}")),
                Err(_) => Err("local execution: client did not connect within the handshake deadline".into()),
            }
        }.map_err(|failure| handshake_failure(endpoint, failure))?;
        match admit(endpoint.clone(), stream, deadline).await {
            Ok(crate::local_runtime::session_runtime_manager::EndpointAdmission::Live) => {
                return Ok(());
            }
            // The data half is held while control pairs inside the same budget.
            Ok(crate::local_runtime::session_runtime_manager::EndpointAdmission::Pending) => {}
            Ok(crate::local_runtime::session_runtime_manager::EndpointAdmission::Rejected(
                reject,
            )) => {
                return Err(handshake_failure(
                    endpoint,
                    format!("local execution: admission rejected: {reject}"),
                ));
            }
            Err(failure) => return Err(handshake_failure(endpoint, failure)),
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

/// Classify the handshake before its cleanup changes endpoint state.
fn handshake_failure(endpoint: &LocalExecutionEndpoint, failure: String) -> RuntimeManagerError {
    if endpoint.dispatch_open() {
        error(failure)
    } else {
        RuntimeManagerError::LoadingCancelled
    }
}

/// Release the exact owned Client after a failed admission. On Linux the
/// independent helper retains reap ownership even if this deadline expires.
async fn reap_failed(
    child: &mut SiblingClient,
    cause: RuntimeManagerError,
    unconfirmed: &mut Option<Arc<io::Error>>,
) -> RuntimeManagerError {
    let failure = match tokio::time::timeout(LOCAL_HANDSHAKE_TIMEOUT, child.terminate()).await {
        #[cfg(target_os = "linux")]
        Ok(Ok(status)) if child.exit.is_none() => {
            format!("local Client supervisor exited without settlement ({status})")
        }
        Ok(Ok(_)) => return cause,
        Ok(Err(failure)) => format!("local Client reap failed: {failure}"),
        Err(_) => "local Client reap timed out".into(),
    };
    *unconfirmed = Some(Arc::new(io::Error::other(failure.clone())));
    error(format!("{cause}; {failure}"))
}

pub(crate) struct LocalClient {
    pub(crate) child: SiblingClient,
    #[cfg(target_os = "linux")]
    session: SessionId,
    pub(super) endpoint: Arc<LocalExecutionEndpoint>,
    #[cfg(target_os = "linux")]
    admitted_at: tokio::time::Instant,
}

impl LocalClient {
    pub(crate) async fn launch(
        session: SessionId,
        endpoint: Arc<LocalExecutionEndpoint>,
    ) -> Result<Self, RuntimeManagerError> {
        let child = launch_sibling(endpoint.clone()).await?;
        #[cfg(not(target_os = "linux"))]
        let _ = session;
        Ok(Self {
            child,
            #[cfg(target_os = "linux")]
            session,
            endpoint,
            #[cfg(target_os = "linux")]
            admitted_at: tokio::time::Instant::now(),
        })
    }
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct RestartBudget {
    attempted: Vec<tokio::time::Instant>,
    consecutive: usize,
}

#[cfg(target_os = "linux")]
impl RestartBudget {
    fn next_delay(
        &mut self,
        now: tokio::time::Instant,
        admitted_at: tokio::time::Instant,
    ) -> Option<std::time::Duration> {
        use std::time::Duration;
        self.attempted
            .retain(|start| now.saturating_duration_since(*start) < Duration::from_secs(60));
        if now.duration_since(admitted_at) >= Duration::from_mins(5) {
            self.consecutive = 0;
        }
        if self.attempted.len() == 3 {
            return None;
        }
        let delay = Duration::from_secs([1, 2, 4][self.consecutive.min(2)]);
        self.consecutive = (self.consecutive + 1).min(2);
        Some(delay)
    }
}

/// The process owner watches for actual termination independently of lane
/// detachment. On drain it hands any remaining owned child to the Host.
#[cfg(target_os = "linux")]
pub(crate) async fn maintain(
    manager: SessionRuntimeManager,
    mut client: LocalClient,
    stop: CancellationToken,
) -> Option<LocalClient> {
    let mut budget = RestartBudget::default();
    loop {
        let result = tokio::select! {
            biased;
            () = stop.cancelled() => return Some(client),
            result = client.child.wait() => result,
        };
        if let Err(error) =
            SessionRuntimeManager::retire_dead_local_execution(client.endpoint.clone()).await
        {
            eprintln!(
                "rustx app-server: local Client {} remains unavailable after exit ({result:?}): {error}",
                client.session
            );
            return None;
        }
        // Successful exit includes intentional Session closure. It does not
        // authorize the process owner to synthesize a new foreground request.
        if result.is_ok_and(|status| status.success()) {
            return None;
        }
        let Some(delay) = budget.next_delay(tokio::time::Instant::now(), client.admitted_at) else {
            eprintln!(
                "rustx app-server: local Client {} exhausted the restart budget",
                client.session
            );
            return None;
        };
        tokio::select! {
            biased;
            () = stop.cancelled() => return None,
            () = tokio::time::sleep(delay) => {}
        }
        budget.attempted.push(tokio::time::Instant::now());
        let next = manager
            .replace_dead_local_execution(&client.session, client.endpoint)
            .await;
        let replacement = match next {
            Ok(endpoint) => LocalClient::launch(client.session, endpoint).await,
            Err(error) => Err(error),
        };
        // Failed authentication, configuration or version never loops.
        match replacement {
            Ok(next) => client = next,
            Err(error) => {
                eprintln!("rustx app-server: local Client replacement failed: {error}");
                return None;
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::RestartBudget;
    use std::time::Duration;
    use tokio::time::Instant;

    #[test]
    fn local_restart_budget_honors_at_most_three_attempts_and_stable_reset() {
        let base = Instant::now();
        let mut budget = RestartBudget::default();
        let mut now = base;
        for delay in [1, 2, 4] {
            assert_eq!(
                budget.next_delay(now, now),
                Some(Duration::from_secs(delay))
            );
            now += Duration::from_secs(delay);
            budget.attempted.push(now);
        }
        assert_eq!(budget.next_delay(base + Duration::from_secs(60), now), None);
        assert_eq!(
            budget.next_delay(base + Duration::from_secs(61), now),
            Some(Duration::from_secs(4))
        );
        assert_eq!(
            budget.next_delay(now + Duration::from_mins(5), now),
            Some(Duration::from_secs(1))
        );
    }
}
