//! Bounded, opt-in native boundary evidence for process regression fixtures.
//! Never consulted for lifecycle decisions; no command or environment is logged.

use std::io::Write;
use std::path::PathBuf;

/// Explicit per-invocation fixture destination. Never enabled by default.
pub const TRACE_ENV: &str = "RUSTX_TEST_SUPERVISION_TRACE";

/// Trusted fixture gate after the outer's exact stop observation.
pub const ANCHOR_STOP_GATE_ENV: &str = "RUSTX_TEST_AFTER_ANCHOR_STOP_SOCKET";

/// Per-invocation Unix socket: test parks the inner after TERMINATE receipt,
/// before any TERM syscall. The fixture owns the peer and releases it explicitly.
pub const TERM_GATE_ENV: &str = "RUSTX_TEST_BEFORE_TERM_SOCKET";

pub(crate) fn before_term() {
    use std::io::Read;
    if let Some(path) = std::env::var_os(TERM_GATE_ENV)
        && let Ok(mut gate) = std::os::unix::net::UnixStream::connect(path)
    {
        let _ = gate.write_all(b"T");
        let _ = gate.read_exact(&mut [0]);
    }
}

/// Scalar native evidence; a signal syscall result is not per-process delivery.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum Event {
    /// Exact retained anchor stop, observed by the WSTOPPED wait.
    AnchorStopObserved,
    /// Unexpected nonterminal status from the WEXITED-only observation.
    AnchorExitWaitNonterminal { stopped: bool },
    /// Exact PID SIGKILL return (zero or errno).
    AnchorUnwedgeKillAttempt { result: i32 },
    /// Exact anchor terminal status, still retained by WNOWAIT.
    AnchorTerminalObserved,
    /// The owned-group wait reached ECHILD; Darwin still requires absence.
    GroupChildrenReaped,
    /// Darwin's positive group-absence proof completed.
    GroupAbsenceProven,
    /// The runner attempted the cancellation control frame; `sent` is write success.
    TerminateSent { sent: bool },
    /// The inner parsed the authoritative cancellation frame.
    TerminateReceived,
    /// Shell PID and its kernel-reported group, or negative errno.
    ShellGroup { pid: i32, pgid: i32 },
    /// Native group signal boundary. `result` is zero or errno; None means
    /// the call is about to execute (self-KILL cannot report its return).
    Signal {
        pgid: i32,
        signal: i32,
        result: Option<i32>,
    },
    /// The existing grace deadline has actually expired.
    GraceExpired,
    /// Sole reaper's raw shell exit status.
    ShellExited { status: i32 },
    /// Outer fallback containment while the anchor is still retained.
    FallbackContainment,
    /// The outer observed this exact retained inner exit status.
    InnerExited { status: i32 },
    /// A process-control failure frame was published (content is not logged).
    ControlFailure,
    /// Supervisor published the complete physical terminal frame.
    TerminalPublished,
    /// Runner parsed the terminal frame.
    TerminalObserved,
    /// Runner reaped its direct supervisor child.
    DirectChildReaped,
}

/// One bounded trace entry. File append order is observation order; process
/// identifies the emitting owner, never a reusable physical authority token.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub process: u32,
    pub event: Event,
}

pub(crate) struct Trace {
    path: PathBuf,
    remaining: usize,
}

impl Trace {
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            remaining: 32,
        }
    }

    pub(crate) fn record(&mut self, event: Event) {
        if self.remaining == 0 {
            return;
        }
        self.remaining -= 1;
        let entry = Entry {
            process: std::process::id(),
            event,
        };
        let Ok(mut bytes) = serde_json::to_vec(&entry) else {
            return;
        };
        bytes.push(b'\n');
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            // One small append, no inherited descriptor or shared test-process state.
            let _ = file.write(&bytes);
        }
    }
}

pub(crate) fn record(event: Event) {
    // Each dedicated supervisor has a single trace. The runner uses its own
    // explicitly configured instance, never this process environment.
    static TRACE: std::sync::OnceLock<std::sync::Mutex<Option<Trace>>> = std::sync::OnceLock::new();
    let trace =
        TRACE.get_or_init(|| std::sync::Mutex::new(std::env::var_os(TRACE_ENV).map(Trace::new)));
    if let Ok(mut trace) = trace.lock()
        && let Some(trace) = trace.as_mut()
    {
        trace.record(event);
    }
}

/// Trusted fixture capability, supplied explicitly by the test executor factory.
/// This is never constructed from ToolEnvironment or ambient environment.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct FixtureControl {
    pub trace: Option<PathBuf>,
    pub before_term_socket: Option<PathBuf>,
}

impl FixtureControl {
    pub(crate) fn configure(&self, supervisor: &mut tokio::process::Command) {
        if let Some(path) = &self.trace {
            supervisor.env(TRACE_ENV, path);
        }
        if let Some(path) = &self.before_term_socket {
            supervisor.env(TERM_GATE_ENV, path);
        }
    }
}

/// Constructs the explicitly trusted executor used by external regression fixtures.
#[doc(hidden)]
#[must_use]
pub fn fixture_executor(
    control: FixtureControl,
) -> std::sync::Arc<dyn crate::tools::executor::ToolExecutor> {
    std::sync::Arc::new(crate::tools::native::bash::BashTool::with_diagnostic_fixture(control))
}

/// Explicit fixture-only pause after the native stop observation. Diagnostics
/// are recorded first; the fixture releases this before containment proceeds.
pub(crate) fn after_anchor_stop() {
    use std::io::Read;
    if let Some(path) = std::env::var_os(ANCHOR_STOP_GATE_ENV) {
        let mut gate = std::os::unix::net::UnixStream::connect(path)
            .expect("anchor-stop fixture connect failed");
        gate.write_all(b"S")
            .expect("anchor-stop fixture write failed");
        gate.read_exact(&mut [0])
            .expect("anchor-stop fixture release failed");
    }
}
