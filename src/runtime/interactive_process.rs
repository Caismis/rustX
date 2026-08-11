//! The rustX-side driver of one long-lived interactive process (MCP stdio
//! servers).
//!
//! The driver is the physical settlement owner of the interactive
//! supervisor unit:
//!
//! - the runtime child-subreaper prerequisite is established before the
//!   supervisor unit spawns;
//! - once the supervisor spawn succeeds, the runtime-owned driver task
//!   immediately owns physical settlement; a later handshake/control setup
//!   error (accept failure, connection loss) settles the unit instead of
//!   stranding a raw child;
//! - the direct supervisor child is reaped before physical settlement is
//!   published;
//! - the unit's terminal event is the outer supervisor's authoritative
//!   `AllChildrenReaped` report; control-channel loss before it escalates
//!   to the shared adopted-anchor emergency containment;
//! - stderr is drained until EOF; only a bounded preview is retained, and
//!   reading never stops merely because the preview limit was reached;
//! - dropping the business-facing handle requests shutdown but never
//!   abandons the physical process owner.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::AsyncReadExt;

use crate::runtime::interactive_supervisor::RUSTX_CONTROL_ENV;
use crate::runtime::process_runner::{
    MAX_PROCESS_OUTPUT_BYTES, SupervisorEvent, read_supervisor_event, send_start,
    send_terminal_ack, send_terminate,
};
use crate::runtime::supervised_unit::{EmergencyContainment, emergency_contain_group};

/// The explicit command description of one long-lived interactive server.
/// Unlike the supervised-command spec, the business stdin/stdout pair
/// belongs to the child protocol; supervisor control uses a private Unix
/// socket.
#[cfg(unix)]
#[derive(Debug, Clone)]
pub(crate) struct InteractiveProcessSpec {
    /// The executable to run inside the owned process group.
    pub program: PathBuf,
    /// Explicit program arguments.
    pub args: Vec<String>,
    /// Explicit working directory.
    pub cwd: PathBuf,
    /// Explicit environment, never inherited.
    pub environment: Vec<(String, String)>,
}

/// A rustX-owned interactive process handle.
///
/// The returned protocol streams are business-facing handles. The detached
/// driver owns the supervisor child and the control connection, so dropping
/// the streams cannot abandon the process hierarchy. `Drop` requests orderly
/// shutdown; the driver waits for the supervisor's terminal event and then
/// reaps its direct child.
#[cfg(unix)]
pub(crate) struct SupervisedInteractiveProcess {
    pub stdin: Option<tokio::process::ChildStdin>,
    pub stdout: Option<tokio::process::ChildStdout>,
    /// The bounded stderr preview; the drain task reads until EOF regardless
    /// of the preview bound.
    pub(crate) stderr_preview: Arc<Mutex<Vec<u8>>>,
    settled: Arc<AtomicBool>,
    settled_notify: Arc<tokio::sync::Notify>,
    shutdown: Option<tokio::sync::mpsc::Sender<()>>,
    /// Test-only: the pid of the direct supervisor child (observability for
    /// the direct-reap regression).
    #[cfg(test)]
    supervisor_child_pid: Option<u32>,
}

