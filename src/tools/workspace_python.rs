//! One default uv-created Python environment per Workspace.
//!
//! Native Agent Bash, Workflow Bash, and Subagent Bash observe this owner.
//! Manual terminals, generic command execution, MCP, and Managed Python do not.
//! Registration and session creation schedule preparation and do not wait for it.
//! An existing environment is reused or diagnosed; it is never deleted or rebuilt.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use notify::Watcher;

use super::execution_environment::ExecutionHost;
use interpreter::FixedInterpreter;

mod guard;
mod interpreter;
mod owner;
#[cfg(test)]
pub(crate) mod tests;

pub(crate) use guard::GUARD_MESSAGE_KEY;

/// The Workspace Python choice frozen into one command environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PythonSelection {
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
    pub cancellation: crate::runtime::cancellation::CancellationSignal,
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
    shutdown: crate::runtime::cancellation::CancellationSignal,
    probe: Arc<dyn ProgramProbe>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    admitted: Mutex<HashSet<PathBuf>>,
    host_override: Option<Arc<ExecutionHost>>,
    state_override: Option<PathBuf>,
    schedules: bool,
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

/// The current Workspace Python selection. This never waits for uv.
pub(crate) fn observe(root: &Path) -> PythonSelection {
    owner::global().observe(root)
}

/// Schedules preparation without waiting. Session creation and registration use this.
pub(crate) fn schedule(root: &Path) {
    owner::global().schedule_on(root);
}

/// Prepares one Workspace and waits. Registration and native Bash do not call
/// this. A caller that has already chosen to wait, including a test that needs
/// the selection before the first command, can.
pub async fn prepare_now(root: &Path) {
    let Ok(canonical) = std::fs::canonicalize(root) else {
        return;
    };
    owner::global().prepare(canonical).await;
}

#[cfg(test)]
pub(crate) fn publish_ready_for_test(root: &Path) {
    let root = std::fs::canonicalize(root).expect("workspace");
    let target = root.join(".venv");
    let bin = target.join("bin");
    assert!(
        bin.join("python3").exists(),
        "a test-ready environment needs python3"
    );
    let guard = guard::ensure_guard("Workspace Python is no longer available")
        .or_else(guard::fallback_guard)
        .expect("python guard");
    owner::global()
        .records
        .lock()
        .expect("workspace python records")
        .insert(
            root.clone(),
            Record::Ready {
                generation: owner::generation_of(&root),
                bin,
                virtual_env: target,
                guard,
                fixed: None,
            },
        );
}

/// Watches the registration handoff directory until `shutdown` fires.
pub(crate) fn start_admit_watcher(shutdown: tokio_util::sync::CancellationToken) {
    tokio::spawn(async move {
        if let Some(directory) = owner::admit_directory() {
            let _ = std::fs::create_dir_all(&directory);
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let watch =
                notify::recommended_watcher(move |_event: notify::Result<notify::Event>| {
                    let _ = sender.send(());
                })
                .and_then(|mut watcher| {
                    watcher.watch(&directory, notify::RecursiveMode::NonRecursive)?;
                    Ok(watcher)
                });
            owner::receive_admissions(&directory, shutdown, watch, receiver).await;
        }
    });
}

pub(crate) async fn shutdown() {
    owner::global().stop().await;
}
