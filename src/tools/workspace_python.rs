//! One default uv-created Python environment per Workspace.
//!
//! Native Agent Bash, Workflow Bash, and Subagent Bash observe this owner.
//! Manual terminals, generic command execution, MCP, and Managed Python do not.
//! Registration and session creation schedule preparation and do not wait for it.
//! An existing environment is reused or diagnosed; it is never deleted or rebuilt.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use notify::Watcher;
use pep440_rs::{Version, VersionSpecifier, VersionSpecifiers};
use sha2::{Digest, Sha256};

use super::execution_environment::{self, ExecutionHost};

const UV_MINIMUM: &str = "0.10.0";
const CREATE_TIMEOUT: Duration = Duration::from_mins(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
const RETRY_COOLDOWN: Duration = Duration::from_secs(30);
const EXTERNAL_PERMITS: usize = 2;

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

fn global() -> &'static Arc<Owner> {
    static OWNER: OnceLock<Arc<Owner>> = OnceLock::new();
    OWNER.get_or_init(|| Arc::new(Owner::production()))
}

impl Owner {
    fn production() -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
            flights: Mutex::new(HashMap::new()),
            permits: tokio::sync::Semaphore::new(EXTERNAL_PERMITS),
            shutdown: crate::runtime::cancellation::CancellationSignal::new(),
            probe: production_probe(),
            tasks: Mutex::new(Vec::new()),
            admitted: Mutex::new(HashSet::new()),
            host_override: None,
            state_override: None,
            schedules: true,
            #[cfg(test)]
            before_lock: Mutex::new(None),
            #[cfg(test)]
            before_create: Mutex::new(None),
            #[cfg(test)]
            before_snapshot: Mutex::new(None),
            #[cfg(test)]
            before_signal: Mutex::new(None),
            #[cfg(test)]
            waiting_permit: Mutex::new(None),
            #[cfg(test)]
            cache_miss: Mutex::new(None),
            #[cfg(test)]
            after_interpreter: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn for_test(
        probe: Arc<dyn ProgramProbe>,
        host: Option<Arc<ExecutionHost>>,
        state: Option<PathBuf>,
        before_lock: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Self {
        Self {
            probe,
            host_override: host,
            state_override: state,
            schedules: false,
            before_lock: Mutex::new(before_lock),
            ..Self::production()
        }
    }

    fn host(&self) -> Arc<ExecutionHost> {
        self.host_override
            .clone()
            .unwrap_or_else(execution_environment::current)
    }

    fn observe(&self, root: &Path) -> PythonSelection {
        let Ok(canonical) = std::fs::canonicalize(root) else {
            return guarded("the workspace directory is unavailable");
        };
        let generation = generation_of(&canonical);
        let record = self
            .records
            .lock()
            .expect("workspace python records")
            .get(&canonical)
            .cloned();
        match &record {
            Some(Record::Ready {
                generation: stored,
                bin,
                virtual_env,
                guard,
                fixed,
            }) if stored == &generation
                && fixed.as_deref().is_none_or(FixedInterpreter::is_current)
                && validated_guard_directory(guard).is_some() =>
            {
                return PythonSelection {
                    path_prefix: vec![bin.clone(), guard.clone()],
                    virtual_env: Some(virtual_env.clone()),
                    diagnostic: String::new(),
                    guard_unavailable: false,
                };
            }
            Some(Record::Unavailable {
                generation: stored,
                diagnostic,
                retry_after,
            }) if stored == &generation && Instant::now() < *retry_after => {
                return guarded(diagnostic);
            }
            Some(Record::Conflict {
                generation: stored,
                diagnostic,
                directory_candidate,
            }) if stored == &generation
                && directory_candidate
                    .as_ref()
                    .is_none_or(|path| !path.is_dir()) =>
            {
                return guarded(diagnostic);
            }
            _ => {}
        }
        #[cfg(test)]
        if let Some(miss) = self.cache_miss.lock().expect("cache miss").take() {
            let _ = miss.send(());
        }
        if self.schedules && std::ptr::eq(self, Arc::as_ptr(global())) {
            global().schedule_on(&canonical);
        }
        let diagnostic = record
            .map(|record| record.diagnostic())
            .filter(|diagnostic| !diagnostic.is_empty())
            .unwrap_or_else(|| "Workspace Python is not ready".to_owned());
        guarded(&diagnostic)
    }

    fn schedule_on(self: &Arc<Self>, root: &Path) {
        const MAX_LIVE_TASKS: usize = 64;
        if !self.schedules {
            return;
        }
        let Some(handle) = tokio::runtime::Handle::try_current().ok() else {
            return;
        };
        let Ok(canonical) = std::fs::canonicalize(root) else {
            return;
        };
        {
            let mut admitted = self.admitted.lock().expect("workspace python admission");
            if !admitted.insert(canonical.clone()) {
                return;
            }
        }
        let mut tasks = self.tasks.lock().expect("workspace python tasks");
        if self.shutdown.is_cancelled() {
            self.admitted
                .lock()
                .expect("workspace python admission")
                .remove(&canonical);
            return;
        }
        tasks.retain(|task| !task.is_finished());
        if tasks.len() >= MAX_LIVE_TASKS {
            drop(tasks);
            self.admitted
                .lock()
                .expect("workspace python admission")
                .remove(&canonical);
            return;
        }
        let owner = Arc::clone(self);
        let task = handle.spawn(async move {
            let _leave = AdmitLeave {
                owner: Arc::clone(&owner),
                root: canonical.clone(),
            };
            owner.prepare(canonical).await;
        });
        tasks.push(task);
    }

    async fn prepare(&self, root: PathBuf) {
        let Ok(canonical) = std::fs::canonicalize(&root) else {
            self.unavailable(
                &root,
                "the workspace directory is unavailable",
                generation_of(&root),
            );
            return;
        };
        let (flight, leader) = {
            let mut flights = self.flights.lock().expect("workspace python flights");
            if self.shutdown.is_cancelled() {
                return;
            }
            if let Some(existing) = flights.get(&canonical) {
                (Arc::clone(existing), false)
            } else {
                let flight = Arc::new(Flight::new());
                flights.insert(canonical.clone(), Arc::clone(&flight));
                (flight, true)
            }
        };
        if !leader {
            flight.wait().await;
            return;
        }
        let _finish = FinishFlight {
            owner: self,
            root: canonical.clone(),
            flight,
        };
        self.prepare_locked(&canonical).await;
    }

    async fn prepare_locked(&self, root: &Path) {
        if self.shutdown.is_cancelled() {
            return;
        }
        let generation = generation_of(root);
        let lock = match acquire_lock(
            root,
            self.before_lock(),
            self.state_override.clone(),
            self.shutdown.clone(),
        )
        .await
        {
            Ok(lock) => lock,
            Err(message) => {
                self.unavailable(root, &message, generation);
                return;
            }
        };
        if generation_of(root) != generation {
            drop(lock);
            return;
        }
        let pin = read_pin(root);
        let requires = read_requires(root);
        if generation_of(root) != generation {
            drop(lock);
            return;
        }
        let pin = match pin {
            Ok(pin) => pin,
            Err(message) => {
                self.conflict(root, message, generation);
                return;
            }
        };
        if matches!(requires, Requires::Invalid) {
            self.conflict(root, "requires-python cannot be parsed", generation.clone());
            drop(lock);
            return;
        }
        let requires = match &requires {
            Requires::Spec(spec) => Some(spec.as_str()),
            Requires::Absent | Requires::Invalid => None,
        };
        if let Some(pin) = pin.as_deref()
            && !path_request(pin)
            && parse_request(pin).is_err()
        {
            self.conflict_with_directory_candidate(
                root,
                ".python-version cannot be verified",
                generation.clone(),
                Some(root.join(pin)),
            );
            drop(lock);
            return;
        }
        if let (Some(pin), Some(requires)) = (pin.as_deref(), requires)
            && !path_request(pin)
            && exact_pin_conflicts(pin, requires)
        {
            self.conflict(
                root,
                ".python-version conflicts with requires-python",
                generation.clone(),
            );
            drop(lock);
            return;
        }
        let target = root.join(".venv");
        match classify_target(&target) {
            Target::Absent => {
                self.create(root, pin.as_deref(), requires, &generation)
                    .await;
            }
            Target::Directory => {
                self.adopt(root, &target, pin.as_deref(), requires, &generation)
                    .await;
            }
            Target::Conflict(diagnostic) => {
                self.conflict(root, diagnostic, generation);
            }
        }
        drop(lock);
    }

    fn inputs_match(&self, root: &Path, expected: &Generation) -> bool {
        if self.shutdown.is_cancelled() {
            return false;
        }
        let current = generation_of(root);
        current.same_inputs(expected)
    }

    async fn create(
        &self,
        root: &Path,
        pin: Option<&str>,
        requires: Option<&str>,
        expected: &Generation,
    ) {
        let host = self.host();
        if host.closed().is_some() {
            self.unavailable(root, "PATH or HOME is not Unicode", expected.clone());
            return;
        }
        let Some(uv) = find_uv(host.as_ref(), root) else {
            self.unavailable(root, "uv was not found on the host PATH", expected.clone());
            return;
        };
        let version = match self.uv_version(root, &uv, host.as_ref(), expected).await {
            Ok(version) => version,
            Err(message) => {
                if self.inputs_match(root, expected) {
                    self.unavailable(root, &message, expected.clone());
                }
                return;
            }
        };
        if !self.inputs_match(root, expected) {
            return;
        }
        if !uv_can_create(&version) {
            self.unavailable(
                root,
                "uv is older than 0.10.0 and cannot create a Workspace environment",
                expected.clone(),
            );
            return;
        }
        let request = match creation_request(root, pin, requires) {
            Ok(request) => request,
            Err(message) => {
                self.conflict(root, message, expected.clone());
                return;
            }
        };
        let fixed = match self
            .check_fixed_interpreter(root, host.as_ref(), pin, requires, expected)
            .await
        {
            Ok(fixed) => fixed,
            Err(message) => {
                if self.inputs_match(root, expected) {
                    // The requested executable is outside Generation. Its
                    // installation or repair must become observable on retry.
                    self.unavailable(root, message, expected.clone());
                }
                return;
            }
        };
        let target = root.join(".venv");
        let mut argv = vec![uv.display().to_string(), "venv".to_owned()];
        argv.extend(isolated_discovery_args());
        // An explicit request always bypasses ancestor version files, including
        // ancestors of the private directory. uv still owns discovery, policy,
        // downloads and version selection; the PEP 440 conjunction includes
        // every root requirement before uv can write the final target.
        // A fixed launcher has already been probed. Dispatch that validated
        // executable so mutable launcher configuration cannot change selection.
        let request = fixed
            .as_ref()
            .map_or(request, |fixed| fixed.executable.display().to_string());
        argv.extend(["--python".to_owned(), request]);
        argv.push(target.display().to_string());
        #[cfg(test)]
        pass_gate(&self.before_create).await;
        let Ok(permit) = self.acquire_permit().await else {
            return;
        };
        if !self.inputs_match(root, expected)
            || fixed.as_ref().is_some_and(|fixed| !fixed.is_current())
        {
            return;
        }
        // This is the last classification before native creation dispatch.
        // Waiting for capacity grants no permission to initialize a target
        // another process added while this operation was suspended.
        match classify_target(&target) {
            Target::Absent => {}
            Target::Directory => {
                drop(permit);
                self.adopt(root, &target, pin, requires, expected).await;
                return;
            }
            Target::Conflict(message) => {
                self.conflict(root, message, generation_of(root));
                return;
            }
        }
        let program = self.program_request(root, argv, host.as_ref(), CREATE_TIMEOUT, expected);
        let result = self.dispatch(program, &permit).await;
        drop(permit);
        let failure = match result {
            Ok(output) if output.status == 0 => None,
            Ok(output) => Some(bounded_diagnostic(
                "uv could not create Workspace Python",
                &output.stderr,
            )),
            Err(message) => Some(message),
        };
        if let Some(message) = failure {
            if self.inputs_match(root, expected) {
                self.unavailable(root, &message, generation_of(root));
            }
            return;
        }
        self.adopt(root, &target, pin, requires, expected).await;
    }

    async fn adopt(
        &self,
        root: &Path,
        target: &Path,
        pin: Option<&str>,
        requires: Option<&str>,
        expected: &Generation,
    ) {
        // Snapshot after the environment exists and before its interpreters are
        // probed. Creating `.venv` changes these stamps; a later replacement or
        // a pin edit has not been checked and cannot be published.
        #[cfg(test)]
        pass_gate(&self.before_snapshot).await;
        let probed = generation_of(root);
        if self.shutdown.is_cancelled() || !probed.same_inputs(expected) {
            return;
        }
        // The same snapshot authorizes inputs, probes and Ready publication.
        // Root links are conflicts even if their resolved prefix is valid.
        if let Target::Conflict(message) = classify_target(target) {
            self.conflict(root, message, probed);
            return;
        }
        if classify_target(target) != Target::Directory {
            return;
        }
        if generation_of(root) != probed {
            return;
        }
        let host = self.host();
        let mut fixed: Option<FixedInterpreter> = None;
        for name in ["python", "python3"] {
            let program = target.join("bin").join(name);
            let current = match self
                .check_interpreter(root, &program, host.as_ref(), pin, requires, &probed)
                .await
            {
                Ok(current) => current,
                Err(error) => {
                    if generation_of(root) == probed {
                        match error {
                            InterpreterError::Conflict(message) => {
                                self.conflict(root, message, probed);
                            }
                            InterpreterError::Unavailable(message) => {
                                self.unavailable(root, message, probed);
                            }
                        }
                    }
                    return;
                }
            };
            // Retain the first witness across both probe awaits. Resampling
            // after an alias change cannot authorize the earlier validation.
            if generation_of(root) != probed
                || fixed.as_ref().is_some_and(|fixed| !fixed.is_current())
            {
                return;
            }
            if fixed.as_ref().is_some_and(|first| {
                current
                    .as_ref()
                    .is_none_or(|current| current.executable != first.executable)
            }) {
                self.unavailable(
                    root,
                    "the Python interpreter changed during validation",
                    probed,
                );
                return;
            }
            if fixed.is_none() {
                fixed = current;
            }
            #[cfg(test)]
            pass_gate(&self.after_interpreter).await;
        }
        let Some(guard) =
            ensure_guard("Workspace Python is no longer available").or_else(fallback_guard)
        else {
            if generation_of(root) == probed {
                self.unavailable(
                    root,
                    "the Workspace Python guard could not be installed",
                    probed,
                );
            }
            return;
        };
        if generation_of(root) != probed || fixed.as_ref().is_some_and(|fixed| !fixed.is_current())
        {
            return;
        }
        self.records
            .lock()
            .expect("workspace python records")
            .insert(
                root.to_path_buf(),
                Record::Ready {
                    generation: probed,
                    bin: target.join("bin"),
                    virtual_env: target.to_path_buf(),
                    guard,
                    fixed: fixed.map(Arc::new),
                },
            );
        self.bound_records();
    }

    async fn uv_version(
        &self,
        root: &Path,
        uv: &Path,
        host: &ExecutionHost,
        expected: &Generation,
    ) -> Result<String, String> {
        let output = self
            .run(
                root,
                vec![uv.display().to_string(), "--version".to_owned()],
                host,
                PROBE_TIMEOUT,
                expected,
            )
            .await?;
        if output.status != 0 {
            return Err("uv --version failed".to_owned());
        }
        output
            .stdout
            .split_whitespace()
            .nth(1)
            .map(str::to_owned)
            .ok_or_else(|| "uv --version could not be parsed".to_owned())
    }

    /// Resolve file/launcher and installation-directory requests identically
    /// for creation and adoption, without requiring uv. Validate the selected
    /// entry before target writes and retain its identity across capacity waits.
    async fn check_fixed_interpreter(
        &self,
        root: &Path,
        host: &ExecutionHost,
        pin: Option<&str>,
        requires: Option<&str>,
        expected: &Generation,
    ) -> Result<Option<FixedInterpreter>, &'static str> {
        let Some(pin) = pin.filter(|pin| path_request(pin)) else {
            return Ok(None);
        };
        let path = if Path::new(pin).is_absolute() {
            PathBuf::from(pin)
        } else {
            root.join(pin)
        };
        let path_stamp = stamp(&path).ok_or("the Python interpreter cannot be inspected")?;
        let entry = fixed_interpreter_entry(&path)?;
        let entry_stamp = stamp(&entry).ok_or("the Python interpreter cannot be inspected")?;
        let interpreter = self.probe_interpreter(root, &entry, host, expected).await?;
        interpreter.check_requirements(Some(pin), requires)?;
        let executable = std::fs::canonicalize(&interpreter.executable)
            .map_err(|_| "the Python interpreter cannot be inspected")?;
        let executable_stamp =
            stamp(&executable).ok_or("the Python interpreter cannot be inspected")?;
        if std::fs::canonicalize(&entry).ok().as_ref() != Some(&executable) {
            // The launcher probe authorized metadata, not the later contents
            // of its reported executable path. Validate that actual file
            // within its own before/after stamp witness.
            let actual = self
                .probe_interpreter(root, &executable, host, expected)
                .await?;
            actual.check_requirements(Some(pin), requires)?;
            if std::fs::canonicalize(&actual.executable).ok().as_ref() != Some(&executable) {
                return Err("the Python interpreter changed during validation");
            }
        }
        let fixed = FixedInterpreter {
            path,
            stamp: path_stamp,
            entry,
            entry_stamp,
            executable,
            executable_stamp,
        };
        if !fixed.is_current() {
            return Err("the Python interpreter changed during validation");
        }
        Ok(Some(fixed))
    }

    async fn check_interpreter(
        &self,
        root: &Path,
        program: &Path,
        host: &ExecutionHost,
        pin: Option<&str>,
        requires: Option<&str>,
        expected: &Generation,
    ) -> Result<Option<FixedInterpreter>, InterpreterError> {
        use InterpreterError::{Conflict, Unavailable};
        let interpreter = self
            .probe_interpreter(root, program, host, expected)
            .await
            .map_err(Conflict)?;
        {
            let venv = root.join(".venv");
            let Ok(actual) = std::fs::canonicalize(&interpreter.prefix) else {
                return Err(Conflict(
                    "Workspace .venv is not a valid virtual environment",
                ));
            };
            let Ok(expected) = std::fs::canonicalize(&venv) else {
                return Err(Conflict(
                    "Workspace .venv is not a valid virtual environment",
                ));
            };
            if actual != expected || interpreter.prefix == interpreter.base {
                return Err(Conflict(
                    "Workspace .venv is not a valid virtual environment",
                ));
            }
        }
        interpreter
            .check_requirements(pin, requires)
            .map_err(Conflict)?;
        let fixed = if let Some(pin) = pin.filter(|pin| path_request(pin)) {
            let fixed = self
                .check_fixed_interpreter(root, host, Some(pin), requires, expected)
                .await
                .map_err(Unavailable)?
                .expect("path request");
            // A launcher (notably /usr/bin/python3 on macOS) may exec a
            // different binary. Compare the interpreters' reported base
            // executables, rather than the launcher and venv symlink paths.
            if !fixed.is_current()
                || std::fs::canonicalize(&interpreter.executable).ok().as_ref()
                    != Some(&fixed.executable)
            {
                return Err(Unavailable(
                    "Workspace Python does not satisfy .python-version",
                ));
            }
            Some(fixed)
        } else {
            None
        };
        Ok(fixed)
    }