#[cfg(unix)]
impl SupervisedInteractiveProcess {
    /// Starts one interactive process under the dedicated long-lived owner.
    pub(crate) fn spawn(spec: InteractiveProcessSpec) -> Result<Self, String> {
        use std::sync::atomic::AtomicU64;
        static NEXT_SOCKET: AtomicU64 = AtomicU64::new(1);

        // The runtime child-subreaper capability is a pre-ownership
        // prerequisite: the catastrophic fallback authority must exist
        // before the supervisor unit spawns (mirrors the short-lived
        // supervised-command runner).
        crate::runtime::process_supervision::ensure_child_subreaper()?;

        let InteractiveProcessSpec {
            program,
            args,
            cwd,
            environment,
        } = spec;
        let socket_path = std::env::temp_dir().join(format!(
            "rustx-interactive-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = tokio::net::UnixListener::bind(&socket_path)
            .map_err(|error| format!("cannot bind interactive control socket: {error}"))?;
        let mut supervisor = tokio::process::Command::new(
            crate::runtime::process_runner::interactive_supervisor_binary(),
        );
        supervisor
            .arg("outer")
            .arg(&program)
            .args(&args)
            .current_dir(&cwd)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &environment {
            supervisor.env(key, value);
        }
        supervisor.env(RUSTX_CONTROL_ENV, &socket_path);
        let mut child = supervisor
            .spawn()
            .map_err(|error| format!("cannot spawn interactive supervisor: {error}"))?;
        drop(supervisor);
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stderr_capture = Arc::new(Mutex::new(Vec::new()));
        if let Some(mut stderr_pipe) = stderr {
            let capture = stderr_capture.clone();
            // The stderr drain reads until EOF — the preview bound only
            // limits what is retained, never how long the pipe is drained,
            // so a server that keeps writing stderr can never die on a full
            // pipe while it continues operating. The bounded preview is
            // published incrementally so it is observable before EOF.
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 8192];
                loop {
                    match stderr_pipe.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(count) => {
                            let remaining = MAX_PROCESS_OUTPUT_BYTES.saturating_sub(bytes.len());
                            bytes.extend_from_slice(&buffer[..count.min(remaining)]);
                            capture
                                .lock()
                                .expect("interactive stderr lock")
                                .clone_from(&bytes);
                        }
                    }
                }
                *capture.lock().expect("interactive stderr lock") = bytes;
            });
        }
        let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel(1);
        let settled = Arc::new(AtomicBool::new(false));
        let settled_notify = Arc::new(tokio::sync::Notify::new());
        let settled_for_driver = settled.clone();
        let settled_notify_for_driver = settled_notify.clone();
        #[cfg(test)]
        let supervisor_child_pid = child.id().expect("the spawned supervisor child has a pid");
        // The runtime-owned driver is the physical settlement owner from
        // the moment the spawn succeeded. Dropping the JoinHandle detaches
        // it; the driver completes the terminal exchange and reaps the
        // direct supervisor child regardless of the business handles.
        std::mem::drop(tokio::spawn(drive_interactive_unit(
            child,
            listener,
            socket_path,
            shutdown_rx,
            settled_for_driver,
            settled_notify_for_driver,
        )));
        Ok(Self {
            stdin,
            stdout,
            stderr_preview: stderr_capture,
            settled,
            settled_notify,
            shutdown: Some(shutdown_tx),
            #[cfg(test)]
            supervisor_child_pid: Some(supervisor_child_pid),
        })
    }

    /// The bounded, lossily-decoded stderr preview observed so far.
    ///
    /// The drain task reads the pipe until EOF regardless of this bound, so
    /// reading this never affects the server's ability to keep writing.
    pub(crate) fn stderr_preview(&self) -> String {
        let bytes = self.stderr_preview.lock().expect("interactive stderr lock");
        String::from_utf8_lossy(&bytes).trim().to_owned()
    }

    /// Requests orderly server retirement without using the business stdin.
    pub(crate) fn request_shutdown(&self) {
        if let Some(shutdown) = &self.shutdown {
            let _ = shutdown.try_send(());
        }
    }

    /// Waits until the detached supervisor driver has observed its terminal
    /// event and reaped the direct supervisor child.
    pub(crate) async fn wait_for_settlement(&self) {
        if self.settled.load(Ordering::Acquire) {
            return;
        }
        loop {
            let notified = self.settled_notify.notified();
            if self.settled.load(Ordering::Acquire) {
                return;
            }
            notified.await;
            if self.settled.load(Ordering::Acquire) {
                return;
            }
        }
    }
}

#[cfg(unix)]
impl Drop for SupervisedInteractiveProcess {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.try_send(());
        }
    }
}

