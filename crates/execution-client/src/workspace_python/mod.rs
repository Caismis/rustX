//! One default uv-created Python environment per Workspace.
//!
//! Native Agent Bash, Workflow Bash, and Subagent Bash observe this owner.
//! Manual terminals, generic command execution, MCP, and Managed Python do not.
//! Session creation schedules preparation and does not wait for it.
//! An existing environment is reused or diagnosed; it is never deleted or rebuilt.

use std::collections::HashMap;
#[cfg(feature = "test-support")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;

use host::ExecutionHost;
use interpreter::FixedInterpreter;

mod guard;
mod host;
mod interpreter;
mod owner;
#[cfg(test)]
mod tests;

pub use guard::GUARD_MESSAGE_KEY;

/// The Workspace Python choice frozen into one command environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonSelection {
    pub path_prefix: Vec<PathBuf>,
    pub virtual_env: Option<PathBuf>,
    pub diagnostic: String,
    /// The specific guard directory could not be written and no fallback
    /// directory was available. An empty prefix is not a usable selection.
    pub guard_unavailable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    dev: u64,
    ino: u64,
    mtime: i64,
    mtime_nsec: i64,
    len: u64,
    mode: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PathStamp {
    entry: FileStamp,
    target: Option<FileStamp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Generation {
    root_dev: u64,
    root_ino: u64,
    pin_file: Option<PathStamp>,
    project_file: Option<PathStamp>,
    venv: Option<FileStamp>,
    config: Option<PathStamp>,
    python: Option<PathStamp>,
    python3: Option<PathStamp>,
}

impl Generation {
    fn same_inputs(&self, expected: &Self) -> bool {
        self.root_dev == expected.root_dev
            && self.root_ino == expected.root_ino
            && self.pin_file == expected.pin_file
            && self.project_file == expected.project_file
    }
}

#[derive(Clone, Debug)]
enum Record {
    Ready {
        generation: Generation,
        bin: PathBuf,
        virtual_env: PathBuf,
        guard: PathBuf,
        fixed: Option<Arc<FixedInterpreter>>,
    },
    Unavailable {
        generation: Generation,
        diagnostic: String,
        retry_after: Instant,
    },
    Conflict {
        generation: Generation,
        diagnostic: String,
        directory_candidate: Option<PathBuf>,
    },
}

impl Record {
    fn diagnostic(&self) -> String {
        match self {
            Self::Ready { .. } => String::new(),
            Self::Unavailable { diagnostic, .. } | Self::Conflict { diagnostic, .. } => {
                diagnostic.clone()
            }
        }
    }
}

struct Flight {
    done: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl Flight {
    fn new() -> Self {
        Self {
            done: std::sync::atomic::AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }

    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            if self.done.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    fn finish(&self) {
        self.done.store(true, std::sync::atomic::Ordering::Release);
        self.notify.notify_waiters();
    }
}

#[derive(Clone)]
pub(crate) struct ProgramRequest {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub cwd_identity: (u64, u64),
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
    pub cancellation: rustx_process_supervisor::signal::ProcessSignal,
}

#[derive(Clone, Debug)]
pub(crate) struct ProgramOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub(crate) trait ProgramProbe: Send + Sync {
    fn run(&self, request: ProgramRequest) -> BoxFuture<'_, Result<ProgramOutput, String>>;
}

#[cfg(test)]
struct TestGate {
    entered: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
}

#[cfg(test)]
async fn pass_gate(slot: &Mutex<Option<TestGate>>) {
    let gate = slot.lock().expect("test gate").take();
    if let Some(gate) = gate {
        let _ = gate.entered.send(());
        let _ = gate.release.await;
    }
}

struct Owner {
    records: Mutex<HashMap<PathBuf, Record>>,
    flights: Mutex<HashMap<PathBuf, Arc<Flight>>>,
    permits: tokio::sync::Semaphore,
    shutdown: rustx_process_supervisor::signal::ProcessSignal,
    #[cfg(test)]
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    #[cfg(test)]
    admitted: Mutex<std::collections::HashSet<PathBuf>>,
    #[cfg(test)]
    schedules: bool,
    probe: Arc<dyn ProgramProbe>,
    host_override: Option<Arc<ExecutionHost>>,
    state_override: Option<PathBuf>,
    #[cfg(test)]
    before_lock: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    #[cfg(test)]
    before_create: Mutex<Option<TestGate>>,
    #[cfg(test)]
    before_snapshot: Mutex<Option<TestGate>>,
    #[cfg(test)]
    before_signal: Mutex<Option<TestGate>>,
    #[cfg(test)]
    waiting_permit: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    #[cfg(test)]
    cache_miss: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    #[cfg(test)]
    after_interpreter: Mutex<Option<TestGate>>,
}

/// A physical Workspace Python owner for one admitted Client binding.
pub struct WorkspacePython {
    owner: Owner,
    root: PathBuf,
    permit: Arc<tokio::sync::Semaphore>,
    scheduled: AtomicBool,
}

impl WorkspacePython {
    /// Creates an owner with the binding's physical revocation signal.
    #[must_use]
    pub fn new(root: PathBuf, shutdown: rustx_process_supervisor::signal::ProcessSignal) -> Self {
        Self {
            owner: Owner::production(shutdown),
            permit: Arc::new(tokio::sync::Semaphore::new(1)),
            scheduled: AtomicBool::new(false),
            root,
        }
    }

    /// Acquires the one live preparation slot.
    pub fn prepare_permit(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let permit = self.permit.clone().try_acquire_owned().ok()?;
        if self.owner.current_record(&self.root).is_some() {
            return None;
        }
        self.scheduled.store(true, Ordering::Release);
        Some(permit)
    }

    /// Whether this binding has authorized physical preparation.
    #[must_use]
    pub fn scheduled(&self) -> bool {
        self.scheduled.load(Ordering::Acquire)
    }

    /// Current readiness at the Client, including input replacement.
    #[must_use]
    pub fn status(&self) -> rustx_execution_protocol::WorkspacePythonStatus {
        use rustx_execution_protocol::WorkspacePythonStatus;
        if !self.scheduled() {
            return WorkspacePythonStatus::Unscheduled;
        }
        match self.permit.clone().try_acquire_owned() {
            Ok(_permit) => {
                if matches!(
                    self.owner.current_record(&self.root),
                    Some(Record::Ready { .. })
                ) {
                    WorkspacePythonStatus::Ready
                } else {
                    WorkspacePythonStatus::Unavailable
                }
            }
            Err(_) => WorkspacePythonStatus::Preparing,
        }
    }

    /// Drives a Client preparation through physical settlement.
    pub async fn prepare(&self) {
        self.owner.prepare(self.root.clone()).await;
    }

    /// Applies the verified Workspace interpreter or its guard.
    ///
    /// # Errors
    ///
    /// Returns an error when an executable guard cannot be installed or
    /// the resulting environment is invalid.
    pub fn apply(&self, entries: &mut Vec<(String, String)>) -> Result<(), &'static str> {
        host::apply(entries, &self.owner.observe(&self.root))
    }
}

/// Builds the unprepared environment for an in-process supervisor fixture.
/// Product native Bash receives its selection from the Client binding.
#[cfg(feature = "test-support")]
#[must_use]
pub fn fixture_selection(root: &Path) -> PythonSelection {
    Owner::production(rustx_process_supervisor::signal::ProcessSignal::new()).observe(root)
}