    async fn probe_interpreter(
        &self,
        root: &Path,
        program: &Path,
        host: &ExecutionHost,
        expected: &Generation,
    ) -> Result<Interpreter, &'static str> {
        let output = self
            .run(
                root,
                vec![
                    program.display().to_string(),
                    "-I".to_owned(),
                    "-c".to_owned(),
                    "import platform, sys; print(sys.prefix); print(sys.base_prefix); print(platform.python_version()); print(sys.implementation.name); print(getattr(sys, '_base_executable', sys.executable))".to_owned(),
                ],
                host,
                PROBE_TIMEOUT,
                expected,
            )
            .await
            .map_err(|_| "the Python interpreter could not be probed")?;
        if output.status != 0 {
            return Err("the Python interpreter could not be probed");
        }
        let mut lines = output.stdout.lines();
        let prefix = lines.next().unwrap_or("").trim().to_owned();
        let base = lines.next().unwrap_or("").trim().to_owned();
        let version_text = lines.next().unwrap_or("").trim();
        let implementation = lines.next().unwrap_or("").trim().to_owned();
        let executable = PathBuf::from(lines.next().unwrap_or("").trim());
        Ok(Interpreter {
            prefix,
            base,
            version: Version::from_str(version_text)
                .map_err(|_| "the Python version could not be verified")?,
            implementation,
            executable,
        })
    }

    async fn run(
        &self,
        root: &Path,
        argv: Vec<String>,
        host: &ExecutionHost,
        timeout: Duration,
        expected: &Generation,
    ) -> Result<ProgramOutput, String> {
        let permit = self.acquire_permit().await?;
        if !self.inputs_match(root, expected) {
            return Err("Workspace Python preparation stopped or inputs changed".to_owned());
        }
        self.dispatch(
            self.program_request(root, argv, host, timeout, expected),
            &permit,
        )
        .await
    }

    async fn acquire_permit(&self) -> Result<tokio::sync::SemaphorePermit<'_>, String> {
        let acquire = self.permits.acquire();
        #[cfg(test)]
        let acquire = {
            let mut acquire = Box::pin(acquire);
            futures_util::future::poll_fn(move |cx| {
                let state = std::future::Future::poll(acquire.as_mut(), cx);
                if state.is_pending()
                    && let Some(waiting) =
                        self.waiting_permit.lock().expect("waiting permit").take()
                {
                    let _ = waiting.send(());
                }
                state
            })
        };
        tokio::select! {
            result = acquire => result.map_err(|_| "Workspace Python preparation stopped".to_owned()),
            () = self.shutdown.cancelled() => Err("Workspace Python preparation stopped".to_owned()),
        }
    }

    fn program_request(
        &self,
        root: &Path,
        argv: Vec<String>,
        host: &ExecutionHost,
        timeout: Duration,
        expected: &Generation,
    ) -> ProgramRequest {
        ProgramRequest {
            argv,
            cwd: root.to_path_buf(),
            cwd_identity: (expected.root_dev, expected.root_ino),
            env: execution_environment::bootstrap_environment(host),
            timeout,
            cancellation: self.shutdown.child(),
        }
    }

    async fn dispatch(
        &self,
        request: ProgramRequest,
        _permit: &tokio::sync::SemaphorePermit<'_>,
    ) -> Result<ProgramOutput, String> {
        #[cfg(test)]
        pass_gate(&self.before_signal).await;
        self.probe.run(request).await
    }

    fn bound_records(&self) {
        const MAX_RECORDS: usize = 128;
        let mut records = self.records.lock().expect("workspace python records");
        if records.len() <= MAX_RECORDS {
            return;
        }
        let flights = self.flights.lock().expect("workspace python flights");
        let victims: Vec<PathBuf> = records
            .keys()
            .filter(|path| !flights.contains_key(*path))
            .cloned()
            .collect();
        drop(flights);
        for victim in victims {
            if records.len() <= MAX_RECORDS {
                break;
            }
            records.remove(&victim);
        }
    }

    fn unavailable(&self, root: &Path, diagnostic: &str, generation: Generation) {
        self.records
            .lock()
            .expect("workspace python records")
            .insert(
                root.to_path_buf(),
                Record::Unavailable {
                    generation,
                    diagnostic: diagnostic.to_owned(),
                    retry_after: Instant::now() + RETRY_COOLDOWN,
                },
            );
        self.bound_records();
    }

    fn conflict(&self, root: &Path, diagnostic: &str, generation: Generation) {
        self.conflict_with_directory_candidate(root, diagnostic, generation, None);
    }

    fn conflict_with_directory_candidate(
        &self,
        root: &Path,
        diagnostic: &str,
        generation: Generation,
        directory_candidate: Option<PathBuf>,
    ) {
        self.records
            .lock()
            .expect("workspace python records")
            .insert(
                root.to_path_buf(),
                Record::Conflict {
                    generation,
                    diagnostic: diagnostic.to_owned(),
                    directory_candidate,
                },
            );
        self.bound_records();
    }

    #[cfg(test)]
    fn before_lock(&self) -> Option<tokio::sync::oneshot::Sender<()>> {
        self.before_lock.lock().expect("before lock").take()
    }

    #[cfg(not(test))]
    #[allow(clippy::unused_self)]
    fn before_lock(&self) -> Option<tokio::sync::oneshot::Sender<()>> {
        None
    }
}

enum InterpreterError {
    Conflict(&'static str),
    Unavailable(&'static str),
}

#[derive(Debug)]
struct FixedInterpreter {
    path: PathBuf,
    stamp: PathStamp,
    entry: PathBuf,
    entry_stamp: PathStamp,
    executable: PathBuf,
    executable_stamp: PathStamp,
}

impl FixedInterpreter {
    fn is_current(&self) -> bool {
        stamp(&self.path).as_ref() == Some(&self.stamp)
            && fixed_interpreter_entry(&self.path).ok().as_ref() == Some(&self.entry)
            && stamp(&self.entry).as_ref() == Some(&self.entry_stamp)
            && stamp(&self.executable).as_ref() == Some(&self.executable_stamp)
    }
}

/// Resolve Unix installations in bin/python, then bin/python3 order, independent
/// of the installed uv version (or its absence during adoption). Re-resolve:
/// adding bin/python must invalidate a prior bin/python3 selection too.
fn fixed_interpreter_entry(path: &Path) -> Result<PathBuf, &'static str> {
    let metadata =
        std::fs::metadata(path).map_err(|_| "the Python interpreter cannot be inspected")?;
    if metadata.is_dir() {
        let python = path.join("bin/python");
        let entry = match std::fs::symlink_metadata(&python) {
            Ok(_) => python,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.join("bin/python3"),
            Err(_) => return Err("the preferred Python installation entry cannot be inspected"),
        };
        if std::fs::metadata(&entry).is_ok_and(|meta| meta.is_file() && meta.mode() & 0o111 != 0) {
            return Ok(entry);
        }
        return Err("the selected bin/python or bin/python3 is missing or not an executable file");
    }
    if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
        return Err("the Python interpreter is not an executable file");
    }
    Ok(path.to_path_buf())
}

struct Interpreter {
    prefix: String,
    base: String,
    version: Version,
    implementation: String,
    executable: PathBuf,
}

impl Interpreter {
    fn check_requirements(
        &self,
        pin: Option<&str>,
        requires: Option<&str>,
    ) -> Result<(), &'static str> {
        if let Some(requires) = requires {
            let specs = VersionSpecifiers::from_str(requires)
                .map_err(|_| "requires-python cannot be parsed")?;
            if !specs.contains(&self.version) {
                return Err("Workspace Python does not satisfy requires-python");
            }
        }
        if let Some(pin) = pin.filter(|pin| !path_request(pin)) {
            request_satisfied(pin, &self.version, &self.implementation)?;
        }
        Ok(())
    }
}

struct FinishFlight<'a> {
    owner: &'a Owner,
    root: PathBuf,
    flight: Arc<Flight>,
}

impl Drop for FinishFlight<'_> {
    fn drop(&mut self) {
        self.owner
            .flights
            .lock()
            .expect("workspace python flights")
            .remove(&self.root);
        self.flight.finish();
    }
}

struct AdmitLeave {
    owner: Arc<Owner>,
    root: PathBuf,
}

impl Drop for AdmitLeave {
    fn drop(&mut self) {
        self.owner
            .admitted
            .lock()
            .expect("workspace python admission")
            .remove(&self.root);
    }
}

fn production_probe() -> Arc<dyn ProgramProbe> {
    #[cfg(test)]
    {
        Arc::new(ClosedProbe)
    }
    #[cfg(not(test))]
    {
        Arc::new(SupervisedProbe)
    }
}

#[cfg(test)]
struct ClosedProbe;

#[cfg(test)]
impl ProgramProbe for ClosedProbe {
    fn run(&self, _request: ProgramRequest) -> BoxFuture<'_, Result<ProgramOutput, String>> {
        Box::pin(async { Err("workspace python probe is not installed".to_owned()) })
    }
}

struct SupervisedProbe;

impl ProgramProbe for SupervisedProbe {
    fn run(&self, request: ProgramRequest) -> BoxFuture<'_, Result<ProgramOutput, String>> {
        Box::pin(async move {
            let invocation = crate::runtime::process_runner::ProgramInvocation {
                argv: request.argv,
                stdin_socket: None,
                terminal: None,
                cwd_identity: request.cwd_identity,
            };
            let spec = crate::runtime::process_runner::SupervisedCommandSpec {
                command: "workspace-python".to_owned(),
                cwd: request.cwd,
                command_environment: request.env,
                timeout: Some(request.timeout),
                cancellation: request.cancellation,
            };
            let result =
                crate::runtime::process_runner::run_supervised_program(spec, invocation).await?;
            let status = result
                .exit_code
                .ok_or_else(|| "the workspace python program did not exit normally".to_owned())?;
            Ok(ProgramOutput {
                status,
                stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
            })
        })
    }
}

/// The current Workspace Python selection. This never waits for uv.
pub(crate) fn observe(root: &Path) -> PythonSelection {
    global().observe(root)
}

/// Schedules preparation without waiting. Session creation and registration use this.
pub(crate) fn schedule(root: &Path) {
    global().schedule_on(root);
}

/// Prepares one Workspace and waits. Registration and native Bash do not call
/// this. A caller that has already chosen to wait, including a test that needs
/// the selection before the first command, can.
pub async fn prepare_now(root: &Path) {
    let Ok(canonical) = std::fs::canonicalize(root) else {
        return;
    };
    global().prepare(canonical).await;
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
    let guard = ensure_guard("Workspace Python is no longer available")
        .or_else(fallback_guard)
        .expect("python guard");
    global()
        .records
        .lock()
        .expect("workspace python records")
        .insert(
            root.clone(),
            Record::Ready {
                generation: generation_of(&root),
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
        if let Some(directory) = admit_directory() {
            let _ = std::fs::create_dir_all(&directory);
            scan_admit(&directory);
            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            if let Ok(mut watcher) =
                notify::recommended_watcher(move |_event: notify::Result<notify::Event>| {
                    let _ = sender.send(());
                })
                && watcher
                    .watch(&directory, notify::RecursiveMode::NonRecursive)
                    .is_ok()
            {
                loop {
                    tokio::select! {
                        () = shutdown.cancelled() => break,
                        () = global().shutdown.cancelled() => break,
                        Some(()) = receiver.recv() => scan_admit(&directory),
                    }
                }
            }
        }
    });
}

pub(crate) async fn shutdown() {
    global().stop().await;
}

impl Owner {
    async fn stop(&self) {
        self.shutdown.cancel();
        let tasks = std::mem::take(&mut *self.tasks.lock().expect("workspace python tasks"));
        for task in tasks {
            let _ = task.await;
        }
        let flights: Vec<_> = self
            .flights
            .lock()
            .expect("workspace python flights")
            .values()
            .cloned()
            .collect();
        for flight in flights {
            flight.wait().await;
        }
    }
}

fn scan_admit(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.take(32).flatten() {
        let path = entry.path();
        let Ok(Some(text)) = read_optional_file(&path) else {
            continue;
        };
        let location = text.trim();
        if Path::new(location).is_absolute() {
            schedule(Path::new(location));
        }
        let _ = std::fs::remove_file(path);
    }
}

fn admit_directory() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("admit"))
}

#[cfg(test)]
#[allow(clippy::unnecessary_wraps)] // production returns None when HOME is missing
fn state_dir() -> Option<PathBuf> {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    if let Some(path) = std::env::var_os("RUSTX_WORKSPACE_PYTHON_STATE") {
        return Some(PathBuf::from(path));
    }
    Some(
        DIR.get_or_init(|| {
            std::env::temp_dir().join(format!("rustx-workspace-python-{}", std::process::id()))
        })
        .clone(),
    )
}

#[cfg(not(test))]
fn state_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if !home.is_absolute() {
        return None;
    }
    Some(home.join("rustx").join("runtime").join("workspace-python"))
}

fn generation_of(root: &Path) -> Generation {
    let meta = std::fs::symlink_metadata(root).ok();
    Generation {
        root_dev: meta.as_ref().map_or(0, MetadataExt::dev),
        root_ino: meta.as_ref().map_or(0, MetadataExt::ino),
        pin_file: stamp(&root.join(".python-version")),
        project_file: stamp(&root.join("pyproject.toml")),
        venv: std::fs::symlink_metadata(root.join(".venv"))
            .ok()
            .as_ref()
            .map(file_stamp),
        config: stamp(&root.join(".venv/pyvenv.cfg")),
        python: stamp(&root.join(".venv/bin/python")),
        python3: stamp(&root.join(".venv/bin/python3")),
    }
}

fn stamp(path: &Path) -> Option<PathStamp> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let target = if meta.is_symlink() {
        std::fs::metadata(path).ok().as_ref().map(file_stamp)
    } else {
        None
    };
    Some(PathStamp {
        entry: file_stamp(&meta),
        target,
    })
}

fn file_stamp(meta: &std::fs::Metadata) -> FileStamp {
    FileStamp {
        dev: meta.dev(),
        ino: meta.ino(),
        mtime: meta.mtime(),
        mtime_nsec: meta.mtime_nsec(),
        len: meta.len(),
        mode: meta.mode(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Absent,
    Directory,
    Conflict(&'static str),
}

fn classify_target(target: &Path) -> Target {
    match std::fs::symlink_metadata(target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Target::Absent,
        Err(_) => Target::Conflict("Workspace .venv cannot be inspected"),
        Ok(meta) if meta.file_type().is_symlink() => {
            Target::Conflict("Workspace .venv is a symlink")
        }
        Ok(meta) if meta.is_dir() => Target::Directory,
        Ok(_) => Target::Conflict("Workspace .venv is not a directory"),
    }
}

enum Requires {
    Absent,
    Spec(String),
    Invalid,
}

fn read_pin(root: &Path) -> Result<Option<String>, &'static str> {
    let Some(text) = read_optional_file(&root.join(".python-version"))
        .map_err(|()| ".python-version cannot be read")?
    else {
        return Ok(None);
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // uv can write a bare relative directory name (without ./). Versions
        // keep their meaning even when a same-named directory exists.
        if !path_request(line) && parse_request(line).is_err() && root.join(line).is_dir() {
            // Preserve authored directory links for the fixed witness.
            return Ok(Some(root.join(line).display().to_string()));
        }
        return Ok(Some(line.to_owned()));
    }
    Ok(None)
}

fn read_optional_file(path: &Path) -> Result<Option<String>, ()> {
    if std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(None);
    }
    // Shared bounded reads open with O_NONBLOCK and validate the opened FD.
    let bytes = crate::bounded_file::read_bounded(path).map_err(|_| ())?;
    String::from_utf8(bytes).map(Some).map_err(|_| ())
}

fn read_requires(root: &Path) -> Requires {
    let text = match read_optional_file(&root.join("pyproject.toml")) {
        Ok(Some(text)) => text,
        Ok(None) => return Requires::Absent,
        Err(()) => return Requires::Invalid,
    };
    let Ok(document) = text.parse::<toml_edit::DocumentMut>() else {
        return Requires::Invalid;
    };
    let Some(project) = document.get("project") else {
        return Requires::Absent;
    };
    match project.get("requires-python") {
        None => Requires::Absent,
        Some(value) => value
            .as_str()
            .filter(|spec| VersionSpecifiers::from_str(spec).is_ok())
            .map_or(Requires::Invalid, |spec| Requires::Spec(spec.to_owned())),
    }
}

fn find_uv(host: &ExecutionHost, workspace: &Path) -> Option<PathBuf> {
    let path = host.entries().get("PATH")?;
    let venv_bin = workspace.join(".venv").join("bin");
    for directory in path.split(':') {
        if directory.is_empty() {
            continue;
        }
        let directory = PathBuf::from(directory);
        if directory == venv_bin {
            continue;
        }
        let candidate = directory.join("uv");
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}

fn isolated_discovery_args() -> [String; 3] {
    [
        "--no-project".to_owned(),
        "--directory".to_owned(),
        discovery_directory().display().to_string(),
    ]
}

fn discovery_directory() -> PathBuf {
    static DIRECTORY: Mutex<Option<PathBuf>> = Mutex::new(None);
    let mut cached = DIRECTORY.lock().expect("workspace python discovery");
    if let Some(directory) = fresh_discovery(&mut cached, &discovery_parents()) {
        return directory;
    }
    std::env::temp_dir().join(format!(
        "rustx-uv-discovery-{}-{}",
        std::process::id(),
        monotonic_nanos()
    ))
}

fn discovery_parents() -> Vec<PathBuf> {
    let mut parents = vec![std::env::temp_dir()];
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime);
        if path.is_absolute() && path.is_dir() {
            parents.push(path);
        }
    }
    let shm = PathBuf::from("/dev/shm");
    if shm.is_dir() {
        parents.push(shm);
    }
    parents
}

/// Keeps a discovery directory only after it exists. A failed create is not
/// remembered, and a deleted directory is replaced on the next use.
fn fresh_discovery(cached: &mut Option<PathBuf>, parents: &[PathBuf]) -> Option<PathBuf> {
    if let Some(directory) = cached.as_ref().filter(|directory| directory.is_dir()) {
        return Some(directory.clone());
    }
    let created = create_private_directory(parents, "rustx-uv-discovery")?;
    *cached = Some(created.clone());
    Some(created)
}

fn create_private_directory(parents: &[PathBuf], prefix: &str) -> Option<PathBuf> {
    for parent in parents {
        if !parent.is_dir() {
            continue;
        }
        for _ in 0..4 {
            let directory = parent.join(format!(
                "{prefix}-{}-{}",
                std::process::id(),
                monotonic_nanos()
            ));
            if claim_fresh_directory(&directory).is_some() {
                return Some(directory);
            }
        }
    }
    None
}

fn monotonic_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn uv_can_create(text: &str) -> bool {
    let Ok(version) = Version::from_str(text) else {
        return false;
    };
    let Ok(minimum) = Version::from_str(UV_MINIMUM) else {
        return false;
    };
    version >= minimum
}

/// Translate partial release requests to PEP 440 equality patterns and let uv
/// perform selection over the conjunction. This does not implement a solver.
fn creation_request(
    root: &Path,
    pin: Option<&str>,
    requires: Option<&str>,
) -> Result<String, &'static str> {
    let Some(pin) = pin else {
        return Ok(requires
            .filter(|spec| !spec.trim().is_empty())
            .unwrap_or("default")
            .to_owned());
    };
    if path_request(pin) {
        return Ok(absolute_pin(root, pin));
    }
    let Some(requires) = requires else {
        return Ok(pin.to_owned());
    };
    let parsed = parse_request(pin).map_err(|()| ".python-version cannot be verified")?;
    let (implementation, pinned): (_, VersionSpecifiers) = match parsed {
        ParsedRequest::Spec {
            implementation,
            spec,
        } => (
            implementation,
            VersionSpecifiers::from_str(&spec).map_err(|_| ".python-version cannot be verified")?,
        ),
        ParsedRequest::Release {
            implementation,
            version,
        } => {
            let specifier =
                if version.release().len() < 3 && version == Version::new(version.release()) {
                    VersionSpecifier::equals_star_version(version)
                } else {
                    VersionSpecifier::equals_version(version)
                };
            (implementation, std::iter::once(specifier).collect())
        }
    };
    let additional =
        VersionSpecifiers::from_str(requires).map_err(|_| "requires-python cannot be parsed")?;
    let combined: VersionSpecifiers = pinned.into_iter().chain(additional).collect();
    Ok(implementation.map_or_else(|| combined.to_string(), |name| format!("{name}@{combined}")))
}