/// The detached driver of one interactive supervisor unit: the single
/// physical settlement owner of the supervisor child.
#[cfg(unix)]
#[allow(clippy::too_many_lines)] // one coherent accept/relay/contain/reap pipeline
async fn drive_interactive_unit(
    mut child: tokio::process::Child,
    listener: tokio::net::UnixListener,
    socket_path: PathBuf,
    mut shutdown_rx: tokio::sync::mpsc::Receiver<()>,
    settled: Arc<AtomicBool>,
    settled_notify: Arc<tokio::sync::Notify>,
) {
    // Handshake: accept the outer's control connection. If the outer dies
    // before connecting, the select reaps it — a post-spawn setup error can
    // never strand a raw child.
    let accepted = tokio::select! {
        accepted = listener.accept() => accepted.map(|(control, _)| control),
        status = child.wait() => {
            let _ = status;
            Err(std::io::Error::other("the supervisor exited before connecting"))
        }
    };
    let Ok(control) = accepted else {
        // The outer died before connecting (or accept failed): the child
        // was reaped by the select; settlement is published without
        // stranding a raw child.
        let _ = std::fs::remove_file(&socket_path);
        settled.store(true, Ordering::Release);
        settled_notify.notify_waiters();
        return;
    };
    let _ = std::fs::remove_file(&socket_path);
    let (mut control_read, mut control_write) = tokio::io::split(control);
    let mut anchor: Option<i32> = None;
    let mut started = false;
    let mut terminal_seen = false;
    loop {
        tokio::select! {
            biased;
            command = shutdown_rx.recv(), if !terminal_seen => {
                if command.is_some() {
                    let () = send_terminate(&mut control_write).await;
                }
                // A dropped sender is not a shutdown request: the business
                // handle requests shutdown explicitly.
            }
            event = read_supervisor_event(&mut control_read) => match event {
                Ok(Some(SupervisorEvent::AnchorReady { pgid })) => {
                    if anchor.is_none() && pgid > 0 {
                        anchor = Some(pgid);
                    }
                    if !started {
                        started = true;
                        if let Err(error) = send_start(&mut control_write).await {
                            let _ = error;
                        }
                    }
                }
                Ok(Some(
                    SupervisorEvent::OwnershipEstablished
                    | SupervisorEvent::NoOwnership
                    | SupervisorEvent::SignalAttempt { .. }
                    | SupervisorEvent::ShellExited { .. },
                )) => {}
                Ok(Some(SupervisorEvent::ProcessControlFailure { message })) => {
                    let _ = message;
                }
                Ok(Some(SupervisorEvent::AllChildrenReaped)) => {
                    // The authoritative terminal event of the unit. The
                    // direct supervisor child is still reaped below before
                    // settlement is published.
                    terminal_seen = true;
                    let () = send_terminal_ack(&mut control_write).await;
                    break;
                }
                Ok(None) => {
                    // Control EOF before the terminal event: the supervisor
                    // unit is lost. The group may still be live; the shared
                    // adopted-anchor emergency containment settles it.
                    break;
                }
                Err(_error) => {
                    break;
                }
            },
        }
    }
    if terminal_seen {
        // The direct supervisor child is reaped before physical settlement
        // is published.
        let _ = child.wait().await;
    } else {
        // Catastrophic supervisor loss: reap the lost outer, then contain
        // the owned group through the retained inner anchor. Without an
        // anchor there was never an owned group (a pre-ownership loss).
        let _ = child.wait().await;
        if let Some(pgid) = anchor {
            match tokio::task::spawn_blocking(move || emergency_contain_group(pgid, false)).await {
                Ok(Ok(
                    EmergencyContainment::TerminalProven | EmergencyContainment::AnchorUnavailable,
                )) => {}
                // Anchor loss is never itself a terminal proof; the unit
                // already failed, and settlement is published with the
                // failure intent recorded by the caller.
                Ok(Err(_error)) => {}
                Err(_error) => {}
            }
        }
    }
    settled.store(true, Ordering::Release);
    settled_notify.notify_waiters();
}

