//! One default uv-created Python environment per Workspace.
//!
//! Native Agent Bash, Workflow Bash, and Subagent Bash observe this owner.
//! Manual terminals, generic command execution, MCP, and Managed Python do not.
//! Registration and session creation schedule preparation and do not wait for it.
//! An existing environment is reused or diagnosed; it is never deleted or rebuilt.

use std::collections::HashMap;
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
use pep440_rs::{Version, VersionSpecifiers};
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    dev: u64,
    ino: u64,
    mtime: i64,
    mtime_nsec: i64,
    len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Generation {
    root_dev: u64,
    root_ino: u64,
    pin_file: Option<FileStamp>,
    project_file: Option<FileStamp>,
    python: Option<FileStamp>,
    python3: Option<FileStamp>,
}

#[derive(Clone, Debug)]
enum Record {
    Ready {
        generation: Generation,
        bin: PathBuf,
        virtual_env: PathBuf,
        guard: PathBuf,
    },
    Unavailable {
        generation: Generation,
        diagnostic: String,
        retry_after: Instant,
    },
    Conflict {
        generation: Generation,
        diagnostic: String,
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

struct Owner {
    records: Mutex<HashMap<PathBuf, Record>>,
    flights: Mutex<HashMap<PathBuf, Arc<Flight>>>,
    permits: tokio::sync::Semaphore,
    shutdown: tokio_util::sync::CancellationToken,
    probe: Arc<dyn ProgramProbe>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    signals: Mutex<Vec<crate::runtime::cancellation::CancellationSignal>>,
    host_override: Option<Arc<ExecutionHost>>,
    state_override: Option<PathBuf>,
    schedules: bool,
    #[cfg(test)]
    before_lock: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

fn global() -> &'static Owner {
    static OWNER: OnceLock<Owner> = OnceLock::new();
    OWNER.get_or_init(Owner::production)
}

impl Owner {
    fn production() -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
            flights: Mutex::new(HashMap::new()),
            permits: tokio::sync::Semaphore::new(EXTERNAL_PERMITS),
            shutdown: tokio_util::sync::CancellationToken::new(),
            probe: production_probe(),
            tasks: Mutex::new(Vec::new()),
            signals: Mutex::new(Vec::new()),
            host_override: None,
            state_override: None,
            schedules: true,
            #[cfg(test)]
            before_lock: Mutex::new(None),
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
            }) if stored == &generation => {
                return PythonSelection {
                    path_prefix: vec![bin.clone(), guard.clone()],
                    virtual_env: Some(virtual_env.clone()),
                    diagnostic: String::new(),
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
            }) if stored == &generation => {
                return guarded(diagnostic);
            }
            _ => {}
        }
        if self.schedules {
            self.schedule(canonical);
        }
        let diagnostic = record
            .map(|record| record.diagnostic())
            .filter(|diagnostic| !diagnostic.is_empty())
            .unwrap_or_else(|| "Workspace Python is not ready".to_owned());
        guarded(&diagnostic)
    }

    fn schedule(&self, root: PathBuf) {
        let Some(handle) = tokio::runtime::Handle::try_current().ok() else {
            return;
        };
        let task = handle.spawn(async move {
            global().prepare(root).await;
        });
        self.tasks
            .lock()
            .expect("workspace python tasks")
            .push(task);
    }

    async fn prepare(&self, root: PathBuf) {
        let Ok(canonical) = std::fs::canonicalize(&root) else {
            self.unavailable(
                &root,
                "the workspace directory is unavailable",
                Generation {
                    root_dev: 0,
                    root_ino: 0,
                    pin_file: None,
                    project_file: None,
                    python: None,
                    python3: None,
                },
            );
            return;
        };
        let (flight, leader) = {
            let mut flights = self.flights.lock().expect("workspace python flights");
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
        self.prepare_locked(&canonical).await;
        self.flights
            .lock()
            .expect("workspace python flights")
            .remove(&canonical);
        flight.finish();
    }

    async fn prepare_locked(&self, root: &Path) {
        if self.shutdown.is_cancelled() {
            return;
        }
        let lock = match acquire_lock(root, self.before_lock(), self.state_override.clone()).await {
            Ok(lock) => lock,
            Err(message) => {
                self.unavailable(root, &message, generation_of(root));
                return;
            }
        };
        let pin = read_pin(root);
        let requires = read_requires(root);
        if matches!(requires, Requires::Invalid) {
            self.conflict(
                root,
                "requires-python cannot be parsed",
                generation_of(root),
            );
            drop(lock);
            return;
        }
        let requires = match &requires {
            Requires::Spec(spec) => Some(spec.as_str()),
            Requires::Absent | Requires::Invalid => None,
        };
        if let (Some(pin), Some(requires)) = (pin.as_deref(), requires)
            && exact_pin_conflicts(pin, requires)
        {
            self.conflict(
                root,
                ".python-version conflicts with requires-python",
                generation_of(root),
            );
            drop(lock);
            return;
        }
        let target = root.join(".venv");
        match classify_target(&target) {
            Target::Absent => {
                self.create(root, pin.as_deref(), requires).await;
            }
            Target::Directory => {
                self.adopt(root, &target, pin.as_deref(), requires).await;
            }
            Target::Conflict(diagnostic) => {
                self.conflict(root, diagnostic, generation_of(root));
            }
        }
        drop(lock);
    }

    async fn create(&self, root: &Path, pin: Option<&str>, requires: Option<&str>) {
        let host = self.host();
        if host.closed().is_some() {
            self.unavailable(root, "PATH or HOME is not Unicode", generation_of(root));
            return;
        }
        let Some(uv) = find_uv(host.as_ref(), root) else {
            self.unavailable(
                root,
                "uv was not found on the host PATH",
                generation_of(root),
            );
            return;
        };
        let version = match self.uv_version(root, &uv, host.as_ref()).await {
            Ok(version) => version,
            Err(message) => {
                self.unavailable(root, &message, generation_of(root));
                return;
            }
        };
        if !uv_can_create(&version) {
            self.unavailable(
                root,
                "uv is older than 0.10.0 and cannot create a Workspace environment",
                generation_of(root),
            );
            return;
        }
        let interpreter = match self
            .find_interpreter(root, &uv, host.as_ref(), pin, requires)
            .await
        {
            Ok(path) => path,
            Err(message) => {
                self.unavailable(root, &message, generation_of(root));
                return;
            }
        };
        if let Err(message) = self
            .check_interpreter(root, &interpreter, host.as_ref(), pin, requires, false)
            .await
        {
            self.conflict(root, message, generation_of(root));
            return;
        }
        if classify_target(&root.join(".venv")) != Target::Absent {
            self.adopt(root, &root.join(".venv"), pin, requires).await;
            return;
        }
        let target = root.join(".venv");
        let argv = vec![
            uv.display().to_string(),
            "venv".to_owned(),
            "--no-project".to_owned(),
            "--python".to_owned(),
            interpreter.display().to_string(),
            target.display().to_string(),
        ];
        if let Err(message) = self.run(root, argv, host.as_ref(), CREATE_TIMEOUT).await {
            self.unavailable(root, &message, generation_of(root));
            return;
        }
        self.adopt(root, &target, pin, requires).await;
    }

    async fn adopt(&self, root: &Path, target: &Path, pin: Option<&str>, requires: Option<&str>) {
        let host = self.host();
        for name in ["python", "python3"] {
            let program = target.join("bin").join(name);
            if let Err(message) = self
                .check_interpreter(root, &program, host.as_ref(), pin, requires, true)
                .await
            {
                self.conflict(root, message, generation_of(root));
                return;
            }
        }
        let generation = generation_of(root);
        let Some(guard) = ensure_guard("Workspace Python is no longer available") else {
            self.unavailable(
                root,
                "the Workspace Python guard could not be installed",
                generation,
            );
            return;
        };
        // A changed generation must not publish Ready and must not cool down
        // the new inputs. The next native use revalidates them.
        if generation_of(root) != generation {
            return;
        }
        self.records
            .lock()
            .expect("workspace python records")
            .insert(
                root.to_path_buf(),
                Record::Ready {
                    generation,
                    bin: target.join("bin"),
                    virtual_env: target.to_path_buf(),
                    guard,
                },
            );
    }

    async fn uv_version(
        &self,
        root: &Path,
        uv: &Path,
        host: &ExecutionHost,
    ) -> Result<String, String> {
        let output = self
            .run(
                root,
                vec![uv.display().to_string(), "--version".to_owned()],
                host,
                PROBE_TIMEOUT,
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

    async fn find_interpreter(
        &self,
        root: &Path,
        uv: &Path,
        host: &ExecutionHost,
        pin: Option<&str>,
        requires: Option<&str>,
    ) -> Result<PathBuf, String> {
        let mut argv = vec![
            uv.display().to_string(),
            "python".to_owned(),
            "find".to_owned(),
            "--no-project".to_owned(),
        ];
        // The root pin is the request. With no pin, requires-python constrains
        // uv's selection. Ancestor projects are not discovered: `--no-project`
        // is set and only the Workspace root is read.
        if let Some(pin) = pin {
            argv.push(pin.to_owned());
        } else if let Some(requires) = requires {
            argv.push(requires.to_owned());
        }
        let output = self.run(root, argv, host, CREATE_TIMEOUT).await?;
        if output.status != 0 {
            return Err(bounded_diagnostic(
                "uv could not select a Python interpreter",
                &output.stderr,
            ));
        }
        let path = output.stdout.lines().next().unwrap_or("").trim();
        if path.is_empty() {
            return Err("uv did not report a Python interpreter".to_owned());
        }
        Ok(PathBuf::from(path))
    }

    async fn check_interpreter(
        &self,
        root: &Path,
        program: &Path,
        host: &ExecutionHost,
        pin: Option<&str>,
        requires: Option<&str>,
        require_venv: bool,
    ) -> Result<(), &'static str> {
        let output = self
            .run(
                root,
                vec![
                    program.display().to_string(),
                    "-I".to_owned(),
                    "-c".to_owned(),
                    "import sys; print(sys.prefix); print(sys.base_prefix); print('%d.%d.%d' % sys.version_info[:3])".to_owned(),
                ],
                host,
                PROBE_TIMEOUT,
            )
            .await
            .map_err(|_| "the Python interpreter could not be probed")?;
        if output.status != 0 {
            return Err("the Python interpreter could not be probed");
        }
        let mut lines = output.stdout.lines();
        let prefix = lines.next().unwrap_or("").trim();
        let base = lines.next().unwrap_or("").trim();
        let version_text = lines.next().unwrap_or("").trim();
        if require_venv {
            let venv = root.join(".venv");
            let Ok(actual) = std::fs::canonicalize(prefix) else {
                return Err("Workspace .venv is not a valid virtual environment");
            };
            let Ok(expected) = std::fs::canonicalize(&venv) else {
                return Err("Workspace .venv is not a valid virtual environment");
            };
            if actual != expected || prefix == base {
                return Err("Workspace .venv is not a valid virtual environment");
            }
        }
        let version = Version::from_str(version_text)
            .map_err(|_| "the Python version could not be verified")?;
        if let Some(requires) = requires {
            let specs = VersionSpecifiers::from_str(requires)
                .map_err(|_| "requires-python cannot be parsed")?;
            if !specs.contains(&version) {
                return Err("Workspace Python does not satisfy requires-python");
            }
        }
        if let Some(pin) = pin {
            if path_request(pin) {
                let requested = if Path::new(pin).is_absolute() {
                    PathBuf::from(pin)
                } else {
                    root.join(pin)
                };
                let Ok(actual) = std::fs::canonicalize(program) else {
                    return Err("Workspace Python does not satisfy .python-version");
                };
                let Ok(expected) = std::fs::canonicalize(&requested) else {
                    return Err("Workspace Python does not satisfy .python-version");
                };
                if actual != expected {
                    return Err("Workspace Python does not satisfy .python-version");
                }
            } else if !pin_matches(pin, &version) {
                return Err("Workspace Python does not satisfy .python-version");
            }
        }
        Ok(())
    }

    async fn run(
        &self,
        root: &Path,
        argv: Vec<String>,
        host: &ExecutionHost,
        timeout: Duration,
    ) -> Result<ProgramOutput, String> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| "Workspace Python preparation stopped".to_owned())?;
        if self.shutdown.is_cancelled() {
            return Err("Workspace Python preparation stopped".to_owned());
        }
        let signal = crate::runtime::cancellation::CancellationSignal::new();
        self.signals
            .lock()
            .expect("workspace python signals")
            .push(signal.clone());
        let request = ProgramRequest {
            argv,
            cwd: root.to_path_buf(),
            env: execution_environment::bootstrap_environment(host),
            timeout,
            cancellation: signal,
        };
        self.probe.run(request).await
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
    }

    fn conflict(&self, root: &Path, diagnostic: &str, generation: Generation) {
        self.records
            .lock()
            .expect("workspace python records")
            .insert(
                root.to_path_buf(),
                Record::Conflict {
                    generation,
                    diagnostic: diagnostic.to_owned(),
                },
            );
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

#[cfg(not(test))]
struct SupervisedProbe;

#[cfg(not(test))]
impl ProgramProbe for SupervisedProbe {
    fn run(&self, request: ProgramRequest) -> BoxFuture<'_, Result<ProgramOutput, String>> {
        Box::pin(async move {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(&request.cwd).map_err(|error| error.to_string())?;
            let invocation = crate::runtime::process_runner::ProgramInvocation {
                argv: request.argv,
                stdin_socket: None,
                terminal: None,
                cwd_identity: (metadata.dev(), metadata.ino()),
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
pub(crate) fn schedule(root: PathBuf) {
    global().schedule(root);
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
    global().shutdown.cancel();
    for signal in global()
        .signals
        .lock()
        .expect("workspace python signals")
        .drain(..)
    {
        signal.cancel();
    }
    join_preparation().await;
}

async fn join_preparation() {
    let tasks = std::mem::take(&mut *global().tasks.lock().expect("workspace python tasks"));
    for task in tasks {
        let _ = task.await;
    }
}

fn scan_admit(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.take(32).flatten() {
        let path = entry.path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let location = text.trim();
        if Path::new(location).is_absolute() {
            schedule(PathBuf::from(location));
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
        python: stamp(&root.join(".venv/bin/python")),
        python3: stamp(&root.join(".venv/bin/python3")),
    }
}

fn stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(FileStamp {
        dev: meta.dev(),
        ino: meta.ino(),
        mtime: meta.mtime(),
        mtime_nsec: meta.mtime_nsec(),
        len: meta.len(),
    })
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

fn read_pin(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(".python-version")).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        return Some(line.to_owned());
    }
    None
}

fn read_requires(root: &Path) -> Requires {
    let path = root.join("pyproject.toml");
    if !path.exists() {
        return Requires::Absent;
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Requires::Invalid;
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

fn uv_can_create(text: &str) -> bool {
    let Ok(version) = Version::from_str(text) else {
        return false;
    };
    let Ok(minimum) = Version::from_str(UV_MINIMUM) else {
        return false;
    };
    version >= minimum
}

fn exact_pin_conflicts(pin: &str, requires: &str) -> bool {
    let Some(version) = extracted_version(pin) else {
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

fn pin_matches(pin: &str, actual: &Version) -> bool {
    if pin
        .chars()
        .any(|c| matches!(c, '>' | '<' | '=' | '~' | ','))
    {
        return VersionSpecifiers::from_str(pin).is_ok_and(|specs| specs.contains(actual));
    }
    let Some(requested) = extracted_version(pin) else {
        return false;
    };
    if requested.release().len() >= 3 {
        requested.release() == actual.release()
    } else {
        actual.release().starts_with(requested.release())
    }
}

fn extracted_version(pin: &str) -> Option<Version> {
    pin.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '!' || c == '+'))
        .find_map(|token| Version::from_str(token).ok())
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
    PythonSelection {
        path_prefix: ensure_guard(diagnostic).into_iter().collect(),
        virtual_env: None,
        diagnostic: diagnostic.to_owned(),
    }
}

fn ensure_guard(diagnostic: &str) -> Option<PathBuf> {
    let directory = state_dir()?.join("guards").join(guard_name(diagnostic));
    std::fs::create_dir_all(&directory).ok()?;
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' '{}' >&2\nexit 127\n",
        shell_line(diagnostic)
    );
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
        file.write_all(script.as_bytes()).ok()?;
    }
    Some(directory)
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
) -> Result<nix::fcntl::Flock<std::fs::File>, String> {
    let state = state_override
        .or_else(state_dir)
        .ok_or_else(|| "HOME is not available for Workspace Python coordination".to_owned())?;
    let directory = state.join("locks");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = lock_file(&state, root);
    if let Some(sender) = before_lock {
        let _ = sender.send(());
    }
    tokio::task::spawn_blocking(move || {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|error| error.to_string())?;
        nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive)
            .map_err(|(_, error)| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::{
        Owner, ProgramOutput, ProgramProbe, ProgramRequest, Record, ensure_guard,
        exact_pin_conflicts, generation_of, lock_file, pin_matches, scan_admit, uv_can_create,
    };
    use crate::tools::execution_environment::{ExecutionHost, host_from_entries};
    use pep440_rs::Version;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    struct Scripted {
        calls: Mutex<Vec<Vec<String>>>,
        version: String,
        envs: Mutex<Vec<Vec<(String, String)>>>,
    }

    impl Scripted {
        fn new(version: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                version: version.to_owned(),
                envs: Mutex::new(Vec::new()),
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
                let argv = &request.argv;
                if argv.iter().any(|arg| arg == "--version") {
                    return Ok(ProgramOutput {
                        status: 0,
                        stdout: format!("uv {}\n", self.version),
                        stderr: String::new(),
                    });
                }
                if argv.iter().any(|arg| arg == "find") {
                    if let Some(request) = argv.last() {
                        let requested = PathBuf::from(request);
                        if requested.is_absolute() && requested.is_file() {
                            return Ok(ProgramOutput {
                                status: 0,
                                stdout: format!("{}\n", requested.display()),
                                stderr: String::new(),
                            });
                        }
                    }
                    let base = request.cwd.join("base-python");
                    write_executable(&base, "#!/bin/sh\nexit 0\n");
                    return Ok(ProgramOutput {
                        status: 0,
                        stdout: format!("{}\n", base.display()),
                        stderr: String::new(),
                    });
                }
                if argv.iter().any(|arg| arg == "venv") {
                    let target = PathBuf::from(argv.last().expect("target"));
                    let interpreter = argv
                        .iter()
                        .position(|arg| arg == "--python")
                        .map(|index| PathBuf::from(&argv[index + 1]));
                    if let Some(interpreter) = interpreter {
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
                            stdout: format!("{}\n/usr\n3.12.3\n", venv.display()),
                            stderr: String::new(),
                        });
                    }
                    return Ok(ProgramOutput {
                        status: 0,
                        stdout: "/usr\n/usr\n3.12.3\n".to_owned(),
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

    fn workspace() -> tempfile::TempDir {
        tempfile::tempdir().expect("workspace")
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
        assert!(pin_matches("cpython-3.12.3-linux", &actual));
        assert!(!pin_matches("3.11", &actual));
        assert!(!pin_matches("3.12.4", &actual));
        assert!(exact_pin_conflicts("3.11.0", ">=3.12"));
        assert!(!exact_pin_conflicts("3.12", ">=3.12.1"));
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
        assert!(argv.iter().any(|arg| arg == "--python"));
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
    async fn issue_u07_unpinned_requires_python_is_the_find_request() {
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
            .find(|argv| argv.iter().any(|arg| arg == "find"))
            .expect("find");
        assert!(find.iter().any(|arg| arg == "--no-project"));
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
            .find(|argv| argv.iter().any(|arg| arg == "find"))
            .expect("find");
        assert_eq!(
            find.last().map(String::as_str),
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
}