fn exact_pin_conflicts(pin: &str, requires: &str) -> bool {
    let Ok(ParsedRequest::Release { version, .. }) = parse_request(pin) else {
        return false;
    };
    if version.release().len() < 3 {
        return false;
    }
    VersionSpecifiers::from_str(requires).is_ok_and(|specs| !specs.contains(&version))
}

fn path_request(pin: &str) -> bool {
    pin.contains('/') || pin.contains('\\')
}

/// A path request is bound to the Workspace root. uv's `--directory` must not
/// reinterpret `./pinned-python` against the private discovery directory.
fn absolute_pin(root: &Path, pin: &str) -> String {
    if !path_request(pin) {
        return pin.to_owned();
    }
    let requested = Path::new(pin);
    let absolute = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    std::fs::canonicalize(&absolute)
        .unwrap_or(absolute)
        .display()
        .to_string()
}

const IMPLEMENTATIONS: &[&str] = &["cpython", "pypy", "pyston", "graalpy", "graalpython"];

enum ParsedRequest {
    Spec {
        implementation: Option<String>,
        spec: String,
    },
    Release {
        implementation: Option<String>,
        version: Version,
    },
}

fn parse_request(pin: &str) -> Result<ParsedRequest, ()> {
    let (implementation, rest) = split_implementation(pin)?;
    if rest
        .chars()
        .any(|c| matches!(c, '>' | '<' | '=' | '~' | ','))
    {
        if VersionSpecifiers::from_str(rest).is_err() {
            return Err(());
        }
        return Ok(ParsedRequest::Spec {
            implementation,
            spec: rest.to_owned(),
        });
    }
    let version = Version::from_str(rest).map_err(|_| ())?;
    Ok(ParsedRequest::Release {
        implementation,
        version,
    })
}

fn split_implementation(pin: &str) -> Result<(Option<String>, &str), ()> {
    let mut best: Option<&str> = None;
    for name in IMPLEMENTATIONS {
        let Some(tail) = pin.get(name.len()..) else {
            continue;
        };
        if !pin[..name.len()].eq_ignore_ascii_case(name) {
            continue;
        }
        if !(tail.starts_with('-') || tail.starts_with('@')) {
            continue;
        }
        if best.is_none_or(|current| name.len() > current.len()) {
            best = Some(name);
        }
    }
    if let Some(name) = best {
        let tail = pin[name.len()..]
            .strip_prefix('-')
            .or_else(|| pin[name.len()..].strip_prefix('@'))
            .unwrap_or("");
        if tail.is_empty() {
            return Err(());
        }
        return Ok((Some((*name).to_owned()), tail));
    }
    if let Some(head_end) = pin.find(|character: char| {
        character.is_ascii_digit() || matches!(character, '>' | '<' | '=' | '~' | ',')
    }) {
        let head = pin[..head_end].trim_end_matches(['-', '@']);
        if !head.is_empty()
            && head
                .chars()
                .all(|character| character.is_ascii_alphabetic())
        {
            return Err(());
        }
    }
    Ok((None, pin))
}

fn request_satisfied(
    pin: &str,
    version: &Version,
    implementation: &str,
) -> Result<(), &'static str> {
    let request = parse_request(pin).map_err(|()| ".python-version cannot be verified")?;
    match request {
        ParsedRequest::Spec {
            implementation: required,
            spec,
        } => {
            if let Some(required) = required
                && !implementation.eq_ignore_ascii_case(&required)
            {
                return Err("Workspace Python does not satisfy .python-version");
            }
            let specs = VersionSpecifiers::from_str(&spec)
                .map_err(|_| ".python-version cannot be verified")?;
            if specs.contains(version) {
                Ok(())
            } else {
                Err("Workspace Python does not satisfy .python-version")
            }
        }
        ParsedRequest::Release {
            implementation: required,
            version: requested,
        } => {
            if let Some(required) = required {
                if implementation.is_empty() {
                    return Err("the Python implementation could not be verified");
                }
                if !implementation.eq_ignore_ascii_case(&required) {
                    return Err("Workspace Python does not satisfy .python-version");
                }
            }
            let matches = if requested.release().len() >= 3
                || requested != Version::new(requested.release())
            {
                requested == *version
            } else {
                version.release().starts_with(requested.release())
            };
            if matches {
                Ok(())
            } else {
                Err("Workspace Python does not satisfy .python-version")
            }
        }
    }
}

#[cfg(test)]
fn pin_matches(pin: &str, actual: &Version) -> bool {
    request_satisfied(pin, actual, "cpython").is_ok()
}

fn bounded_diagnostic(summary: &str, stderr: &str) -> String {
    let detail = shell_line(stderr);
    if detail.is_empty() {
        summary.to_owned()
    } else {
        format!("{summary}: {detail}")
    }
}

fn guarded(diagnostic: &str) -> PythonSelection {
    selection_from_guard(ensure_guard(diagnostic), diagnostic)
}

fn selection_from_guard(specific: Option<PathBuf>, diagnostic: &str) -> PythonSelection {
    finish_guard(specific.or_else(fallback_guard), diagnostic)
}

fn finish_guard(created: Option<PathBuf>, diagnostic: &str) -> PythonSelection {
    directory_guard(created.or_else(provisioned_guard), diagnostic)
}

fn directory_guard(directory: Option<PathBuf>, diagnostic: &str) -> PythonSelection {
    match directory {
        Some(directory) => PythonSelection {
            path_prefix: vec![directory],
            virtual_env: None,
            diagnostic: diagnostic.to_owned(),
            guard_unavailable: false,
        },
        None => PythonSelection {
            path_prefix: Vec::new(),
            virtual_env: None,
            diagnostic: diagnostic.to_owned(),
            guard_unavailable: true,
        },
    }
}

const GUARD_PYTHON: &[u8] = include_bytes!("../../assets/python-guard/python");
const GUARD_PYTHON3: &[u8] = include_bytes!("../../assets/python-guard/python3");

/// `python-guard` installed beside the running executable. The source checkout
/// is not consulted: a relocated binary keeps this directory next to itself,
/// and both scripts must match the bytes embedded in that binary.
fn provisioned_guard() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    if let Some(directory) = guard_beside_executable(&executable) {
        return Some(directory);
    }
    let canonical = std::fs::canonicalize(&executable).ok()?;
    if canonical == executable {
        return None;
    }
    guard_beside_executable(&canonical)
}

fn guard_beside_executable(executable: &Path) -> Option<PathBuf> {
    validated_guard_directory(&executable.parent()?.join("python-guard"))
}

fn validated_guard_directory(directory: &Path) -> Option<PathBuf> {
    for (name, expected) in [("python", GUARD_PYTHON), ("python3", GUARD_PYTHON3)] {
        let path = directory.join(name);
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
            return None;
        }
        if read_optional_file(&path).ok()??.as_bytes() != expected {
            return None;
        }
    }
    Some(directory.to_path_buf())
}

pub(crate) const GUARD_MESSAGE_KEY: &str = "RUSTX_PYTHON_GUARD_MESSAGE";

fn fallback_guard() -> Option<PathBuf> {
    static GUARD: Mutex<Option<PathBuf>> = Mutex::new(None);
    cached_guard(
        &mut GUARD.lock().expect("workspace python guard"),
        &fallback_parents(),
    )
}

fn cached_guard(cached: &mut Option<PathBuf>, parents: &[PathBuf]) -> Option<PathBuf> {
    if let Some(valid) = cached.as_deref().and_then(validated_guard_directory) {
        return Some(valid);
    }
    *cached = install_process_guard(parents, "Workspace Python is not ready");
    cached.clone()
}

fn fallback_parents() -> Vec<PathBuf> {
    let mut parents = vec![std::env::temp_dir()];
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime);
        if path.is_absolute() {
            parents.push(path);
        }
    }
    let shm = PathBuf::from("/dev/shm");
    if shm.is_dir() {
        parents.push(shm);
    }
    if let Some(parent) = std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(Path::parent)
    {
        parents.push(parent.to_path_buf());
    }
    parents
}

fn install_process_guard(parents: &[PathBuf], diagnostic: &str) -> Option<PathBuf> {
    for parent in parents {
        if !parent.is_dir() {
            continue;
        }
        for _ in 0..4 {
            let nanos = monotonic_nanos();
            let directory =
                parent.join(format!("rustx-python-guard-{}-{nanos}", std::process::id()));
            if claim_fresh_directory(&directory).is_none() {
                continue;
            }
            if let Some(directory) = install_guard(&directory, diagnostic) {
                return Some(directory);
            }
        }
    }
    None
}

fn claim_fresh_directory(directory: &Path) -> Option<PathBuf> {
    std::fs::create_dir(directory).ok()?;
    let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700));
    Some(directory.to_path_buf())
}

fn ensure_guard(diagnostic: &str) -> Option<PathBuf> {
    let directory = state_dir()?.join("guards").join(guard_name(diagnostic));
    install_guard(&directory, diagnostic)
}

fn install_guard(directory: &Path, diagnostic: &str) -> Option<PathBuf> {
    std::fs::create_dir_all(directory).ok()?;
    let _ = diagnostic; // The immutable script reads the per-command diagnostic.
    for name in ["python", "python3"] {
        let path = directory.join(name);
        if path.exists() {
            continue;
        }
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        };
        file.write_all(GUARD_PYTHON).ok()?;
    }
    validated_guard_directory(directory)
}

fn guard_name(diagnostic: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(diagnostic.as_bytes());
    format!("{:x}", hasher.finalize())[..32].to_owned()
}

fn shell_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " .,:-_".contains(c) {
                c
            } else {
                ' '
            }
        })
        .take(180)
        .collect()
}

fn lock_file(state: &Path, root: &Path) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(root.as_os_str().as_bytes());
    state.join("locks").join(format!("{:x}", hasher.finalize()))
}