#[cfg(all(test, unix))]
mod interactive_tests {
    //! Deterministic regressions of the interactive supervisor unit's
    //! M5-equivalent physical ownership. Every test runs the real supervisor
    //! binary through [`SupervisedInteractiveProcess::spawn`] and uses
    //! marker/pid files with strict deadlock guards — never timing-based
    //! correctness assertions.

    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use super::{InteractiveProcessSpec, SupervisedInteractiveProcess};
    use crate::runtime::interactive_supervisor::{
        ANCHOR_PID_FILE_ENV, FAIL_SIGNAL_ENV, OUTER_FAIL_ENV,
    };
    use crate::runtime::process_runner::MAX_PROCESS_OUTPUT_BYTES;

    const DEADLINE: Duration = Duration::from_secs(20);

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("fixture dir");
            Self { dir }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn spawn(
            &self,
            script: &str,
            extra_env: Vec<(String, String)>,
        ) -> Result<SupervisedInteractiveProcess, String> {
            let mut environment =
                vec![("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned())];
            environment.extend(extra_env);
            let spec = InteractiveProcessSpec {
                program: PathBuf::from("/bin/sh"),
                args: vec!["-c".to_owned(), script.to_owned()],
                cwd: self.dir.path().to_path_buf(),
                environment,
            };
            SupervisedInteractiveProcess::spawn(spec)
        }
    }

    fn wait_for_file(path: &Path, description: &str) {
        let deadline = Instant::now() + DEADLINE;
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "{description} never appeared: {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn read_pid(path: &Path) -> i32 {
        wait_for_file(path, "pid file");
        std::fs::read_to_string(path)
            .expect("pid file")
            .trim()
            .parse()
            .expect("pid")
    }

    fn proc_state(pid: i32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let close = stat.rfind(')')?;
        stat[close + 2..].chars().next()
    }

    fn wait_for_reaped(pid: i32, description: &str) {
        let deadline = Instant::now() + DEADLINE;
        while proc_state(pid).is_some() {
            assert!(
                Instant::now() < deadline,
                "{description} (pid {pid}) was never reaped"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    async fn settle(process: &mut SupervisedInteractiveProcess) {
        process.request_shutdown();
        tokio::time::timeout(DEADLINE, process.wait_for_settlement())
            .await
            .expect("the unit must settle");
    }

    fn python_available() -> bool {
        ["/usr/local/bin/python3", "/usr/bin/python3", "/bin/python3"]
            .iter()
            .any(|path| Path::new(path).is_file())
    }

    /// Normal server shutdown: `request_shutdown` runs the TERM sequence and
    /// the whole unit settles.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn normal_server_shutdown_settles_the_unit() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let script = format!("echo started > {}; sleep 30", marker.display());
        let mut process = fixture.spawn(&script, Vec::new()).expect("spawn");
        wait_for_file(&marker, "server start marker");
        settle(&mut process).await;
    }

    /// A server child that outlives its server parent stays owned: the unit
    /// does not settle while the in-group descendant lives, and shutdown
    /// terminates it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn server_child_outliving_server_parent_is_contained() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let child_pid_file = fixture.path("child.pid");
        let script = format!(
            "sleep 30 & echo $! > {}; echo started > {}; sleep 5",
            child_pid_file.display(),
            marker.display()
        );
        let mut process = fixture.spawn(&script, Vec::new()).expect("spawn");
        wait_for_file(&marker, "server start marker");
        let child_pid = read_pid(&child_pid_file);
        settle(&mut process).await;
        wait_for_reaped(child_pid, "outliving server child");
    }

    /// Runs one in-server escape attempt and returns the three deterministic
    /// observations: the server really reached the syscall, the syscall was
    /// rejected with `EPERM`, and the post-escape marker was never written.
    ///
    /// The `reached` marker is what makes this non-vacuous: an absent escape
    /// marker only proves containment if the attempt actually executed.
    async fn assert_escape_is_denied(call: &str) {
        let fixture = Fixture::new();
        let reached = fixture.path("reached");
        let denied = fixture.path("denied");
        let escaped = fixture.path("escaped");
        // The server records that it reached the syscall, then classifies the
        // outcome: PermissionError (EPERM from the inherited fixed-membership
        // seccomp filter) versus a successful escape.
        let program = format!(
            "import os\n\
             open({reached:?}, 'w').close()\n\
             try:\n\
             \x20   os.{call}\n\
             except PermissionError:\n\
             \x20   open({denied:?}, 'w').close()\n\
             else:\n\
             \x20   open({escaped:?}, 'w').close()\n",
            reached = reached.display().to_string(),
            denied = denied.display().to_string(),
            escaped = escaped.display().to_string(),
        );
        let script = format!("python3 -c {}", shell_single_quote(&program));
        let mut process = fixture.spawn(&script, Vec::new()).expect("spawn");
        wait_for_file(&reached, "escape attempt reached marker");
        settle(&mut process).await;
        assert!(
            denied.exists(),
            "{call} must be rejected with EPERM by the inherited membership filter"
        );
        assert!(
            !escaped.exists(),
            "{call} must never succeed: nothing may leave the owned group"
        );
    }

    /// Quotes one argument for `/bin/sh -c` as a single-quoted word.
    fn shell_single_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    /// A `setsid` escape attempt fails deterministically with EPERM (the
    /// shared fixed-membership restriction): the attempt provably runs, the
    /// syscall is denied, and nothing leaves the owned group.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn setsid_escape_attempt_fails_deterministically() {
        if !python_available() {
            eprintln!("python3 unavailable; setsid escape regression not exercised");
            return;
        }
        assert_escape_is_denied("setsid()").await;
    }

    /// A `setpgid` escape attempt fails deterministically with EPERM.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn setpgid_escape_attempt_fails_deterministically() {
        if !python_available() {
            eprintln!("python3 unavailable; setpgid escape regression not exercised");
            return;
        }
        assert_escape_is_denied("setpgid(0, 0)").await;
    }