async fn acquire_lock(
    root: &Path,
    before_lock: Option<tokio::sync::oneshot::Sender<()>>,
    state_override: Option<PathBuf>,
    shutdown: crate::runtime::cancellation::CancellationSignal,
) -> Result<nix::fcntl::Flock<std::fs::File>, String> {
    let state = state_override
        .or_else(state_dir)
        .ok_or_else(|| "HOME is not available for Workspace Python coordination".to_owned())?;
    let directory = state.join("locks");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = lock_file(&state, root);
    let file = tokio::task::spawn_blocking(move || {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;
    if let Some(sender) = before_lock {
        let _ = sender.send(());
    }
    let mut file = Some(file);
    let deadline = Instant::now() + CREATE_TIMEOUT;
    loop {
        if shutdown.is_cancelled() {
            return Err("Workspace Python preparation stopped".to_owned());
        }
        if Instant::now() >= deadline {
            return Err("the Workspace Python lock timed out".to_owned());
        }
        let current = file.take().expect("lock file");
        match nix::fcntl::Flock::lock(current, nix::fcntl::FlockArg::LockExclusiveNonblock) {
            Ok(lock) => return Ok(lock),
            Err((current, errno))
                if errno == nix::errno::Errno::EWOULDBLOCK
                    || errno == nix::errno::Errno::EAGAIN =>
            {
                file = Some(current);
                tokio::select! {
                    () = shutdown.cancelled() => {
                        return Err("Workspace Python preparation stopped".to_owned());
                    }
                    () = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            }
            Err((_, errno)) => return Err(errno.to_string()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{
        Owner, ParsedRequest, ProgramOutput, ProgramProbe, ProgramRequest, PythonSelection, Record,
        ensure_guard, exact_pin_conflicts, generation_of, lock_file, parse_request, path_request,
        pin_matches, scan_admit, uv_can_create,
    };
    use crate::tools::execution_environment::{ExecutionHost, host_from_entries};
    use futures_util::future::BoxFuture;
    use pep440_rs::Version;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    struct HoldProbe {
        release: tokio::sync::watch::Receiver<bool>,
    }

    impl ProgramProbe for HoldProbe {
        fn run(&self, _request: ProgramRequest) -> BoxFuture<'_, Result<ProgramOutput, String>> {
            let mut release = self.release.clone();
            Box::pin(async move {
                loop {
                    if *release.borrow() {
                        break;
                    }
                    if release.changed().await.is_err() {
                        break;
                    }
                }
                Err("preparation stopped".to_owned())
            })
        }
    }

    #[derive(Default)]
    struct RecordingSupervisedProbe {
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl ProgramProbe for RecordingSupervisedProbe {
        fn run(&self, request: ProgramRequest) -> BoxFuture<'_, Result<ProgramOutput, String>> {
            self.calls.lock().unwrap().push(request.argv.clone());
            Box::pin(async move { super::SupervisedProbe.run(request).await })
        }
    }

    struct Scripted {
        calls: Mutex<Vec<Vec<String>>>,
        version: String,
        implementation: String,
        envs: Mutex<Vec<Vec<(String, String)>>>,
        entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    impl Scripted {
        fn new(version: &str) -> Self {
            Self::with_implementation(version, "cpython")
        }

        fn with_implementation(version: &str, implementation: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                version: version.to_owned(),
                implementation: implementation.to_owned(),
                envs: Mutex::new(Vec::new()),
                entered: Mutex::new(None),
                release: Mutex::new(None),
            }
        }
    }

    impl ProgramProbe for Scripted {
        fn run(
            &self,
            request: ProgramRequest,
        ) -> futures_util::future::BoxFuture<'_, Result<ProgramOutput, String>> {
            Box::pin(async move {
                let _ = (request.timeout, request.cancellation.clone());
                self.envs.lock().expect("envs").push(request.env.clone());
                self.calls.lock().expect("calls").push(request.argv.clone());
                let entered = self.entered.lock().expect("entered").take();
                if let Some(entered) = entered {
                    let _ = entered.send(());
                    let release = self.release.lock().expect("release").take();
                    if let Some(release) = release {
                        let _ = release.await;
                    }
                }
                let argv = &request.argv;
                if argv.iter().any(|arg| arg == "--version") {
                    return Ok(ProgramOutput {
                        status: 0,
                        stdout: format!("uv {}\n", self.version),
                        stderr: String::new(),
                    });
                }
                if argv.iter().any(|arg| arg == "venv") {
                    let target = PathBuf::from(argv.last().expect("target"));
                    let interpreter = argv
                        .iter()
                        .position(|arg| arg == "--python")
                        .map(|index| PathBuf::from(&argv[index + 1]));
                    if let Some(interpreter) = interpreter.filter(|p| p.is_absolute()) {
                        write_linked_venv(&target, &interpreter);
                    } else {
                        write_venv(&target);
                    }
                    return Ok(ProgramOutput {
                        status: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                    });
                }
                if argv.iter().any(|arg| arg == "-I") {
                    let program = Path::new(&argv[0]);
                    if let Some(venv) = program
                        .parent()
                        .and_then(|bin| bin.parent())
                        .filter(|dir| dir.join("pyvenv.cfg").exists())
                    {
                        return Ok(ProgramOutput {
                            status: 0,
                            stdout: format!(
                                "{}\n/usr\n3.12.3\n{}\n{}\n",
                                venv.display(),
                                self.implementation,
                                std::fs::canonicalize(program).unwrap().display()
                            ),
                            stderr: String::new(),
                        });
                    }
                    return Ok(ProgramOutput {
                        status: 0,
                        stdout: format!(
                            "/usr\n/usr\n3.12.3\n{}\n{}\n",
                            self.implementation,
                            std::fs::canonicalize(program).unwrap().display()
                        ),
                        stderr: String::new(),
                    });
                }
                Ok(ProgramOutput {
                    status: 1,
                    stdout: String::new(),
                    stderr: "unexpected program".to_owned(),
                })
            })
        }
    }

    fn write_executable(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, body).expect("script");
        let mut permissions = std::fs::metadata(path).expect("meta").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("mode");
    }

    fn write_venv(target: &Path) {
        for name in ["python", "python3"] {
            write_executable(&target.join("bin").join(name), "#!/bin/sh\nexit 0\n");
        }
        std::fs::write(target.join("pyvenv.cfg"), "home = /usr\n").expect("pyvenv");
    }

    fn write_linked_venv(target: &Path, interpreter: &Path) {
        std::fs::create_dir_all(target.join("bin")).expect("bin");
        for name in ["python", "python3"] {
            std::os::unix::fs::symlink(interpreter, target.join("bin").join(name)).expect("link");
        }
        std::fs::write(target.join("pyvenv.cfg"), "home = /usr\n").expect("pyvenv");
    }

    fn retained_fixed_interpreter(owner: &Owner, root: &Path) -> Arc<super::FixedInterpreter> {
        match owner.records.lock().unwrap().get(root) {
            Some(Record::Ready {
                fixed: Some(fixed), ..
            }) => fixed.clone(),
            other => panic!("expected a retained directory witness, got {other:?}"),
        }
    }

    fn workspace() -> tempfile::TempDir {
        // Owner records are keyed by physical roots. macOS's default TMPDIR
        // may spell the same directory through the /var -> /private/var alias.
        let parent = std::fs::canonicalize(std::env::temp_dir()).expect("temporary root");
        tempfile::tempdir_in(parent).expect("workspace")
    }

    fn host_for(bin: &Path) -> Arc<ExecutionHost> {
        Arc::new(host_from_entries(vec![
            ("PATH", &bin.display().to_string()),
            ("HOME", "/users/ada"),
            ("UV_VENV_CLEAR", "1"),
            ("UV_PROJECT_ENVIRONMENT", "/elsewhere"),
            ("UV_OFFLINE", "1"),
        ]))
    }

    #[test]
    fn u08_creation_requires_uv_0_10_0() {
        assert!(uv_can_create("0.10.0"));
        assert!(uv_can_create("0.11.2"));
        assert!(!uv_can_create("0.9.26"));
        assert!(!uv_can_create("not-a-version"));
    }

    #[test]
    fn u10_pin_and_requires_python_use_pep440() {
        let actual = Version::from_str("3.12.3").expect("version");
        assert!(pin_matches("3.12", &actual));
        assert!(pin_matches("3.12.3", &actual));
        assert!(pin_matches("cpython-3.12.3", &actual));
        assert!(!pin_matches("pypy-3.12", &actual));
        assert!(!pin_matches("cpython-3.12.3-linux", &actual));
        assert!(!pin_matches("jython-3.12", &actual));
        assert!(!pin_matches("3.11", &actual));
        assert!(!pin_matches("3.12.4", &actual));
        assert!(!pin_matches("3.12.3rc1", &actual));
        assert!(!pin_matches("3.12.3.post1", &actual));
        let prerelease = Version::from_str("3.12.3rc1").expect("prerelease");
        assert!(pin_matches("3.12.3rc1", &prerelease));
        assert!(!pin_matches("3.12.3", &prerelease));
        assert!(exact_pin_conflicts("3.11.0", ">=3.12"));
        assert!(!exact_pin_conflicts("3.12", ">=3.12.1"));
        assert!(!exact_pin_conflicts(
            "/opt/python-3.11.0/bin/python",
            ">=3.12"
        ));
        assert!(path_request("/opt/python-3.11.0/bin/python"));
    }

    #[tokio::test]
    async fn u05_valid_venv_is_reused_without_uv() {
        let root = workspace();
        write_venv(&root.path().join(".venv"));
        let probe = Arc::new(Scripted::new("0.9.0"));
        let owner = Owner::for_test(probe.clone(), None, None, None);
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        owner.prepare(root_path.clone()).await;
        let calls = probe.calls.lock().expect("calls");
        assert!(
            calls
                .iter()
                .all(|argv| !argv.iter().any(|arg| arg == "venv"))
        );
        assert!(calls.iter().any(|argv| argv.iter().any(|arg| arg == "-I")));
        match owner.records.lock().expect("records").get(&root_path) {
            Some(Record::Ready { .. }) => {}
            other => panic!("expected ready, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn u06_missing_environment_is_created_once_with_explicit_uv_argv() {
        let root = workspace();
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let bin = tempfile::tempdir().expect("bin");
        write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Owner::for_test(probe.clone(), Some(host_for(bin.path())), None, None);
        owner.prepare(root_path.clone()).await;
        let calls = probe.calls.lock().expect("calls");
        let venv: Vec<_> = calls
            .iter()
            .filter(|argv| argv.iter().any(|arg| arg == "venv"))
            .collect();
        assert_eq!(venv.len(), 1);
        let argv = &venv[0];
        assert!(argv.iter().any(|arg| arg == "--no-project"));
        assert!(argv.iter().any(|arg| arg == "--directory"));
        assert!(argv.iter().all(|arg| arg != "--no-config"));
        assert!(!argv.iter().any(|arg| arg == "find"));
        assert!(
            !argv
                .iter()
                .any(|arg| arg == "--clear" || arg == "--seed" || arg == "--allow-existing")
        );
        assert_eq!(
            argv.last().map(String::as_str),
            Some(root_path.join(".venv").display().to_string().as_str())
        );
        let envs = probe.envs.lock().expect("envs");
        assert!(envs.iter().all(|env| {
            env.iter()
                .all(|(key, _)| key != "UV_VENV_CLEAR" && key != "UV_PROJECT_ENVIRONMENT")
        }));
        assert!(envs.iter().any(|env| {
            env.iter()
                .any(|(key, value)| key == "UV_OFFLINE" && value == "1")
        }));
        drop(calls);
        match owner.records.lock().expect("records").get(&root_path) {
            Some(Record::Ready { .. }) => {}
            other => panic!("expected ready, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn u07_missing_uv_does_not_create_or_fall_back() {
        let root = workspace();
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let bin = tempfile::tempdir().expect("bin");
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Owner::for_test(probe.clone(), Some(host_for(bin.path())), None, None);
        owner.prepare(root_path.clone()).await;
        assert!(probe.calls.lock().expect("calls").is_empty());
        assert!(!root_path.join(".venv").exists());
        match owner.records.lock().expect("records").get(&root_path) {
            Some(Record::Unavailable { diagnostic, .. }) => {
                assert!(diagnostic.contains("uv was not found"));
            }
            other => panic!("expected unavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn u09_symlink_and_file_targets_are_conflicts() {
        let root = workspace();
        std::os::unix::fs::symlink("/tmp", root.path().join(".venv")).expect("symlink");
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let owner = Owner::for_test(Arc::new(Scripted::new("0.10.2")), None, None, None);
        owner.prepare(root_path.clone()).await;
        match owner.records.lock().expect("records").get(&root_path) {
            Some(Record::Conflict { diagnostic, .. }) => {
                assert!(diagnostic.contains("symlink"));
            }
            other => panic!("expected conflict, got {other:?}"),
        }
        assert!(
            root.path().join(".venv").is_symlink()
                || std::fs::symlink_metadata(root.path().join(".venv"))
                    .expect("meta")
                    .file_type()
                    .is_symlink()
        );
    }

    #[tokio::test]
    async fn u21_unchanged_ready_observation_does_not_probe_again() {
        let root = workspace();
        write_venv(&root.path().join(".venv"));
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Owner::for_test(probe.clone(), None, None, None);
        owner.prepare(root_path.clone()).await;
        let before = probe.calls.lock().expect("calls").len();
        let selection = owner.observe(&root_path);
        let after = probe.calls.lock().expect("calls").len();
        assert_eq!(before, after);
        assert!(selection.virtual_env.is_some());
        assert!(selection.path_prefix[0].ends_with("bin"));
        let _ = generation_of(&root_path);
    }

    #[tokio::test]
    async fn ready_cache_eviction_revalidates_without_rebuilding_environment() {
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Owner::for_test(probe.clone(), None, None, None);
        let mut roots = Vec::new();
        for _ in 0..129 {
            let root = workspace();
            write_venv(&root.path().join(".venv"));
            std::fs::write(root.path().join(".venv/keep"), "installed dependency")
                .expect("sentinel");
            let root_path = std::fs::canonicalize(root.path()).expect("canonical");
            owner.prepare(root_path.clone()).await;
            roots.push((root, root_path));
        }
        let evicted = {
            let records = owner.records.lock().expect("records");
            assert_eq!(records.len(), 128);
            roots
                .iter()
                .find(|(_, path)| !records.contains_key(path))
                .expect("one readiness record was evicted")
                .1
                .clone()
        };
        assert!(owner.observe(&evicted).virtual_env.is_none());
        let before = probe.calls.lock().expect("calls").len();
        owner.prepare(evicted.clone()).await;
        assert!(owner.observe(&evicted).virtual_env.is_some());
        let calls = probe.calls.lock().expect("calls");
        assert_eq!(calls[before..].len(), 2);
        assert!(
            calls[before..]
                .iter()
                .all(|argv| argv.iter().any(|arg| arg == "-I"))
        );
        assert_eq!(
            std::fs::read_to_string(evicted.join(".venv/keep")).expect("preserved sentinel"),
            "installed dependency"
        );
        assert_eq!(owner.records.lock().expect("records").len(), 128);
    }

    #[tokio::test]
    async fn u11_unready_selection_is_a_guard_and_does_not_wait() {
        let root = workspace();
        let owner = Owner::for_test(Arc::new(Scripted::new("0.10.2")), None, None, None);
        let selection = owner.observe(root.path());
        assert!(selection.virtual_env.is_none());
        assert!(!selection.path_prefix.is_empty());
        let script = selection.path_prefix[0].join("python");
        let body = std::fs::read_to_string(&script).expect("guard");
        assert!(body.contains("exit 127"));
        assert!(!body.contains("UV_") && !body.contains("/users/"));
        let _ = ensure_guard("Workspace Python is not ready");
    }

    #[tokio::test]
    async fn u16_second_process_holds_the_workspace_lock() {
        const NAME: &str =
            "tools::workspace_python::tests::u16_second_process_holds_the_workspace_lock";
        if std::env::var_os("RUSTX_WORKSPACE_PYTHON_LOCK_HOLDER").is_some() {
            use std::io::Write;
            let path = std::env::var("RUSTX_WORKSPACE_PYTHON_LOCK_PATH").expect("lock path");
            let ready = std::env::var("RUSTX_WORKSPACE_PYTHON_LOCK_READY").expect("ready fifo");
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .expect("lock file");
            let _lock =
                nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive).expect("lock");
            let mut signal = std::fs::OpenOptions::new()
                .write(true)
                .open(ready)
                .expect("ready fifo");
            signal.write_all(b"1").expect("signal");
            let mut input = String::new();
            let _ = std::io::stdin().read_line(&mut input);
            return;
        }
        let state = tempfile::tempdir().expect("state");
        let root = workspace();
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let bin = tempfile::tempdir().expect("bin");
        write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
        let lock_path = lock_file(state.path(), &root_path);
        std::fs::create_dir_all(lock_path.parent().expect("lock dir")).expect("locks");
        let ready = state.path().join("ready");
        nix::unistd::mkfifo(
            &ready,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .expect("ready fifo");
        let mut child = std::process::Command::new(std::env::current_exe().expect("test exe"))
            .args(["--exact", NAME, "--test-threads=1", "--nocapture"])
            .env("RUSTX_WORKSPACE_PYTHON_LOCK_HOLDER", "1")
            .env("RUSTX_WORKSPACE_PYTHON_LOCK_PATH", &lock_path)
            .env("RUSTX_WORKSPACE_PYTHON_LOCK_READY", &ready)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("holder");
        let ready_read = ready.clone();
        tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut fifo = std::fs::File::open(ready_read).expect("open ready fifo");
            let mut byte = [0u8; 1];
            fifo.read_exact(&mut byte).expect("holder locked");
        })
        .await
        .expect("holder task");
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Arc::new(Owner::for_test(
            probe.clone(),
            Some(host_for(bin.path())),
            Some(state.path().to_path_buf()),
            Some(sender),
        ));
        let preparing = Arc::clone(&owner);
        let prepare_root = root_path.clone();
        let prepare = tokio::spawn(async move { preparing.prepare(prepare_root).await });
        receiver.await.expect("preparation reached the lock");
        assert!(probe.calls.lock().expect("calls").is_empty());
        drop(child.stdin.take());
        let status = tokio::task::spawn_blocking(move || child.wait().expect("holder exit"))
            .await
            .expect("wait");
        assert!(status.success());
        prepare.await.expect("prepare");
        assert_eq!(
            probe
                .calls
                .lock()
                .expect("calls")
                .iter()
                .filter(|argv| argv.iter().any(|arg| arg == "venv"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn issue_u03_admit_scan_removes_the_file_without_waiting() {
        let root = workspace();
        let directory = tempfile::tempdir().expect("admit");
        let file = directory.path().join("admission.txt");
        std::fs::write(&file, format!("{}\n", root.path().display())).expect("admit file");
        scan_admit(directory.path());
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn issue_u05_old_uv_does_not_create_and_reuses_a_valid_environment() {
        let missing = workspace();
        let missing_root = std::fs::canonicalize(missing.path()).expect("canonical");
        let bin = tempfile::tempdir().expect("bin");
        write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
        let probe = Arc::new(Scripted::new("0.9.26"));
        let owner = Owner::for_test(probe.clone(), Some(host_for(bin.path())), None, None);
        owner.prepare(missing_root.clone()).await;
        assert!(
            probe
                .calls
                .lock()
                .expect("calls")
                .iter()
                .all(|argv| !argv.iter().any(|arg| arg == "venv"))
        );
        assert!(!missing_root.join(".venv").exists());
        match owner.records.lock().expect("records").get(&missing_root) {
            Some(Record::Unavailable { diagnostic, .. }) => {
                assert!(diagnostic.contains("0.10.0"));
            }
            other => panic!("expected unavailable, got {other:?}"),
        }

        let existing = workspace();
        write_venv(&existing.path().join(".venv"));
        let sentinel = existing.path().join(".venv/sentinel.txt");
        std::fs::write(&sentinel, "keep").expect("sentinel");
        let existing_root = std::fs::canonicalize(existing.path()).expect("canonical");
        let reuse = Arc::new(Scripted::new("0.9.26"));
        let owner = Owner::for_test(reuse.clone(), Some(host_for(bin.path())), None, None);
        owner.prepare(existing_root.clone()).await;
        assert!(
            reuse
                .calls
                .lock()
                .expect("calls")
                .iter()
                .all(|argv| argv.iter().any(|arg| arg == "-I"))
        );
        assert_eq!(std::fs::read_to_string(sentinel).expect("sentinel"), "keep");
        match owner.records.lock().expect("records").get(&existing_root) {
            Some(Record::Ready { .. }) => {}
            other => panic!("expected ready, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn issue_u07_unpinned_requires_python_is_the_create_request() {
        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("workspace");
        std::fs::create_dir(&root).expect("workspace");
        std::fs::write(parent.path().join(".python-version"), "3.11\n").expect("ancestor pin");
        std::fs::write(
            root.join("pyproject.toml"),
            "[project]\nrequires-python = \">=3.12\"\n",
        )
        .expect("project");
        let root_path = std::fs::canonicalize(&root).expect("canonical");
        let bin = tempfile::tempdir().expect("bin");
        write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Owner::for_test(probe.clone(), Some(host_for(bin.path())), None, None);
        owner.prepare(root_path).await;
        let calls = probe.calls.lock().expect("calls");
        let find = calls
            .iter()
            .find(|argv| argv.iter().any(|arg| arg == "venv"))
            .expect("venv");
        assert!(find.iter().any(|arg| arg == "--no-project"));
        assert!(find.iter().any(|arg| arg == "--directory"));
        assert!(find.iter().all(|arg| arg != "--no-config"));
        assert!(find.iter().any(|arg| arg == ">=3.12"));
        assert!(find.iter().all(|arg| arg != "3.11"));
    }

    #[tokio::test]
    async fn issue_u08_invalid_version_input_is_a_conflict_without_create() {
        let contradictory = workspace();
        std::fs::write(contradictory.path().join(".python-version"), "3.11.0\n").expect("pin");
        std::fs::write(
            contradictory.path().join("pyproject.toml"),
            "[project]\nrequires-python = \">=3.12\"\n",
        )
        .expect("project");
        let root_path = std::fs::canonicalize(contradictory.path()).expect("canonical");
        let bin = tempfile::tempdir().expect("bin");
        write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Owner::for_test(probe.clone(), Some(host_for(bin.path())), None, None);
        owner.prepare(root_path.clone()).await;
        assert!(probe.calls.lock().expect("calls").is_empty());
        assert!(!root_path.join(".venv").exists());
        match owner.records.lock().expect("records").get(&root_path) {
            Some(Record::Conflict { diagnostic, .. }) => {
                assert!(diagnostic.contains("conflicts"));
            }
            other => panic!("expected conflict, got {other:?}"),
        }

        let malformed = workspace();
        std::fs::write(malformed.path().join("pyproject.toml"), "project = [\n").expect("toml");
        let root_path = std::fs::canonicalize(malformed.path()).expect("canonical");
        let owner = Owner::for_test(
            Arc::new(Scripted::new("0.10.2")),
            Some(host_for(bin.path())),
            None,
            None,
        );
        owner.prepare(root_path.clone()).await;
        match owner.records.lock().expect("records").get(&root_path) {
            Some(Record::Conflict { diagnostic, .. }) => {
                assert!(diagnostic.contains("requires-python"));
            }
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn issue_u09_file_target_is_preserved() {
        let root = workspace();
        std::fs::write(root.path().join(".venv"), "keep-me").expect("file");
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let owner = Owner::for_test(Arc::new(Scripted::new("0.10.2")), None, None, None);
        owner.prepare(root_path.clone()).await;
        assert_eq!(
            std::fs::read_to_string(root.path().join(".venv")).expect("file"),
            "keep-me"
        );
        match owner.records.lock().expect("records").get(&root_path) {
            Some(Record::Conflict { diagnostic, .. }) => {
                assert!(diagnostic.contains("not a directory"));
            }
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn issue_u10_path_pin_binds_the_interpreter_and_final_target() {
        let root = workspace();
        let pinned = root.path().join("pinned-python");
        write_executable(&pinned, "#!/bin/sh\nexit 0\n");
        let pinned = std::fs::canonicalize(&pinned).expect("pin");
        std::fs::write(
            root.path().join(".python-version"),
            format!("{}\n", pinned.display()),
        )
        .expect("pin file");
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let bin = tempfile::tempdir().expect("bin");
        write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Owner::for_test(probe.clone(), Some(host_for(bin.path())), None, None);
        owner.prepare(root_path.clone()).await;
        let calls = probe.calls.lock().expect("calls");
        let find = calls
            .iter()
            .find(|argv| argv.iter().any(|arg| arg == "venv"))
            .expect("venv");
        assert_eq!(
            find.iter()
                .position(|arg| arg == "--python")
                .map(|i| find[i + 1].as_str()),
            Some(pinned.display().to_string().as_str())
        );
        let venv = calls
            .iter()
            .find(|argv| argv.iter().any(|arg| arg == "venv"))
            .expect("venv");
        assert_eq!(
            venv.last().map(String::as_str),
            Some(root_path.join(".venv").display().to_string().as_str())
        );
        assert_eq!(
            std::fs::canonicalize(root_path.join(".venv/bin/python")).expect("interpreter"),
            pinned
        );
        match owner.records.lock().expect("records").get(&root_path) {
            Some(Record::Ready { .. }) => {}
            other => panic!("expected ready, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn adopting_a_path_pin_rejects_a_different_base_executable_with_the_same_version() {
        let root = workspace();
        let pinned = root.path().join("pinned-python");
        let other = root.path().join("other-python");
        write_executable(&pinned, "#!/bin/sh\nexit 0\n");
        write_executable(&other, "#!/bin/sh\nexit 0\n");
        write_linked_venv(&root.path().join(".venv"), &other);
        std::fs::write(root.path().join(".python-version"), "./pinned-python").unwrap();
        let original = generation_of(root.path());
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Owner::for_test(probe.clone(), None, None, None);
        owner.prepare(root.path().to_path_buf()).await;
        assert!(matches!(
            owner.records.lock().unwrap().get(root.path()),
            Some(Record::Unavailable { diagnostic, .. }) if diagnostic.contains(".python-version")
        ));
        assert_eq!(generation_of(root.path()), original);
        assert!(
            probe
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|argv| argv.iter().any(|arg| arg == "-I"))
        );
        owner.stop().await;
    }

    #[tokio::test]
    async fn ready_path_pin_alias_change_invalidates_observation_without_new_probes() {
        let root = workspace();
        let base = root.path().join("base-python");
        let other = root.path().join("other-python");
        let alias = root.path().join("pinned-python");
        write_executable(&base, "#!/bin/sh\nexit 0\n");
        write_executable(&other, "#!/bin/sh\nexit 0\n");
        std::os::unix::fs::symlink(&base, &alias).unwrap();
        write_linked_venv(&root.path().join(".venv"), &base);
        std::fs::write(root.path().join(".python-version"), "./pinned-python").unwrap();
        let original = generation_of(root.path());
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Owner::for_test(probe.clone(), None, None, None);
        owner.prepare(root.path().to_path_buf()).await;
        assert!(owner.observe(root.path()).virtual_env.is_some());
        let calls = probe.calls.lock().unwrap().len();
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&other, &alias).unwrap();
        assert_eq!(generation_of(root.path()), original);
        let (miss, mut arrival) = tokio::sync::oneshot::channel();
        *owner.cache_miss.lock().unwrap() = Some(miss);
        assert!(owner.observe(root.path()).virtual_env.is_none());
        arrival
            .try_recv()
            .expect("the changed path pin invalidates Ready");
        assert_eq!(
            probe.calls.lock().unwrap().len(),
            calls,
            "observation only inspects metadata"
        );
        owner.prepare(root.path().to_path_buf()).await;
        assert!(matches!(
            owner.records.lock().unwrap().get(root.path()),
            Some(Record::Unavailable { .. })
        ));
        assert_eq!(
            generation_of(root.path()),
            original,
            "revalidation never rebuilds"
        );
        owner.stop().await;
    }

    #[tokio::test]
    async fn bare_pin_parsing_keeps_versions_and_rejects_non_directory_requests_without_probes() {
        for pin in ["named-python", "cpython-3.12-unsupported"] {
            let root = workspace();
            write_executable(&root.path().join(pin), "#!/bin/sh\nexit 0\n");
            std::fs::write(root.path().join(".python-version"), pin).unwrap();
            let probe = Arc::new(Scripted::new("0.10.2"));
            let owner = Owner::for_test(probe.clone(), None, None, None);
            owner.prepare(root.path().to_path_buf()).await;
            assert!(matches!(
                owner.records.lock().unwrap().get(root.path()),
                Some(Record::Conflict { .. })
            ));
            assert!(probe.calls.lock().unwrap().is_empty());
            owner.stop().await;
        }
        let root = workspace();
        std::fs::create_dir(root.path().join("3.12")).unwrap();
        std::fs::write(root.path().join(".python-version"), "3.12").unwrap();
        let pin = super::read_pin(root.path()).unwrap();
        assert_eq!(pin.as_deref(), Some("3.12"));
        assert_eq!(
            super::creation_request(root.path(), pin.as_deref(), None).unwrap(),
            "3.12"
        );
    }

    #[tokio::test]
    async fn bare_directory_pin_recovers_after_disappearing_without_root_input_changes() {
        let root = workspace();
        let source = root.path().join("source-env");
        let base = root.path().join("base-python");
        write_executable(&base, "#!/bin/sh\nexit 0\n");
        std::fs::create_dir_all(source.join("bin")).unwrap();
        std::os::unix::fs::symlink(&base, source.join("bin/python")).unwrap();
        write_linked_venv(&root.path().join(".venv"), &base);
        std::fs::write(root.path().join(".python-version"), "source-env").unwrap();
        let original = generation_of(root.path());
        let owner = Owner::for_test(Arc::new(Scripted::new("0.10.2")), None, None, None);
        owner.prepare(root.path().to_path_buf()).await;
        assert!(owner.observe(root.path()).virtual_env.is_some());
        let saved = root.path().join("saved-env");
        std::fs::rename(&source, &saved).unwrap();
        assert!(owner.observe(root.path()).virtual_env.is_none());
        owner.prepare(root.path().to_path_buf()).await;
        assert!(matches!(
            owner.records.lock().unwrap().get(root.path()),
            Some(Record::Conflict { .. })
        ));
        let (miss, mut arrival) = tokio::sync::oneshot::channel();
        *owner.cache_miss.lock().unwrap() = Some(miss);
        assert!(owner.observe(root.path()).virtual_env.is_none());
        assert!(matches!(
            arrival.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        std::fs::rename(saved, source).unwrap();
        owner.observe(root.path());
        arrival
            .try_recv()
            .expect("the directory's return invalidates the malformed-request cache");
        owner.prepare(root.path().to_path_buf()).await;
        assert!(owner.observe(root.path()).virtual_env.is_some());
        assert_eq!(generation_of(root.path()), original);
        owner.stop().await;
    }

    #[tokio::test]
    async fn directory_pin_entry_changes_invalidate_ready_without_new_probes() {
        for (change, selected) in [
            ("redirect", "python"),
            ("replace", "python"),
            ("preferred", "python3"),
            ("directory-alias", "python"),
            ("dangling-preferred", "python3"),
            ("cyclic-preferred", "python3"),
        ] {
            let root = workspace();
            let source = root.path().join("source-env");
            std::fs::create_dir_all(source.join("bin")).unwrap();
            let base = root.path().join("base-python");
            let other = root.path().join("other-python");
            write_executable(&base, "#!/bin/sh\nexit 0\n");
            write_executable(&other, "#!/bin/sh\nexit 0\n");
            let preferred_change = selected == "python3";
            let entry = source.join("bin").join(selected);
            std::os::unix::fs::symlink(&base, &entry).unwrap();
            write_linked_venv(&root.path().join(".venv"), &base);
            let directory_alias = root.path().join("source-alias");
            let pin = if change == "directory-alias" {
                std::os::unix::fs::symlink(&source, &directory_alias).unwrap();
                "source-alias"
            } else {
                "./source-env"
            };
            std::fs::write(root.path().join(".python-version"), pin).unwrap();
            let original = generation_of(root.path());
            let source_stamp = super::stamp(&source);
            let base_stamp = super::stamp(&base);
            let probe = Arc::new(Scripted::new("0.10.2"));
            let owner = Owner::for_test(probe.clone(), None, None, None);
            owner.prepare(root.path().to_path_buf()).await;
            assert!(owner.observe(root.path()).virtual_env.is_some());
            let fixed = retained_fixed_interpreter(&owner, root.path());
            let calls = probe.calls.lock().unwrap().len();
            if change == "directory-alias" {
                let other_source = root.path().join("other-env");
                std::fs::create_dir_all(other_source.join("bin")).unwrap();
                std::os::unix::fs::symlink(&base, other_source.join("bin/python")).unwrap();
                std::fs::rename(&directory_alias, root.path().join("previous-alias")).unwrap();
                std::os::unix::fs::symlink(other_source, directory_alias).unwrap();
            } else if preferred_change {
                let target = match change {
                    "dangling-preferred" => Path::new("missing-python"),
                    "cyclic-preferred" => Path::new("python"),
                    _ => &base,
                };
                std::os::unix::fs::symlink(target, source.join("bin/python")).unwrap();
            } else {
                std::fs::rename(&entry, source.join("bin/previous")).unwrap();
                std::os::unix::fs::symlink(
                    if change == "redirect" { &other } else { &base },
                    &entry,
                )
                .unwrap();
            }
            assert_eq!(generation_of(root.path()), original);
            assert_eq!(
                super::stamp(&source),
                source_stamp,
                "root directory stamp is unchanged"
            );
            assert_eq!(
                super::stamp(&base),
                base_stamp,
                "old base executable is unchanged"
            );
            assert!(!fixed.is_current(), "{change}");
            let (miss, mut arrival) = tokio::sync::oneshot::channel();
            *owner.cache_miss.lock().unwrap() = Some(miss);
            assert!(owner.observe(root.path()).virtual_env.is_none());
            arrival
                .try_recv()
                .expect("the entry change invalidates Ready");
            assert_eq!(
                probe.calls.lock().unwrap().len(),
                calls,
                "observation only inspects metadata"
            );
            if matches!(change, "dangling-preferred" | "cyclic-preferred") {
                owner.prepare(root.path().to_path_buf()).await;
                assert!(matches!(
                    owner.records.lock().unwrap().get(root.path()),
                    Some(Record::Unavailable { .. })
                ));
                assert!(
                    probe.calls.lock().unwrap()[calls..]
                        .iter()
                        .all(|argv| Path::new(&argv[0]) != entry)
                );
                assert_eq!(generation_of(root.path()), original);
            }
            owner.stop().await;
        }
    }

    #[tokio::test]
    async fn alias_change_between_interpreter_probes_cannot_replace_the_first_witness() {
        struct SecondProbeGate {
            scripted: Scripted,
            second: PathBuf,
            gate: Mutex<Option<super::TestGate>>,
        }
        impl ProgramProbe for SecondProbeGate {
            fn run(&self, request: ProgramRequest) -> BoxFuture<'_, Result<ProgramOutput, String>> {
                Box::pin(async move {
                    if Path::new(&request.argv[0]) == self.second {
                        super::pass_gate(&self.gate).await;
                    }
                    self.scripted.run(request).await
                })
            }
        }
        let root = workspace();
        let base = root.path().join("base-python");
        let other = root.path().join("other-python");
        let alias = root.path().join("pinned-python");
        write_executable(&base, "#!/bin/sh\nexit 0\n");
        write_executable(&other, "#!/bin/sh\nexit 0\n");
        std::os::unix::fs::symlink(&base, &alias).unwrap();
        write_linked_venv(&root.path().join(".venv"), &base);
        let second = root.path().join(".venv/bin/python3");
        std::fs::remove_file(&second).unwrap();
        std::os::unix::fs::symlink(&other, &second).unwrap();
        std::fs::write(root.path().join(".python-version"), "./pinned-python").unwrap();
        let original = generation_of(root.path());
        let probe = Arc::new(SecondProbeGate {
            scripted: Scripted::new("0.10.2"),
            second,
            gate: Mutex::new(None),
        });
        let (entered, release) = arm_gate(&probe.gate);
        let owner = Arc::new(Owner::for_test(probe, None, None, None));
        let preparing = owner.clone();
        let path = root.path().to_path_buf();
        let task = tokio::spawn(async move { preparing.prepare(path).await });
        entered.await.unwrap();
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&other, &alias).unwrap();
        release.send(()).unwrap();
        task.await.unwrap();
        assert!(owner.records.lock().unwrap().get(root.path()).is_none());
        assert_eq!(generation_of(root.path()), original);
        owner.stop().await;
    }

    #[tokio::test]
    async fn relative_pin_is_absolute_in_the_create_request() {
        let root = workspace();
        let pinned = root.path().join("pinned-python");
        write_executable(&pinned, "#!/bin/sh\nexit 0\n");
        std::fs::write(root.path().join(".python-version"), "./pinned-python\n").expect("pin");
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let bin = tempfile::tempdir().expect("bin");
        write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Owner::for_test(probe.clone(), Some(host_for(bin.path())), None, None);
        owner.prepare(root_path.clone()).await;
        let calls = probe.calls.lock().expect("calls");
        let find = calls
            .iter()
            .find(|argv| argv.iter().any(|arg| arg == "venv"))
            .expect("venv");
        let requested = &find[find
            .iter()
            .position(|arg| arg == "--python")
            .expect("python request")
            + 1];
        assert_eq!(
            requested,
            &super::absolute_pin(&root_path, "./pinned-python")
        );
        assert!(Path::new(requested).is_absolute());
        assert!(!requested.ends_with("./pinned-python"));
    }

    #[tokio::test]
    async fn issue_u11_one_flight_creates_the_environment_once() {
        let root = workspace();
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let bin = tempfile::tempdir().expect("bin");
        write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Arc::new(Owner::for_test(
            probe.clone(),
            Some(host_for(bin.path())),
            None,
            None,
        ));
        let first_owner = Arc::clone(&owner);
        let second_owner = Arc::clone(&owner);
        let first_root = root_path.clone();
        let second_root = root_path.clone();
        let first = tokio::spawn(async move { first_owner.prepare(first_root).await });
        let second = tokio::spawn(async move { second_owner.prepare(second_root).await });
        first.await.expect("first");
        second.await.expect("second");
        assert_eq!(
            probe
                .calls
                .lock()
                .expect("calls")
                .iter()
                .filter(|argv| argv.iter().any(|arg| arg == "venv"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn issue_u13_replaced_interpreter_is_not_still_ready() {
        let root = workspace();
        write_venv(&root.path().join(".venv"));
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let owner = Owner::for_test(Arc::new(Scripted::new("0.10.2")), None, None, None);
        owner.prepare(root_path.clone()).await;
        let python = root_path.join(".venv/bin/python");
        std::fs::remove_file(&python).expect("replace");
        write_executable(&python, "#!/bin/sh\necho replaced\nexit 0\n");
        let selection = owner.observe(&root_path);
        assert!(selection.virtual_env.is_none());
        assert!(!selection.path_prefix.is_empty());
    }

    #[tokio::test]
    async fn environment_structure_changes_invalidate_readiness() {
        for change in [
            "config",
            "root",
            "base-interpreter",
            "permissions",
            "pin-target",
        ] {
            let root = workspace();
            let root_path = std::fs::canonicalize(root.path()).expect("canonical");
            let target = root.path().join(".venv");
            let interpreter = root.path().join("base-python");
            write_executable(&interpreter, "#!/bin/sh\nexit 0\n");
            write_linked_venv(&target, &interpreter);
            let version = root.path().join("version");
            std::fs::write(&version, "3.12\n").expect("version");
            std::os::unix::fs::symlink(&version, root.path().join(".python-version"))
                .expect("pin link");
            let owner = Owner::for_test(Arc::new(Scripted::new("0.10.2")), None, None, None);
            owner.prepare(root_path.clone()).await;
            assert!(owner.observe(&root_path).virtual_env.is_some());
            match change {
                "config" => std::fs::remove_file(target.join("pyvenv.cfg")).expect("remove config"),
                "root" => {
                    let moved = root.path().join("moved-venv");
                    std::fs::rename(&target, &moved).expect("move environment");
                    std::os::unix::fs::symlink(&moved, &target).expect("link environment");
                }
                "base-interpreter" => {
                    std::fs::remove_file(&interpreter).expect("remove interpreter");
                    write_executable(&interpreter, "#!/bin/sh\necho replaced\nexit 0\n");
                }
                "permissions" => {
                    std::fs::set_permissions(&interpreter, std::fs::Permissions::from_mode(0o644))
                        .expect("interpreter permissions");
                }
                "pin-target" => std::fs::write(&version, "3.11.0\n").expect("changed pin target"),
                _ => unreachable!(),
            }
            assert!(
                owner.observe(&root_path).virtual_env.is_none(),
                "{change} left stale readiness"
            );
            owner.prepare(root_path.clone()).await;
            if matches!(change, "config" | "root" | "pin-target") {
                assert!(owner.observe(&root_path).virtual_env.is_none());
            }
        }
    }

    #[tokio::test]
    async fn unreadable_pin_is_a_conflict_without_probing() {
        for (filename, invalid) in [
            (".python-version", "bytes"),
            (".python-version", "directory"),
            (".python-version", "dangling-link"),
            ("pyproject.toml", "bytes"),
            ("pyproject.toml", "directory"),
            ("pyproject.toml", "dangling-link"),
        ] {
            let root = workspace();
            let root_path = std::fs::canonicalize(root.path()).expect("canonical");
            write_venv(&root.path().join(".venv"));
            let pin = root.path().join(filename);
            match invalid {
                "bytes" => std::fs::write(&pin, [0xff]).expect("invalid UTF-8 pin"),
                "directory" => std::fs::create_dir(&pin).expect("directory pin"),
                "dangling-link" => {
                    std::os::unix::fs::symlink(root.path().join("missing"), &pin)
                        .expect("dangling pin");
                }
                _ => unreachable!(),
            }
            let probe = Arc::new(Scripted::new("0.10.2"));
            let owner = Owner::for_test(probe.clone(), None, None, None);
            owner.prepare(root_path.clone()).await;
            assert!(probe.calls.lock().expect("calls").is_empty(), "{invalid}");
            assert!(matches!(
                owner.records.lock().expect("records").get(&root_path),
                Some(Record::Conflict { .. })
            ));
        }
    }

    #[test]
    fn failed_guard_install_does_not_restore_host_python() {
        let blocked = workspace();
        let not_directory = blocked.path().join("not-a-directory");
        std::fs::write(&not_directory, "blocked").expect("blocker");
        assert!(super::install_guard(&not_directory, "disk full").is_none());
        let selection = super::selection_from_guard(None, "disk full");
        assert!(!selection.guard_unavailable);
        assert!(!selection.path_prefix.is_empty());
        let host_bin = tempfile::tempdir().expect("host bin");
        write_executable(
            &host_bin.path().join("python"),
            "#!/bin/sh\nprintf 'HOST\\n'\nexit 0\n",
        );
        write_executable(
            &host_bin.path().join("python3"),
            "#!/bin/sh\nprintf 'HOST\\n'\nexit 0\n",
        );
        write_executable(
            &host_bin.path().join("tool"),
            "#!/bin/sh\nprintf 'TOOL\\n'\nexit 0\n",
        );
        let host = host_from_entries(vec![
            ("PATH", &host_bin.path().display().to_string()),
            ("HOME", "/users/ada"),
        ]);
        let entries = crate::tools::execution_environment::compose(
            &host,
            &crate::tools::environment::ToolEnvironment::new(),
            &selection,
        )
        .expect("composed");
        let path = entries
            .iter()
            .find(|(key, _)| key == "PATH")
            .expect("path")
            .1
            .clone();
        let python = std::process::Command::new("python")
            .env("PATH", &path)
            .output()
            .expect("python");
        assert_eq!(python.status.code(), Some(127));
        assert!(!String::from_utf8_lossy(&python.stderr).contains("HOST"));
        let tool = std::process::Command::new("tool")
            .env("PATH", &path)
            .output()
            .expect("tool");
        assert!(tool.status.success());
        assert_eq!(String::from_utf8_lossy(&tool.stdout), "TOOL\n");
    }

    #[test]
    fn incomplete_state_guard_does_not_restore_host_python() {
        for altered in [false, true] {
            let directory = workspace();
            let diagnostic = "Workspace Python is not ready";
            super::install_guard(directory.path(), diagnostic).expect("initial guard");
            let python = directory.path().join("python");
            if altered {
                std::fs::write(&python, "#!/bin/sh\nexit 0\n").expect("alter guard");
            } else {
                std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o644))
                    .expect("guard permissions");
            }
            let specific = super::install_guard(directory.path(), diagnostic);
            assert!(specific.is_none());
            let selection = super::selection_from_guard(specific, diagnostic);
            let entries = super::execution_environment::compose(
                &host_from_entries(vec![("PATH", "/usr/bin:/bin")]),
                &crate::tools::environment::ToolEnvironment::new(),
                &selection,
            )
            .expect("composed");
            for command in ["python", "/usr/bin/env python3"] {
                let output = run_bash(&entries, command);
                assert_eq!(output.status.code(), Some(127));
                assert!(String::from_utf8_lossy(&output.stderr).contains("Workspace Python"));
            }
        }
    }

    #[tokio::test]
    async fn changed_pin_during_probe_is_not_published_ready() {
        let root = workspace();
        write_venv(&root.path().join(".venv"));
        std::fs::write(root.path().join(".python-version"), "3.12\n").expect("pin");
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let probe = Arc::new(Scripted::new("0.10.2"));
        *probe.entered.lock().expect("entered") = Some(entered_tx);
        *probe.release.lock().expect("release") = Some(release_rx);
        let owner = Arc::new(Owner::for_test(probe, None, None, None));
        let preparing = Arc::clone(&owner);
        let prepare_root = root_path.clone();
        let prepare = tokio::spawn(async move { preparing.prepare(prepare_root).await });
        entered_rx.await.expect("probe entered");
        std::fs::write(root_path.join(".python-version"), "3.13.1\n").expect("new pin");
        release_tx.send(()).expect("release");
        prepare.await.expect("prepare");
        assert!(
            !matches!(
                owner.records.lock().expect("records").get(&root_path),
                Some(Record::Ready { .. })
            ),
            "a stale probe published Ready"
        );
    }

    #[tokio::test]
    async fn replaced_interpreter_during_probe_is_not_published_ready() {
        let root = workspace();
        write_venv(&root.path().join(".venv"));
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let probe = Arc::new(Scripted::new("0.10.2"));
        *probe.entered.lock().expect("entered") = Some(entered_tx);
        *probe.release.lock().expect("release") = Some(release_rx);
        let owner = Arc::new(Owner::for_test(probe, None, None, None));
        let preparing = Arc::clone(&owner);
        let prepare_root = root_path.clone();
        let prepare = tokio::spawn(async move { preparing.prepare(prepare_root).await });
        entered_rx.await.expect("probe entered");
        let python = root_path.join(".venv/bin/python");
        std::fs::remove_file(&python).expect("replace");
        write_executable(
            &python,
            "#!/bin/sh\nprintf 'replaced interpreter\\n'\nexit 0\n",
        );
        release_tx.send(()).expect("release");
        prepare.await.expect("prepare");
        assert!(
            !matches!(
                owner.records.lock().expect("records").get(&root_path),
                Some(Record::Ready { .. })
            ),
            "a replaced interpreter published Ready"
        );
    }

    #[tokio::test]
    async fn implementation_request_accepts_and_rejects_without_uv() {
        let rejected = workspace();
        write_venv(&rejected.path().join(".venv"));
        std::fs::write(rejected.path().join(".python-version"), "pypy-3.12\n").expect("pin");
        let rejected_root = std::fs::canonicalize(rejected.path()).expect("canonical");
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Owner::for_test(probe.clone(), None, None, None);
        owner.prepare(rejected_root.clone()).await;
        assert!(
            probe
                .calls
                .lock()
                .expect("calls")
                .iter()
                .all(|argv| !argv.iter().any(|arg| arg == "venv" || arg == "--version"))
        );
        match owner.records.lock().expect("records").get(&rejected_root) {
            Some(Record::Conflict { .. }) => {}
            other => panic!("expected conflict, got {other:?}"),
        }

        let accepted = workspace();
        write_venv(&accepted.path().join(".venv"));
        std::fs::write(accepted.path().join(".python-version"), "pypy-3.12\n").expect("pin");
        let accepted_root = std::fs::canonicalize(accepted.path()).expect("canonical");
        let probe = Arc::new(Scripted::with_implementation("0.10.2", "pypy"));
        let owner = Owner::for_test(probe.clone(), None, None, None);
        owner.prepare(accepted_root.clone()).await;
        assert!(
            probe
                .calls
                .lock()
                .expect("calls")
                .iter()
                .all(|argv| argv.iter().any(|arg| arg == "-I"))
        );
        match owner.records.lock().expect("records").get(&accepted_root) {
            Some(Record::Ready { .. }) => {}
            other => panic!("expected ready, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shutdown_stops_waiting_for_a_lock_another_process_still_holds() {
        const NAME: &str = "tools::workspace_python::tests::shutdown_stops_waiting_for_a_lock_another_process_still_holds";
        if std::env::var_os("RUSTX_WORKSPACE_PYTHON_LOCK_HOLDER").is_some() {
            use std::io::Write;
            let path = std::env::var("RUSTX_WORKSPACE_PYTHON_LOCK_PATH").expect("lock path");
            let ready = std::env::var("RUSTX_WORKSPACE_PYTHON_LOCK_READY").expect("ready fifo");
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .expect("lock file");
            let _lock =
                nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive).expect("lock");
            let mut signal = std::fs::OpenOptions::new()
                .write(true)
                .open(ready)
                .expect("ready fifo");
            signal.write_all(b"1").expect("signal");
            let mut input = String::new();
            let _ = std::io::stdin().read_line(&mut input);
            return;
        }
        let state = tempfile::tempdir().expect("state");
        let root = workspace();
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let bin = tempfile::tempdir().expect("bin");
        write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
        let lock_path = lock_file(state.path(), &root_path);
        std::fs::create_dir_all(lock_path.parent().expect("lock dir")).expect("locks");
        let ready = state.path().join("ready");
        nix::unistd::mkfifo(
            &ready,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .expect("ready fifo");
        let mut child = std::process::Command::new(std::env::current_exe().expect("test exe"))
            .args(["--exact", NAME, "--test-threads=1", "--nocapture"])
            .env("RUSTX_WORKSPACE_PYTHON_LOCK_HOLDER", "1")
            .env("RUSTX_WORKSPACE_PYTHON_LOCK_PATH", &lock_path)
            .env("RUSTX_WORKSPACE_PYTHON_LOCK_READY", &ready)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("holder");
        let ready_read = ready.clone();
        tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut fifo = std::fs::File::open(ready_read).expect("open ready fifo");
            let mut byte = [0u8; 1];
            fifo.read_exact(&mut byte).expect("holder locked");
        })
        .await
        .expect("holder task");
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Arc::new(Owner::for_test(
            probe.clone(),
            Some(host_for(bin.path())),
            Some(state.path().to_path_buf()),
            Some(sender),
        ));
        let preparing = Arc::clone(&owner);
        let prepare_root = root_path.clone();
        let prepare = tokio::spawn(async move { preparing.prepare(prepare_root).await });
        receiver.await.expect("preparation reached the lock");
        owner.shutdown.cancel();
        prepare.await.expect("prepare");
        assert!(child.try_wait().expect("holder status").is_none());
        assert!(probe.calls.lock().expect("calls").is_empty());
        drop(child.stdin.take());
        let status = tokio::task::spawn_blocking(move || child.wait().expect("holder exit"))
            .await
            .expect("wait");
        assert!(status.success());
    }

    #[tokio::test]
    async fn repeated_admission_keeps_one_task() {
        let root = workspace();
        write_venv(&root.path().join(".venv"));
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let probe = Arc::new(Scripted::new("0.10.2"));
        *probe.entered.lock().expect("entered") = Some(entered_tx);
        *probe.release.lock().expect("release") = Some(release_rx);
        let mut owner = Owner::for_test(probe, None, None, None);
        owner.schedules = true;
        let owner = Arc::new(owner);
        let preparing = Arc::clone(&owner);
        let prepare_root = root_path.clone();
        let prepare = tokio::spawn(async move { preparing.prepare(prepare_root).await });
        entered_rx.await.expect("probe entered");
        for _ in 0..20 {
            owner.schedule_on(&root_path);
        }
        assert!(owner.tasks.lock().expect("tasks").len() <= 1);
        let follower = owner.tasks.lock().expect("tasks").pop();
        release_tx.send(()).expect("release");
        prepare.await.expect("prepare");
        if let Some(follower) = follower {
            follower.await.expect("follower");
        }
        assert_eq!(owner.records.lock().expect("records").len(), 1);
    }

    pub(crate) async fn ancestor_python_version_does_not_bind_workspace_discovery() {
        const CASE: &str = "RUSTX_TEST_DISCOVERY_ANCESTOR_ROOT";
        if let Some(base) = std::env::var_os(CASE) {
            let base = PathBuf::from(base);
            let path = installed_uv_path();
            let managed = base.join("managed");
            std::fs::create_dir(&managed).unwrap();
            for (name, preference, ready) in [
                ("ordinary", "only-system", true),
                ("restricted", "only-managed", false),
            ] {
                let root = base.join(name);
                std::fs::create_dir(&root).unwrap();
                let config = base.join(format!("config-{name}"));
                std::fs::create_dir_all(config.join("uv")).unwrap();
                std::fs::write(config.join("uv/uv.toml"), format!("offline = true\npython-downloads = \"never\"\npython-preference = {preference:?}\n")).unwrap();
                let host = host_from_entries(vec![
                    ("PATH", &path),
                    ("XDG_CONFIG_HOME", config.to_str().unwrap()),
                    ("UV_PYTHON_INSTALL_DIR", managed.to_str().unwrap()),
                ]);
                let owner = Owner::for_test(
                    Arc::new(super::SupervisedProbe),
                    Some(Arc::new(host)),
                    Some(base.join("state")),
                    None,
                );
                owner.prepare(root.clone()).await;
                let is_ready = matches!(
                    owner.records.lock().unwrap().get(&root),
                    Some(Record::Ready { .. })
                );
                assert_eq!(
                    is_ready,
                    ready,
                    "{name}: {:?}",
                    owner.records.lock().unwrap()
                );
                if ready {
                    assert!(root.join(".venv/bin/python").is_file());
                } else {
                    assert!(!root.join(".venv").exists());
                }
                owner.stop().await;
            }
            assert!(super::discovery_directory().starts_with(base.join("tmp")));
            return;
        }
        let base = workspace();
        std::fs::write(base.path().join(".python-version"), "9.99").unwrap();
        std::fs::create_dir(base.path().join("tmp")).unwrap();
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "boundary_suites::workspace_python::ancestor_python_version_does_not_bind_workspace_discovery", "--nocapture"])
            .env(CASE, base.path()).env("TMPDIR", base.path().join("tmp"))
            .kill_on_drop(true).output().await.unwrap();
        assert!(
            output.status.success(),
            "child production pipeline: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            base.path().join("ordinary/.venv/pyvenv.cfg").is_file(),
            "the child must execute the production preparation case"
        );
        assert!(!base.path().join("restricted/.venv").exists());
    }

    #[test]
    fn unicode_and_graalpython_requests_parse_without_a_short_prefix() {
        assert!(parse_request("中文测试").is_err());
        match parse_request("graalpython-3.12").expect("graalpython") {
            ParsedRequest::Release {
                implementation,
                version,
            } => {
                assert_eq!(implementation.as_deref(), Some("graalpython"));
                assert_eq!(version.release(), &[3u64, 12]);
            }
            ParsedRequest::Spec { .. } => panic!("graalpython-3.12 is an exact request"),
        }
    }

    #[tokio::test]
    async fn unicode_pin_does_not_wedge_a_later_preparation() {
        let root = workspace();
        write_venv(&root.path().join(".venv"));
        std::fs::write(root.path().join(".python-version"), "中文测试\n").expect("pin");
        let root_path = std::fs::canonicalize(root.path()).expect("canonical");
        let owner = Owner::for_test(Arc::new(Scripted::new("0.10.2")), None, None, None);
        owner.prepare(root_path.clone()).await;
        match owner.records.lock().expect("records").get(&root_path) {
            Some(Record::Conflict { .. }) => {}
            other => panic!("expected conflict, got {other:?}"),
        }
        std::fs::write(root_path.join(".python-version"), "3.12\n").expect("valid pin");
        let prepare = owner.prepare(root_path.clone());
        tokio::time::timeout(std::time::Duration::from_secs(5), prepare)
            .await
            .expect("a later pin must not wait on the unfinished flight");
        match owner.records.lock().expect("records").get(&root_path) {
            Some(Record::Ready { .. }) => {}
            other => panic!("expected ready, got {other:?}"),
        }
    }

    #[test]
    fn failed_discovery_directory_is_not_cached() {
        let blocked = tempfile::tempdir().expect("blocked");
        let file = blocked.path().join("not-a-directory");
        std::fs::write(&file, "x").expect("file");
        let mut cached = None;
        assert!(super::fresh_discovery(&mut cached, std::slice::from_ref(&file)).is_none());
        assert!(cached.is_none());
        let good = tempfile::tempdir().expect("good");
        let first = super::fresh_discovery(&mut cached, &[file.clone(), good.path().to_path_buf()])
            .expect("later parent");
        assert!(first.is_dir());
        assert_eq!(
            super::fresh_discovery(&mut cached, std::slice::from_ref(&file)).as_ref(),
            Some(&first)
        );
        std::fs::remove_dir(&first).expect("remove");
        let good_path = good.path().to_path_buf();
        let second = super::fresh_discovery(&mut cached, std::slice::from_ref(&good_path))
            .expect("replacement");
        assert_ne!(first, second);
        assert!(second.is_dir());
    }

    #[test]
    fn preexisting_guard_directory_is_not_adopted() {
        let parent = tempfile::tempdir().expect("parent");
        let planted = parent.path().join("planted");
        std::fs::create_dir(&planted).expect("planted");
        write_executable(&planted.join("python"), "#!/bin/sh\nprintf 'HIJACK\\n'\n");
        write_executable(&planted.join("python3"), "#!/bin/sh\nprintf 'HIJACK\\n'\n");
        assert!(super::claim_fresh_directory(&planted).is_none());
    }

    fn run_bash(entries: &[(String, String)], program: &str) -> std::process::Output {
        run_program(entries, "/bin/bash", &["-c", program])
    }

    fn run_program(
        entries: &[(String, String)],
        program: &str,
        args: &[&str],
    ) -> std::process::Output {
        let mut command = std::process::Command::new(program);
        command.args(args).env_clear();
        for (key, value) in entries {
            command.env(key, value);
        }
        command.output().expect("program")
    }

    fn assert_guard_rejected(output: &std::process::Output) {
        assert_eq!(output.status.code(), Some(127), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("disk full"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("99"));
    }

    #[test]
    fn failed_guard_directories_reject_python_and_run_other_commands() {
        let blocked = tempfile::tempdir().expect("blocked");
        let file = blocked.path().join("not-a-directory");
        std::fs::write(&file, "x").expect("file");
        let good = tempfile::tempdir().expect("good");
        assert!(super::install_process_guard(std::slice::from_ref(&file), "disk full").is_none());
        let guard = super::install_process_guard(&[file, good.path().to_path_buf()], "disk full")
            .expect("a later private directory holds the guard");
        let selection = PythonSelection {
            path_prefix: vec![guard],
            virtual_env: None,
            diagnostic: "disk full".to_owned(),
            guard_unavailable: false,
        };
        let host = host_from_entries(vec![("PATH", "/usr/bin:/bin"), ("HOME", "/home/ada")]);
        let entries = crate::tools::execution_environment::compose(
            &host,
            &crate::tools::environment::ToolEnvironment::new(),
            &selection,
        )
        .expect("non-python execution stays available");
        let python = run_bash(&entries, "python3 -c 'print(99)'");
        assert_eq!(python.status.code(), Some(127));
        assert!(String::from_utf8_lossy(&python.stderr).contains("disk full"));
        assert!(!String::from_utf8_lossy(&python.stdout).contains("99"));
        let via_env = run_bash(&entries, "/usr/bin/env python3 -c 'print(99)'");
        assert_eq!(via_env.status.code(), Some(127));
        let listed = run_bash(&entries, "ls /bin/sh >/dev/null && printf ok");
        assert!(listed.status.success());
        assert_eq!(String::from_utf8_lossy(&listed.stdout), "ok");
    }

    #[test]
    fn no_guard_directory_rejects_path_lookup_and_keeps_other_commands() {
        let selection = super::finish_guard(None, "disk full");
        assert!(!selection.guard_unavailable);
        let host = host_from_entries(vec![("PATH", "/usr/bin:/bin"), ("HOME", "/home/ada")]);
        let entries = crate::tools::execution_environment::compose(
            &host,
            &crate::tools::environment::ToolEnvironment::new(),
            &selection,
        )
        .expect("composition stays available");
        let deployed = super::provisioned_guard().expect("guard beside the test executable");
        let executable = std::env::current_exe().expect("test executable");
        let beside = executable
            .parent()
            .expect("executable directory")
            .join("python-guard");
        assert!(
            deployed == beside
                || std::fs::canonicalize(&executable)
                    .ok()
                    .and_then(|path| path.parent().map(|parent| parent.join("python-guard")))
                    .as_ref()
                    == Some(&deployed)
        );
        assert!(!deployed.ends_with("assets/python-guard"));
        assert_eq!(selection.path_prefix, vec![deployed]);
        let path = entries
            .iter()
            .find(|(key, _)| key == "PATH")
            .map(|(_, value)| value.as_str())
            .expect("PATH");
        assert!(!path.contains("/assets/python-guard"));
        assert!(path.contains("/usr/bin"));
        assert!(
            entries
                .iter()
                .any(|(key, value)| { key == super::GUARD_MESSAGE_KEY && value == "disk full" })
        );
        assert_guard_rejected(&run_bash(&entries, "python3 -c 'print(99)'"));
        assert_guard_rejected(&run_program(
            &entries,
            "/usr/bin/env",
            &["python3", "-c", "print(99)"],
        ));
        assert_guard_rejected(&run_program(
            &entries,
            "/bin/sh",
            &["-c", "python3 -c 'print(99)'"],
        ));
        let script_dir = tempfile::tempdir().expect("script");
        let script = script_dir.path().join("tool");
        write_executable(&script, "#!/usr/bin/env python3\nprint(99)\n");
        assert_guard_rejected(&run_program(&entries, &script.display().to_string(), &[]));
        let listed = run_program(
            &entries,
            "/bin/sh",
            &["-c", "ls /bin/sh >/dev/null && printf ok"],
        );
        assert!(listed.status.success(), "{listed:?}");
        assert_eq!(String::from_utf8_lossy(&listed.stdout), "ok");
    }

    fn install_embedded_guard(directory: &Path) {
        std::fs::create_dir_all(directory).expect("guard directory");
        for (name, bytes) in [
            ("python", super::GUARD_PYTHON),
            ("python3", super::GUARD_PYTHON3),
        ] {
            let path = directory.join(name);
            std::fs::write(&path, bytes).expect("guard script");
            let mut permissions = std::fs::metadata(&path).expect("meta").permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).expect("mode");
        }
    }

    #[test]
    fn missing_or_altered_deployed_guard_does_not_use_the_build_tree() {
        let install = tempfile::tempdir().expect("install");
        let executable = install.path().join("rustx");
        std::fs::write(&executable, b"bin").expect("executable");
        assert!(super::guard_beside_executable(&executable).is_none());
        let source = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/python-guard/python3"
        ));
        assert!(
            source.is_file(),
            "the checkout copy is not the runtime lookup"
        );
        let planted = install.path().join("python-guard");
        write_executable(
            &planted.join("python"),
            "#!/bin/sh\nprintf 'HIJACK\\n'\nexit 0\n",
        );
        write_executable(
            &planted.join("python3"),
            "#!/bin/sh\nprintf 'HIJACK\\n'\nexit 0\n",
        );
        assert!(super::guard_beside_executable(&executable).is_none());
    }

    #[test]
    fn relocated_install_rejects_python_without_the_build_tree() {
        let install = tempfile::tempdir().expect("install");
        let executable = install.path().join("rustx");
        std::fs::write(&executable, b"bin").expect("executable");
        let guard = install.path().join("python-guard");
        install_embedded_guard(&guard);
        let resolved = super::guard_beside_executable(&executable).expect("deployed guard");
        assert_eq!(resolved, guard);
        assert!(!resolved.starts_with(env!("CARGO_MANIFEST_DIR")));

        let blocked = tempfile::tempdir().expect("blocked");
        let file = blocked.path().join("not-a-directory");
        std::fs::write(&file, "x").expect("file");
        assert!(super::install_process_guard(std::slice::from_ref(&file), "disk full").is_none());
        let selection = super::directory_guard(Some(resolved), "disk full");
        assert!(!selection.guard_unavailable);
        let host = host_from_entries(vec![("PATH", "/usr/bin:/bin"), ("HOME", "/home/ada")]);
        let entries = crate::tools::execution_environment::compose(
            &host,
            &crate::tools::environment::ToolEnvironment::new(),
            &selection,
        )
        .expect("composition stays available");
        let path = entries
            .iter()
            .find(|(key, _)| key == "PATH")
            .map(|(_, value)| value.as_str())
            .expect("PATH");
        assert!(path.starts_with(&format!("{}:", guard.display())));
        assert!(!path.contains("/assets/python-guard"));
        assert_guard_rejected(&run_bash(&entries, "python3 -c 'print(99)'"));
        assert_guard_rejected(&run_program(
            &entries,
            "/usr/bin/env",
            &["python3", "-c", "print(99)"],
        ));
        assert_guard_rejected(&run_program(
            &entries,
            "/bin/sh",
            &["-c", "python3 -c 'print(99)'"],
        ));
        let script = install.path().join("tool");
        write_executable(&script, "#!/usr/bin/env python3\nprint(99)\n");
        assert_guard_rejected(&run_program(&entries, &script.display().to_string(), &[]));
        let listed = run_program(
            &entries,
            "/bin/sh",
            &["-c", "ls /bin/sh >/dev/null && printf ok"],
        );
        assert!(listed.status.success(), "{listed:?}");
        assert_eq!(String::from_utf8_lossy(&listed.stdout), "ok");
    }

    pub(crate) async fn relative_interpreter_pin_is_bound_to_the_workspace() {
        for launcher in [false, true] {
            let root = workspace();
            let interpreter = PathBuf::from("/usr/bin/python3");
            assert!(
                interpreter.is_file(),
                "system Python required by boundary suite"
            );
            let requested = if launcher {
                let wrapper = root.path().join("python-launcher");
                write_executable(&wrapper, "#!/bin/sh\nexec /usr/bin/python3 \"$@\"\n");
                wrapper
            } else {
                interpreter.clone()
            };
            std::os::unix::fs::symlink(&requested, root.path().join("pinned-python")).unwrap();
            std::fs::write(root.path().join(".python-version"), "./pinned-python").unwrap();
            let version = python_version(&interpreter);
            std::fs::write(
                root.path().join("pyproject.toml"),
                format!(
                    "[project]\nrequires-python = \">={}.{}\"\n",
                    version.release()[0],
                    version.release()[1]
                ),
            )
            .unwrap();
            let path = installed_uv_path();
            let host = host_from_entries(vec![
                ("PATH", &path),
                ("UV_OFFLINE", "1"),
                ("UV_PYTHON_DOWNLOADS", "never"),
            ]);
            let owner = Owner::for_test(
                Arc::new(super::SupervisedProbe),
                Some(Arc::new(host)),
                None,
                None,
            );
            owner.prepare(root.path().to_path_buf()).await;
            assert!(
                matches!(
                    owner.records.lock().unwrap().get(root.path()),
                    Some(Record::Ready { .. })
                ),
                "{:?}",
                owner.records.lock().unwrap()
            );
            assert_eq!(
                python_base_executable(&root.path().join(".venv/bin/python")),
                python_base_executable(&interpreter)
            );
            owner.stop().await;
        }
    }

    pub(crate) async fn directory_pins_create_and_reuse_without_uv() {
        for spelling in ["absolute", "relative", "bare"] {
            let root = workspace();
            let source = root.path().join("source-env");
            let path = installed_uv_path();
            for args in [
                vec!["venv", "--no-project", "--python", "/usr/bin/python3"],
                vec!["python", "pin", "--no-project"],
            ] {
                let request = if spelling == "bare" && args[0] == "python" {
                    Path::new("source-env")
                } else {
                    &source
                };
                let output = std::process::Command::new("uv")
                    .args(args)
                    .arg(request)
                    .arg("--no-config")
                    .current_dir(root.path())
                    .env_clear()
                    .env("PATH", &path)
                    .env("HOME", root.path())
                    .env("UV_OFFLINE", "1")
                    .env("UV_PYTHON_DOWNLOADS", "never")
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{output:?}");
            }
            assert_eq!(
                std::fs::read_to_string(root.path().join(".python-version"))
                    .unwrap()
                    .trim(),
                if spelling == "bare" {
                    "source-env"
                } else {
                    source.to_str().unwrap()
                },
                "uv itself emits the absolute and bare directory pins"
            );
            if spelling == "relative" {
                std::fs::write(root.path().join(".python-version"), "./source-env").unwrap();
                // Exercise the python3-only layout as well as the usual pair.
                let base = python_base_executable(&source.join("bin/python3"));
                std::fs::remove_file(source.join("bin/python")).unwrap();
                std::fs::remove_file(source.join("bin/python3")).unwrap();
                std::os::unix::fs::symlink(base, source.join("bin/python3")).unwrap();
            }
            std::fs::write(
                root.path().join("pyproject.toml"),
                "[project]\nrequires-python = \">=3\"\n",
            )
            .unwrap();
            let host = host_from_entries(vec![
                ("PATH", &path),
                ("UV_OFFLINE", "1"),
                ("UV_PYTHON_DOWNLOADS", "never"),
            ]);
            let owner = Owner::for_test(
                Arc::new(super::SupervisedProbe),
                Some(Arc::new(host)),
                None,
                None,
            );
            owner.prepare(root.path().to_path_buf()).await;
            assert_eq!(
                owner.observe(root.path()).virtual_env,
                Some(root.path().join(".venv")),
                "spelling={spelling}: {:?}",
                owner.records.lock().unwrap()
            );
            assert_eq!(
                python_base_executable(&root.path().join(".venv/bin/python")),
                python_base_executable(&source.join("bin/python3"))
            );
            owner.stop().await;

            let no_uv = workspace();
            let host = host_for(no_uv.path());
            assert!(super::find_uv(&host, root.path()).is_none());
            let original = generation_of(root.path());
            let owner = Owner::for_test(Arc::new(super::SupervisedProbe), Some(host), None, None);
            owner.prepare(root.path().to_path_buf()).await;
            assert_eq!(
                owner.observe(root.path()).virtual_env,
                Some(root.path().join(".venv")),
                "reuse without uv, spelling={spelling}: {:?}",
                owner.records.lock().unwrap()
            );
            assert_eq!(
                generation_of(root.path()),
                original,
                "adoption never rebuilds"
            );
            owner.stop().await;
        }
    }

    pub(crate) async fn invalid_directory_pins_fail_without_changing_the_target() {
        for kind in ["empty", "nonexecutable", "not-python", "dangling", "cyclic"] {
            for reuse in [false, true] {
                let root = workspace();
                let source = root.path().join("source-env");
                std::fs::create_dir_all(source.join("bin")).unwrap();
                if matches!(kind, "dangling" | "cyclic") {
                    std::os::unix::fs::symlink(
                        if kind == "dangling" {
                            "missing-python"
                        } else {
                            "python"
                        },
                        source.join("bin/python"),
                    )
                    .unwrap();
                    std::os::unix::fs::symlink("/usr/bin/python3", source.join("bin/python3"))
                        .unwrap();
                } else if kind != "empty" {
                    write_executable(&source.join("bin/python"), "#!/bin/sh\nexit 0\n");
                    if kind == "nonexecutable" {
                        std::fs::set_permissions(
                            source.join("bin/python"),
                            std::fs::Permissions::from_mode(0o600),
                        )
                        .unwrap();
                        std::os::unix::fs::symlink("/usr/bin/python3", source.join("bin/python3"))
                            .unwrap();
                    }
                }
                std::fs::write(root.path().join(".python-version"), "./source-env").unwrap();
                if reuse {
                    let output = std::process::Command::new("/usr/bin/python3")
                        .args(["-m", "venv", "--without-pip"])
                        .arg(root.path().join(".venv"))
                        .env_remove("PYTHONEXECUTABLE")
                        .env_remove("__PYVENV_LAUNCHER__")
                        .output()
                        .unwrap();
                    assert!(output.status.success(), "{output:?}");
                }
                let original = generation_of(root.path());
                let path = installed_uv_path();
                let host = host_from_entries(vec![
                    ("PATH", &path),
                    ("UV_OFFLINE", "1"),
                    ("UV_PYTHON_DOWNLOADS", "never"),
                ]);
                let probe = Arc::new(RecordingSupervisedProbe::default());
                let owner = Owner::for_test(probe.clone(), Some(Arc::new(host)), None, None);
                tokio::time::timeout(
                    super::PROBE_TIMEOUT * 4,
                    owner.prepare(root.path().to_path_buf()),
                )
                .await
                .unwrap();
                {
                    let records = owner.records.lock().unwrap();
                    assert!(
                        matches!(records.get(root.path()), Some(Record::Unavailable { .. })),
                        "{kind}, reuse={reuse}: {records:?}"
                    );
                }
                assert_eq!(generation_of(root.path()), original);
                assert!(owner.observe(root.path()).virtual_env.is_none());
                {
                    let calls = probe.calls.lock().unwrap();
                    assert!(
                        calls
                            .iter()
                            .all(|argv| Path::new(&argv[0]) != source.join("bin/python3")),
                        "no fallback interpreter probe: {calls:?}"
                    );
                    assert!(
                        calls
                            .iter()
                            .all(|argv| !argv.iter().any(|arg| arg == "venv")),
                        "no creation dispatch: {calls:?}"
                    );
                    if matches!(kind, "dangling" | "cyclic") {
                        assert!(
                            calls
                                .iter()
                                .all(|argv| Path::new(&argv[0]) != source.join("bin/python")),
                            "invalid selected entry fails before probing: {calls:?}"
                        );
                    }
                }
                owner.stop().await;
            }
        }
    }

    #[tokio::test]
    async fn live_preparations_stay_joined_past_the_task_cap() {
        let (release_tx, release_rx) = tokio::sync::watch::channel(false);
        let probe = Arc::new(HoldProbe {
            release: release_rx,
        });
        let mut owner = Owner::for_test(probe.clone(), None, None, None);
        owner.schedules = true;
        let owner = Arc::new(owner);
        let mut roots = Vec::new();
        for _ in 0..65 {
            let root = workspace();
            write_venv(&root.path().join(".venv"));
            let root_path = std::fs::canonicalize(root.path()).expect("canonical");
            roots.push(root);
            owner.schedule_on(&root_path);
        }
        assert_eq!(owner.tasks.lock().expect("tasks").len(), 64);
        assert_eq!(owner.admitted.lock().expect("admitted").len(), 64);
        owner.shutdown.cancel();
        release_tx.send(true).expect("release");
        let tasks = std::mem::take(&mut *owner.tasks.lock().expect("tasks"));
        assert_eq!(tasks.len(), 64);
        for task in tasks {
            tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .expect("shutdown joins a live preparation")
                .expect("task");
        }
    }
    fn arm_gate(
        slot: &Mutex<Option<super::TestGate>>,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, arrival) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        *slot.lock().unwrap() = Some(super::TestGate {
            entered,
            release: released,
        });
        (arrival, release)
    }

    #[tokio::test]
    async fn special_and_oversized_version_files_fail_without_probes_and_join() {
        for name in [".python-version", "pyproject.toml"] {
            for kind in ["fifo", "fifo-link", "oversized"] {
                let root = workspace();
                let path = root.path().join(name);
                if kind == "oversized" {
                    let file = std::fs::File::create(&path).unwrap();
                    file.set_len(1024 * 1024 + 1).unwrap();
                } else {
                    let fifo = root.path().join("pipe");
                    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR).unwrap();
                    if kind == "fifo" {
                        std::fs::rename(fifo, &path).unwrap();
                    } else {
                        std::os::unix::fs::symlink(fifo, &path).unwrap();
                    }
                }
                let probe = Arc::new(Scripted::new("0.10.2"));
                let owner = Owner::for_test(probe.clone(), None, None, None);
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    owner.prepare(root.path().to_path_buf()),
                )
                .await
                .unwrap();
                assert!(
                    matches!(
                        owner.records.lock().unwrap().get(root.path()),
                        Some(Record::Conflict { .. })
                    ),
                    "{name}: {kind}"
                );
                assert!(probe.calls.lock().unwrap().is_empty());
                tokio::time::timeout(std::time::Duration::from_secs(2), owner.stop())
                    .await
                    .unwrap();
            }
        }
    }

    #[tokio::test]
    async fn changed_pin_before_adopt_snapshot_is_never_validated_as_new_generation() {
        let root = workspace();
        write_venv(&root.path().join(".venv"));
        std::fs::write(root.path().join(".python-version"), "3.12").unwrap();
        let probe = Arc::new(Scripted::new("0.10.2"));
        let owner = Arc::new(Owner::for_test(probe.clone(), None, None, None));
        let (entered, release) = arm_gate(&owner.before_snapshot);
        let preparing = owner.clone();
        let path = root.path().to_path_buf();
        let task = tokio::spawn(async move { preparing.prepare(path).await });
        entered.await.unwrap();
        std::fs::write(root.path().join(".python-version"), "3.11").unwrap();
        release.send(()).unwrap();
        task.await.unwrap();
        assert!(owner.records.lock().unwrap().get(root.path()).is_none());
        assert!(probe.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn environment_root_link_appearing_during_uv_discovery_is_a_conflict() {
        let root = workspace();
        let external = workspace();
        write_venv(&external.path().join("venv"));
        let bin = workspace();
        write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
        let probe = Arc::new(Scripted::new("0.10.2"));
        let (entered, arrival) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        *probe.entered.lock().unwrap() = Some(entered);
        *probe.release.lock().unwrap() = Some(released);
        let owner = Arc::new(Owner::for_test(
            probe.clone(),
            Some(host_for(bin.path())),
            None,
            None,
        ));
        let preparing = owner.clone();
        let path = root.path().to_path_buf();
        let task = tokio::spawn(async move { preparing.prepare(path).await });
        arrival.await.unwrap();
        std::os::unix::fs::symlink(external.path().join("venv"), root.path().join(".venv"))
            .unwrap();
        release.send(()).unwrap();
        task.await.unwrap();
        assert!(matches!(
            owner.records.lock().unwrap().get(root.path()),
            Some(Record::Conflict { .. })
        ));
        assert_eq!(
            probe.calls.lock().unwrap().len(),
            1,
            "no create or interpreter execution after conflict"
        );
    }

    #[test]
    fn cached_guard_deletion_and_mode_changes_select_a_verified_replacement() {
        let parent = workspace();
        let mut cached = None;
        let parents = [parent.path().to_path_buf()];
        let first = super::cached_guard(&mut cached, &parents).unwrap();
        std::fs::remove_dir_all(&first).unwrap();
        let second = super::cached_guard(&mut cached, &parents).unwrap();
        assert_ne!(first, second);
        std::fs::set_permissions(
            second.join("python3"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let third = super::cached_guard(&mut cached, &parents).unwrap();
        assert_ne!(second, third);
        let host = host_from_entries(vec![("PATH", "/usr/bin:/bin")]);
        let selection = super::directory_guard(Some(third), "disk full");
        let entries = crate::tools::execution_environment::compose(
            &host,
            &crate::tools::environment::ToolEnvironment::new(),
            &selection,
        )
        .unwrap();
        assert_guard_rejected(&run_bash(&entries, "python3"));
        assert_guard_rejected(&run_bash(&entries, "/usr/bin/env python3"));
        assert!(run_bash(&entries, "ls /bin/sh >/dev/null").status.success());
    }

    pub(crate) async fn replaced_root_while_create_waits_for_permit_is_not_written() {
        let parent = workspace();
        let root = parent.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        let bin = workspace();
        write_executable(
            &bin.path().join("uv"),
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'uv 0.10.2\\n'; else /usr/bin/touch physical-start; fi\n",
        );
        let owner = Arc::new(Owner::for_test(
            Arc::new(super::SupervisedProbe),
            Some(host_for(bin.path())),
            None,
            None,
        ));
        let (entered, release) = arm_gate(&owner.before_create);
        let preparing = owner.clone();
        let path = root.clone();
        let task = tokio::spawn(async move { preparing.prepare(path).await });
        entered.await.unwrap();
        let held = owner.permits.acquire_many(2).await.unwrap();
        release.send(()).unwrap();
        std::fs::rename(&root, parent.path().join("old-workspace")).unwrap();
        std::fs::create_dir(&root).unwrap();
        drop(held);
        task.await.unwrap();
        assert!(!root.join("physical-start").exists());
        assert!(!root.join(".venv").exists());
        assert!(owner.records.lock().unwrap().get(&root).is_none());
    }

    pub(crate) async fn shutdown_between_check_and_signal_cannot_start_a_program() {
        let root = workspace();
        write_venv(&root.path().join(".venv"));
        for name in ["python", "python3"] {
            write_executable(
                &root.path().join(".venv/bin").join(name),
                "#!/bin/sh\n/usr/bin/touch physical-start\n",
            );
        }
        let mut owner = Owner::for_test(Arc::new(super::SupervisedProbe), None, None, None);
        owner.schedules = true;
        let owner = Arc::new(owner);
        let (entered, release) = arm_gate(&owner.before_signal);
        owner.schedule_on(root.path());
        entered.await.unwrap();
        let stopping = owner.clone();
        let task = tokio::spawn(async move { stopping.stop().await });
        owner.shutdown.cancelled().await;
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(!root.path().join("physical-start").exists());
        assert!(owner.tasks.lock().unwrap().is_empty());
        assert!(owner.flights.lock().unwrap().is_empty());
    }
    #[test]
    fn all_guard_sources_missing_cannot_compose_an_unguarded_host_path() {
        let blocked = workspace();
        let file = blocked.path().join("not-directory");
        std::fs::write(&file, "blocked").unwrap();
        let mut cached = None;
        assert!(super::cached_guard(&mut cached, &[file]).is_none());
        let selection = super::directory_guard(None, "no guard resource");
        let host = host_from_entries(vec![("PATH", "/usr/bin:/bin")]);
        let error = crate::tools::execution_environment::compose(
            &host,
            &crate::tools::environment::ToolEnvironment::new(),
            &selection,
        )
        .unwrap_err();
        assert!(error.contains("guard"));
    }

    fn installed_uv_path() -> String {
        let uv = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join("uv"))
            .find(|path| path.is_file())
            .expect("uv required by boundary suite");
        format!("{}:/usr/bin:/bin", uv.parent().unwrap().display())
    }

    pub(crate) async fn repaired_path_pin_recovers_without_rewriting_root_inputs() {
        for reuse in [false, true] {
            for unusable in [false, true] {
                let root = workspace();
                let pinned = root.path().join("external-python");
                std::fs::write(root.path().join(".python-version"), "./external-python").unwrap();
                std::fs::write(
                    root.path().join("pyproject.toml"),
                    "[project]\nrequires-python = \">=3\"\n",
                )
                .unwrap();
                if unusable {
                    std::fs::write(&pinned, "not executable").unwrap();
                    std::fs::set_permissions(&pinned, std::fs::Permissions::from_mode(0o600))
                        .unwrap();
                }
                if reuse {
                    let output = std::process::Command::new("/usr/bin/python3")
                        .args(["-m", "venv", "--without-pip"])
                        .arg(root.path().join(".venv"))
                        .env_remove("PYTHONEXECUTABLE")
                        .env_remove("__PYVENV_LAUNCHER__")
                        .output()
                        .unwrap();
                    assert!(output.status.success(), "{output:?}");
                }
                let original = generation_of(root.path());
                let path = if reuse {
                    "/usr/bin:/bin".to_owned()
                } else {
                    installed_uv_path()
                };
                let host = host_from_entries(vec![
                    ("PATH", &path),
                    ("UV_OFFLINE", "1"),
                    ("UV_PYTHON_DOWNLOADS", "never"),
                ]);
                let owner = Owner::for_test(
                    Arc::new(super::SupervisedProbe),
                    Some(Arc::new(host)),
                    None,
                    None,
                );
                owner.prepare(root.path().to_path_buf()).await;
                let (miss, mut arrival) = tokio::sync::oneshot::channel();
                *owner.cache_miss.lock().unwrap() = Some(miss);
                assert!(owner.observe(root.path()).virtual_env.is_none());
                assert!(matches!(
                    arrival.try_recv(),
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                ));
                if unusable {
                    std::fs::remove_file(&pinned).unwrap();
                }
                std::os::unix::fs::symlink("/usr/bin/python3", &pinned).unwrap();
                assert_eq!(
                    generation_of(root.path()),
                    original,
                    "repair only the external pin"
                );
                {
                    let mut records = owner.records.lock().unwrap();
                    let Some(Record::Unavailable { retry_after, .. }) =
                        records.get_mut(root.path())
                    else {
                        panic!("external pin failures must expire: {records:?}");
                    };
                    *retry_after = std::time::Instant::now();
                }
                owner.observe(root.path());
                arrival
                    .try_recv()
                    .expect("expired observation admits preparation again");
                owner.prepare(root.path().to_path_buf()).await;
                assert_eq!(
                    owner.observe(root.path()).virtual_env,
                    Some(root.path().join(".venv")),
                    "reuse={reuse}, unusable={unusable}: {:?}",
                    owner.records.lock().unwrap()
                );
                if reuse {
                    assert_eq!(
                        generation_of(root.path()),
                        original,
                        "adoption never rebuilds"
                    );
                }
                owner.stop().await;
            }
        }
    }

    pub(crate) async fn real_uv_missing_interpreter_obeys_download_policy() {
        for policy in ["automatic", "manual", "never"] {
            let root = workspace();
            std::fs::write(root.path().join(".python-version"), "3.12").unwrap();
            let managed = workspace();
            let cache = workspace();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mirror = format!("http://{}", listener.local_addr().unwrap());
            let (stop, mut stopped) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut requests = 0;
                loop {
                    tokio::select! {
                        _ = &mut stopped => break,
                        connection = listener.accept() => {
                            let (mut connection, _) = connection.unwrap();
                            let mut bytes = [0u8; 8192];
                            let count = connection.read(&mut bytes).await.unwrap();
                            assert!(String::from_utf8_lossy(&bytes[..count]).starts_with("GET "));
                            requests += 1;
                            connection.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                        }
                    }
                }
                requests
            });
            let path = installed_uv_path();
            let host = host_from_entries(vec![
                ("PATH", &path),
                ("UV_PYTHON_INSTALL_DIR", managed.path().to_str().unwrap()),
                ("UV_CACHE_DIR", cache.path().to_str().unwrap()),
                ("UV_PYTHON_PREFERENCE", "only-managed"),
                ("UV_PYTHON_DOWNLOADS", policy),
                ("UV_PYTHON_INSTALL_MIRROR", &mirror),
                ("UV_HTTP_RETRIES", "0"),
            ]);
            let owner = Owner::for_test(
                Arc::new(super::SupervisedProbe),
                Some(Arc::new(host)),
                None,
                None,
            );
            owner.prepare(root.path().to_path_buf()).await;
            stop.send(()).unwrap();
            let requests = server.await.unwrap();
            assert_eq!(
                requests > 0,
                policy == "automatic",
                "{policy}: download requests = {requests}"
            );
            assert!(
                matches!(
                    owner.records.lock().unwrap().get(root.path()),
                    Some(Record::Unavailable { .. })
                ),
                "{policy}"
            );
            assert!(!root.path().join(".venv").exists());
            owner.stop().await;
        }
    }

    pub(crate) async fn real_uv_does_not_seed_or_select_an_external_project() {
        let root = workspace();
        let foreign = workspace();
        std::fs::write(foreign.path().join(".python-version"), "9.99").unwrap();
        let managed = workspace();
        let cache = workspace();
        let path = installed_uv_path();
        let host = host_from_entries(vec![
            ("PATH", &path),
            ("UV_PROJECT", foreign.path().to_str().unwrap()),
            ("UV_VENV_SEED", "1"),
            ("UV_OFFLINE", "1"),
            ("UV_PYTHON_DOWNLOADS", "never"),
            ("UV_PYTHON_INSTALL_DIR", managed.path().to_str().unwrap()),
            ("UV_CACHE_DIR", cache.path().to_str().unwrap()),
        ]);
        let owner = Owner::for_test(
            Arc::new(super::SupervisedProbe),
            Some(Arc::new(host)),
            None,
            None,
        );
        owner.prepare(root.path().to_path_buf()).await;
        assert!(
            matches!(
                owner.records.lock().unwrap().get(root.path()),
                Some(Record::Ready { .. })
            ),
            "{:?}",
            owner.records.lock().unwrap()
        );
        let output = std::process::Command::new(root.path().join(".venv/bin/python"))
            .args([
                "-I",
                "-c",
                "import importlib.util; assert importlib.util.find_spec('pip') is None",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        owner.stop().await;
    }

    pub(crate) async fn real_dual_venv_launcher_variables_cannot_change_prefix() {
        let root = workspace();
        let foreign = workspace();
        for target in [root.path().join(".venv"), foreign.path().join("venv")] {
            let output = std::process::Command::new("/usr/bin/python3")
                .args(["-m", "venv", "--without-pip"])
                .arg(target)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        let foreign_python = foreign.path().join("venv/bin/python");
        for key in ["PYTHONEXECUTABLE", "__PYVENV_LAUNCHER__"] {
            for configured in [false, true] {
                let mut host_entries = vec![("PATH", "/usr/bin:/bin")];
                if !configured {
                    host_entries.push((key, foreign_python.to_str().unwrap()));
                }
                let host = Arc::new(host_from_entries(host_entries));
                let owner = Owner::for_test(
                    Arc::new(super::SupervisedProbe),
                    Some(host.clone()),
                    None,
                    None,
                );
                owner.prepare(root.path().to_path_buf()).await;
                assert!(
                    matches!(
                        owner.records.lock().unwrap().get(root.path()),
                        Some(Record::Ready { .. })
                    ),
                    "{key} host={}: {:?}",
                    !configured,
                    owner.records.lock().unwrap()
                );
                let base =
                    crate::tools::environment::ToolEnvironment::from_configured(if configured {
                        vec![(key.to_owned(), foreign_python.display().to_string())]
                    } else {
                        vec![]
                    })
                    .unwrap();
                let entries = crate::tools::execution_environment::compose(
                    &host,
                    &base,
                    &owner.observe(root.path()),
                )
                .unwrap();
                let script = "import sys; print(sys.prefix)";
                for absolute in [false, true] {
                    let mut command = if absolute {
                        let mut command =
                            std::process::Command::new(root.path().join(".venv/bin/python"));
                        command.args(["-I", "-c", script]);
                        command
                    } else {
                        let mut command = std::process::Command::new("/bin/bash");
                        command.args(["-c", "python -I -c 'import sys; print(sys.prefix)'"]);
                        command
                    };
                    let output = command
                        .env_clear()
                        .envs(entries.iter().cloned())
                        .output()
                        .unwrap();
                    assert!(output.status.success(), "{output:?}");
                    assert_eq!(
                        Path::new(String::from_utf8(output.stdout).unwrap().trim()),
                        root.path().join(".venv"),
                        "{key}, configured={configured}, absolute={absolute}"
                    );
                }
                owner.stop().await;
            }
        }
    }
    #[test]
    fn creation_requests_combine_root_requirements_without_losing_request_semantics() {
        for (pin, requires, expected) in [
            (None, None, "default"),
            (None, Some(""), "default"),
            (None, Some(">=3.13"), ">=3.13"),
            (Some("3"), Some(">=3.13"), "==3.*, >=3.13"),
            (Some("3.12"), Some(">=3.12.5"), "==3.12.*, >=3.12.5"),
            (Some("cpython-3"), Some(">=3.13"), "cpython@==3.*, >=3.13"),
            (Some("pypy@>=3.10"), Some("<3.14"), "pypy@>=3.10, <3.14"),
            (Some("3.13.0rc1"), Some(">=3"), "==3.13.0rc1, >=3"),
        ] {
            let actual = super::creation_request(Path::new("/workspace"), pin, requires).unwrap();
            // Specifier display order is maintained by pep440_rs, so compare
            // parsed constraints and the implementation, not authored order.
            let (actual_impl, actual_spec) = actual
                .split_once('@')
                .map_or((None, actual.as_str()), |(name, spec)| (Some(name), spec));
            let (expected_impl, expected_spec) = expected
                .split_once('@')
                .map_or((None, expected), |(name, spec)| (Some(name), spec));
            assert_eq!(actual_impl, expected_impl);
            if expected_spec == "default" {
                assert_eq!(actual_spec, expected_spec);
            } else {
                assert_eq!(
                    pep440_rs::VersionSpecifiers::from_str(actual_spec).unwrap(),
                    pep440_rs::VersionSpecifiers::from_str(expected_spec).unwrap()
                );
            }
        }
    }

    #[tokio::test]
    async fn invalid_requires_with_a_pin_is_rejected_before_any_probe_or_target() {
        for invalid in ["not-a-specifier", ">=>3.12"] {
            let root = workspace();
            std::fs::write(root.path().join(".python-version"), "3").unwrap();
            std::fs::write(
                root.path().join("pyproject.toml"),
                format!("[project]\nrequires-python = {invalid:?}\n"),
            )
            .unwrap();
            let probe = Arc::new(Scripted::new("0.10.2"));
            let owner = Owner::for_test(probe.clone(), None, None, None);
            owner.prepare(root.path().to_path_buf()).await;
            assert!(matches!(
                owner.records.lock().unwrap().get(root.path()),
                Some(Record::Conflict { .. })
            ));
            assert!(probe.calls.lock().unwrap().is_empty());
            assert!(!root.path().join(".venv").exists());
        }
    }

    pub(crate) async fn targets_added_while_creation_waits_for_capacity_are_never_initialized() {
        for kind in ["symlink", "directory", "file"] {
            let root = workspace();
            let external = workspace();
            let path = installed_uv_path();
            let host = host_from_entries(vec![
                ("PATH", &path),
                ("UV_OFFLINE", "1"),
                ("UV_PYTHON_DOWNLOADS", "never"),
            ]);
            let owner = Arc::new(Owner::for_test(
                Arc::new(super::SupervisedProbe),
                Some(Arc::new(host)),
                None,
                None,
            ));
            let (entered, release) = arm_gate(&owner.before_create);
            let preparing = owner.clone();
            let workspace = root.path().to_path_buf();
            let task = tokio::spawn(async move { preparing.prepare(workspace).await });
            entered.await.unwrap();
            let held = owner.permits.acquire_many(2).await.unwrap();
            let (waiting, blocked) = tokio::sync::oneshot::channel();
            *owner.waiting_permit.lock().unwrap() = Some(waiting);
            release.send(()).unwrap();
            blocked
                .await
                .expect("create is queued behind the held permits");
            let target = root.path().join(".venv");
            match kind {
                "symlink" => std::os::unix::fs::symlink(external.path(), &target).unwrap(),
                "directory" => std::fs::create_dir(&target).unwrap(),
                _ => std::fs::write(&target, "preserve me").unwrap(),
            }
            drop(held);
            tokio::time::timeout(std::time::Duration::from_secs(10), task)
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(
                    owner.records.lock().unwrap().get(root.path()),
                    Some(Record::Conflict { .. })
                ),
                "{kind}"
            );
            assert_eq!(
                std::fs::read_dir(external.path()).unwrap().count(),
                0,
                "external target must remain empty"
            );
            if kind == "directory" {
                assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
            }
            if kind == "file" {
                assert_eq!(std::fs::read_to_string(&target).unwrap(), "preserve me");
            }
            owner.stop().await;
        }
    }

    fn python_version(program: &Path) -> Version {
        let output = std::process::Command::new(program)
            .args([
                "-I",
                "-c",
                "import platform; print(platform.python_version())",
            ])
            .env_remove("PYTHONEXECUTABLE")
            .env_remove("__PYVENV_LAUNCHER__")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        Version::from_str(String::from_utf8(output.stdout).unwrap().trim()).unwrap()
    }

    fn python_base_executable(program: &Path) -> PathBuf {
        let output = std::process::Command::new(program)
            .args([
                "-I",
                "-c",
                "import os, sys; print(os.path.realpath(sys._base_executable))",
            ])
            .env_remove("PYTHONEXECUTABLE")
            .env_remove("__PYVENV_LAUNCHER__")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    }

    fn installed_python_interpreters() -> Vec<(Version, PathBuf)> {
        // Enumeration supplies fixture executables only. The selection under
        // test goes through Owner::prepare and the production supervisor.
        let installed = std::process::Command::new("uv")
            .args([
                "python",
                "list",
                "--only-installed",
                "--output-format",
                "json",
            ])
            .env("UV_OFFLINE", "1")
            .output()
            .unwrap();
        assert!(installed.status.success(), "{installed:?}");
        let listed: Vec<serde_json::Value> = serde_json::from_slice(&installed.stdout).unwrap();
        let mut interpreters: Vec<_> = listed
            .iter()
            .filter(|entry| entry["implementation"] == "cpython" && entry["variant"] == "default")
            .filter_map(|entry| {
                Some((
                    Version::from_str(entry["version"].as_str()?).ok()?,
                    PathBuf::from(entry["path"].as_str()?),
                ))
            })
            .filter(|(version, _)| version.release()[0] == 3 && !version.any_prerelease())
            .collect();
        interpreters.sort_by(|a, b| a.0.cmp(&b.0));
        interpreters
    }

    pub(crate) async fn launcher_selection_change_between_probes_cannot_publish_ready() {
        let interpreters = installed_python_interpreters();
        let first = python_base_executable(&interpreters.first().expect("two Python minors").1);
        let second = python_base_executable(&interpreters.last().unwrap().1);
        assert_ne!(first, second, "two real Python interpreters required");
        let root = workspace();
        let target = root.path().join(".venv");
        std::fs::create_dir_all(target.join("bin")).unwrap();
        std::fs::write(
            target.join("pyvenv.cfg"),
            "include-system-site-packages = false\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&first, target.join("bin/python")).unwrap();
        std::os::unix::fs::symlink(&second, target.join("bin/python3")).unwrap();
        let selector = root.path().join("selector");
        std::fs::write(&selector, format!("{}\n", first.display())).unwrap();
        let launcher = root.path().join("launcher");
        write_executable(
            &launcher,
            "#!/bin/sh\nread -r selected < \"$PATH_PIN_SELECTOR\"\nexec \"$selected\" \"$@\"\n",
        );
        std::fs::write(root.path().join(".python-version"), "./launcher").unwrap();
        let original = generation_of(root.path());
        let paths = [&launcher, &first, &second];
        let stamps = paths.map(|path| super::stamp(path));
        let host = host_from_entries(vec![
            ("PATH", "/usr/bin:/bin"),
            ("PATH_PIN_SELECTOR", selector.to_str().unwrap()),
        ]);
        let owner = Arc::new(Owner::for_test(
            Arc::new(super::SupervisedProbe),
            Some(Arc::new(host)),
            None,
            None,
        ));
        let (entered, release) = arm_gate(&owner.after_interpreter);
        let preparing = owner.clone();
        let path = root.path().to_path_buf();
        let task = tokio::spawn(async move { preparing.prepare(path).await });
        tokio::time::timeout(super::PROBE_TIMEOUT * 4, entered)
            .await
            .expect("first-candidate validation reaches its gate")
            .expect("the first candidate and fixed launcher were validated");
        std::fs::write(&selector, format!("{}\n", second.display())).unwrap();
        release.send(()).unwrap();
        task.await.unwrap();
        assert!(
            matches!(owner.records.lock().unwrap().get(root.path()), Some(Record::Unavailable { diagnostic, .. }) if diagnostic.contains("changed during validation"))
        );
        assert!(owner.observe(root.path()).virtual_env.is_none());
        assert_eq!(generation_of(root.path()), original);
        assert_eq!(
            paths.map(|path| super::stamp(path)),
            stamps,
            "all recorded metadata stayed unchanged"
        );
        owner.stop().await;
    }

    pub(crate) async fn real_multi_interpreter_selection_satisfies_both_root_inputs() {
        let interpreters = installed_python_interpreters();
        let (low_version, low) = interpreters
            .first()
            .expect("two installed Python minors required");
        let (high_version, high) = interpreters.last().unwrap();
        assert!(
            low_version.release()[1] < high_version.release()[1],
            "the boundary requires two real Python minors: {interpreters:?}"
        );
        let fixture = workspace();
        let first = fixture.path().join("first");
        let second = fixture.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        std::os::unix::fs::symlink(low, first.join("python3")).unwrap();
        std::os::unix::fs::symlink(high, second.join("python3")).unwrap();
        let uv_path = installed_uv_path();
        let uv_bin = uv_path.split(':').next().unwrap();
        let path = format!("{}:{}:{uv_bin}", first.display(), second.display());
        let managed = fixture.path().join("managed");
        std::fs::create_dir(&managed).unwrap();
        let host = Arc::new(host_from_entries(vec![
            ("PATH", &path),
            ("UV_PYTHON_INSTALL_DIR", managed.to_str().unwrap()),
            ("UV_PYTHON_PREFERENCE", "only-system"),
            ("UV_OFFLINE", "1"),
            ("UV_PYTHON_DOWNLOADS", "never"),
        ]));
        // Prove that a pin alone chooses the first (incompatible) interpreter.
        let control = fixture.path().join("pin-only");
        let output = std::process::Command::new(Path::new(uv_bin).join("uv"))
            .args(["venv", "--no-project", "--python", "3"])
            .arg(&control)
            .env_clear()
            .envs(crate::tools::execution_environment::bootstrap_environment(
                &host,
            ))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(python_version(&control.join("bin/python")), *low_version);
        let requires = format!(">=3.{}", high_version.release()[1]);
        for pin in ["3", "cpython-3", "cpython@>=3"] {
            let root = workspace();
            std::fs::write(root.path().join(".python-version"), pin).unwrap();
            std::fs::write(
                root.path().join("pyproject.toml"),
                format!("[project]\nrequires-python = {requires:?}\n"),
            )
            .unwrap();
            let owner = Owner::for_test(
                Arc::new(super::SupervisedProbe),
                Some(host.clone()),
                None,
                None,
            );
            owner.prepare(root.path().to_path_buf()).await;
            assert!(
                matches!(
                    owner.records.lock().unwrap().get(root.path()),
                    Some(Record::Ready { .. })
                ),
                "{pin}: {:?}",
                owner.records.lock().unwrap()
            );
            assert_eq!(
                python_version(&root.path().join(".venv/bin/python")),
                *high_version
            );
            owner.stop().await;
        }
        // A fixed path cannot be made compatible by selecting another Python.
        // A disjoint version conjunction must also fail without a target.
        for pin in [
            low.display().to_string(),
            format!("3.{}", low_version.release()[1]),
        ] {
            let root = workspace();
            std::fs::write(root.path().join(".python-version"), &pin).unwrap();
            std::fs::write(
                root.path().join("pyproject.toml"),
                format!("[project]\nrequires-python = {requires:?}\n"),
            )
            .unwrap();
            let owner = Owner::for_test(
                Arc::new(super::SupervisedProbe),
                Some(host.clone()),
                None,
                None,
            );
            owner.prepare(root.path().to_path_buf()).await;
            assert!(
                matches!(
                    owner.records.lock().unwrap().get(root.path()),
                    Some(Record::Conflict { .. } | Record::Unavailable { .. })
                ),
                "{pin}"
            );
            assert!(!root.path().join(".venv").exists());
            owner.stop().await;
        }
    }
    #[tokio::test]
    async fn replaced_launcher_base_is_validated_before_creation() {
        struct LauncherProbe {
            scripted: Scripted,
            launcher: PathBuf,
            executable: PathBuf,
            gate: Mutex<Option<super::TestGate>>,
        }
        impl ProgramProbe for LauncherProbe {
            fn run(&self, request: ProgramRequest) -> BoxFuture<'_, Result<ProgramOutput, String>> {
                Box::pin(async move {
                    let program = Path::new(&request.argv[0]);
                    if program == self.launcher || program == self.executable {
                        let version = std::fs::read_to_string(&self.executable).unwrap();
                        // Capture the old launcher's metadata before the gate;
                        // the executable pathname is replaced while it waits.
                        let stdout = format!(
                            "/usr\n/usr\n{}\ncpython\n{}\n",
                            version.trim(),
                            self.executable.display()
                        );
                        if program == self.launcher {
                            super::pass_gate(&self.gate).await;
                        }
                        return Ok(ProgramOutput {
                            status: 0,
                            stdout,
                            stderr: String::new(),
                        });
                    }
                    self.scripted.run(request).await
                })
            }
        }
        let root = workspace();
        let launcher = root.path().join("launcher");
        let executable = root.path().join("base-python");
        write_executable(&launcher, "#!/bin/sh\nexit 0\n");
        write_executable(&executable, "3.12.3");
        std::fs::write(root.path().join(".python-version"), "./launcher").unwrap();
        std::fs::write(
            root.path().join("pyproject.toml"),
            "[project]\nrequires-python = \">=3.12\"\n",
        )
        .unwrap();
        let bin = workspace();
        write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
        let probe = Arc::new(LauncherProbe {
            scripted: Scripted::new("0.10.2"),
            launcher,
            executable: executable.clone(),
            gate: Mutex::new(None),
        });
        let (entered, release) = arm_gate(&probe.gate);
        let owner = Arc::new(Owner::for_test(
            probe.clone(),
            Some(host_for(bin.path())),
            None,
            None,
        ));
        let preparing = owner.clone();
        let path = root.path().to_path_buf();
        let task = tokio::spawn(async move { preparing.prepare(path).await });
        entered.await.unwrap();
        std::fs::rename(&executable, root.path().join("old-base")).unwrap();
        write_executable(&executable, "3.11.0");
        release.send(()).unwrap();
        task.await.unwrap();
        assert!(matches!(
            owner.records.lock().unwrap().get(root.path()),
            Some(Record::Unavailable { .. })
        ));
        assert!(!root.path().join(".venv").exists());
        assert!(
            !probe
                .scripted
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|argv| argv.iter().any(|arg| arg == "venv"))
        );
        owner.stop().await;
    }

    #[tokio::test]
    async fn changed_fixed_interpreter_during_capacity_wait_does_not_create_a_target() {
        for (directory, change) in [
            (false, "entry"),
            (false, "target"),
            (true, "entry"),
            (true, "target"),
            (true, "preferred"),
        ] {
            let root = workspace();
            let base = root.path().join("base-python");
            let other = root.path().join("other-python");
            write_executable(&base, "#!/bin/sh\nexit 0\n");
            write_executable(&other, "#!/bin/sh\nexit 0\n");
            let pin = root.path().join("pinned-python");
            let alias = if directory {
                std::fs::create_dir_all(pin.join("bin")).unwrap();
                pin.join(if change == "preferred" {
                    "bin/python3"
                } else {
                    "bin/python"
                })
            } else {
                pin
            };
            std::os::unix::fs::symlink(&base, &alias).unwrap();
            std::fs::write(root.path().join(".python-version"), "./pinned-python").unwrap();
            let bin = workspace();
            write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
            let probe = Arc::new(Scripted::new("0.10.2"));
            let owner = Arc::new(Owner::for_test(
                probe.clone(),
                Some(host_for(bin.path())),
                None,
                None,
            ));
            let (entered, release) = arm_gate(&owner.before_create);
            let preparing = owner.clone();
            let path = root.path().to_path_buf();
            let task = tokio::spawn(async move { preparing.prepare(path).await });
            entered.await.unwrap();
            let held = owner.permits.acquire_many(2).await.unwrap();
            let (waiting, blocked) = tokio::sync::oneshot::channel();
            *owner.waiting_permit.lock().unwrap() = Some(waiting);
            release.send(()).unwrap();
            blocked.await.unwrap();
            if change == "preferred" {
                std::os::unix::fs::symlink(&base, root.path().join("pinned-python/bin/python"))
                    .unwrap();
            } else if change == "entry" {
                std::fs::rename(&alias, root.path().join("old-alias")).unwrap();
                std::os::unix::fs::symlink(&other, &alias).unwrap();
            } else {
                std::fs::rename(&base, root.path().join("old-base")).unwrap();
                write_executable(&base, "#!/bin/sh\nexit 0\n");
            }
            drop(held);
            task.await.unwrap();
            assert!(!root.path().join(".venv").exists());
            assert!(owner.records.lock().unwrap().get(root.path()).is_none());
            assert!(
                !probe
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|argv| argv.iter().any(|arg| arg == "venv"))
            );
        }
    }
}