    /// A TERM-resistant server is killed by the grace-period KILL, and the
    /// unit settles only after the owned group is terminal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn term_resistant_server_is_killed_after_the_grace_period() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let server_pid_file = fixture.path("server.pid");
        let script = format!(
            "trap '' TERM; echo $$ > {}; echo started > {}; sleep 30",
            server_pid_file.display(),
            marker.display()
        );
        let mut process = fixture.spawn(&script, Vec::new()).expect("spawn");
        wait_for_file(&marker, "server start marker");
        let server_pid = read_pid(&server_pid_file);
        settle(&mut process).await;
        wait_for_reaped(server_pid, "TERM-resistant server after KILL");
    }

    /// Inner/supervisor control failure: the inner is killed while the
    /// server lives. The outer observes the abnormal anchor exit, issues
    /// the fallback containment while the anchor is retained, and the unit
    /// settles with the owned group terminal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inner_supervisor_loss_is_contained_by_the_outer() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let anchor_pid_file = fixture.path("anchor.pid");
        let server_pid_file = fixture.path("server.pid");
        let script = format!(
            "echo $$ > {}; echo started > {}; sleep 30",
            server_pid_file.display(),
            marker.display()
        );
        let process = fixture
            .spawn(
                &script,
                vec![(
                    ANCHOR_PID_FILE_ENV.to_owned(),
                    anchor_pid_file.display().to_string(),
                )],
            )
            .expect("spawn");
        wait_for_file(&marker, "server start marker");
        let server_pid = read_pid(&server_pid_file);
        let inner_pid = read_pid(&anchor_pid_file);
        // Kill the inner supervisor: the outer's dedicated anchor
        // observation sees the abnormal exit and performs the fallback
        // containment of the owned group.
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(inner_pid),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("kill the inner supervisor");
        tokio::time::timeout(DEADLINE, process.wait_for_settlement())
            .await
            .expect("the outer must contain and settle the unit");
        wait_for_reaped(server_pid, "server after inner-supervisor loss");
    }

    /// Dropping the business-facing handle requests shutdown and never
    /// abandons the physical process owner: the unit settles and the server
    /// is terminated.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_business_handle_requests_shutdown_and_settles() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let server_pid_file = fixture.path("server.pid");
        let script = format!(
            "echo $$ > {}; echo started > {}; sleep 30",
            server_pid_file.display(),
            marker.display()
        );
        let process = fixture.spawn(&script, Vec::new()).expect("spawn");
        wait_for_file(&marker, "server start marker");
        let server_pid = read_pid(&server_pid_file);
        drop(process);
        wait_for_reaped(server_pid, "server after business-handle drop");
    }

    /// A post-spawn handshake failure (the outer dies before connecting)
    /// settles the unit instead of stranding a raw child.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn post_spawn_handshake_failure_settles_without_stranding() {
        let fixture = Fixture::new();
        let process = fixture
            .spawn(
                "sleep 30",
                vec![(OUTER_FAIL_ENV.to_owned(), "1".to_owned())],
            )
            .expect("spawn");
        tokio::time::timeout(DEADLINE, process.wait_for_settlement())
            .await
            .expect("the driver must settle a handshake-failed unit");
    }

    /// The direct supervisor child is reaped before physical settlement is
    /// published.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn direct_supervisor_child_is_reaped_before_settlement() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let script = format!("echo started > {}; sleep 30", marker.display());
        let mut process = fixture.spawn(&script, Vec::new()).expect("spawn");
        wait_for_file(&marker, "server start marker");
        let supervisor_pid = i32::try_from(
            process
                .supervisor_child_pid
                .expect("test-only supervisor pid"),
        )
        .expect("pid fits i32");
        settle(&mut process).await;
        wait_for_reaped(supervisor_pid, "direct supervisor child");
    }

    /// stderr is drained until EOF even far beyond the bounded preview, so a
    /// server that floods stderr keeps operating; the retained preview stays
    /// bounded.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stderr_is_drained_beyond_the_bounded_preview() {
        let fixture = Fixture::new();
        let marker = fixture.path("operating");
        let script = format!(
            "i=0; while [ $i -lt 300 ]; do echo 'stderr flood line number {{{{i}}}} {}'; i=$((i+1)); done; echo operating > {}; sleep 30",
            "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            marker.display()
        );
        let mut process = fixture.spawn(&script, Vec::new()).expect("spawn");
        // The marker appears only if the stderr pipe keeps being drained
        // past the 64 KiB preview bound; a drain that stopped at the bound
        // would block the server on a full pipe forever.
        wait_for_file(&marker, "server continued operating marker");
        let preview_len = process.stderr_preview().len();
        assert!(
            preview_len <= MAX_PROCESS_OUTPUT_BYTES,
            "the retained stderr preview must stay bounded"
        );
        settle(&mut process).await;
    }

    /// An injected signaling failure escalates to containment: the unit
    /// still settles with the owned group terminal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn injected_signal_failure_escalates_to_containment() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let server_pid_file = fixture.path("server.pid");
        let script = format!(
            "echo $$ > {}; echo started > {}; sleep 30",
            server_pid_file.display(),
            marker.display()
        );
        let mut process = fixture
            .spawn(&script, vec![(FAIL_SIGNAL_ENV.to_owned(), "1".to_owned())])
            .expect("spawn");
        wait_for_file(&marker, "server start marker");
        let server_pid = read_pid(&server_pid_file);
        settle(&mut process).await;
        wait_for_reaped(server_pid, "server after injected signal failure");
    }
}
