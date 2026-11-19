//! Workspace Python owner state, gates, locks, and environment preparation.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use sha2::{Digest, Sha256};

use super::guard::{
    bounded_diagnostic, ensure_guard, fallback_guard, guarded, validated_guard_directory,
};
use super::interpreter::{
    FixedInterpreter, InterpreterError, Requires, creation_request, exact_pin_conflicts, find_uv,
    isolated_discovery_args, parse_request, path_request, read_pin, read_requires, uv_can_create,
};
#[cfg(test)]
use super::pass_gate;
use super::{
    FileStamp, Flight, Generation, Owner, PathStamp, ProgramOutput, ProgramProbe, ProgramRequest,
    PythonSelection, Record,
};
use crate::tools::execution_environment::{self, ExecutionHost};

const CREATE_TIMEOUT: Duration = Duration::from_mins(10);
pub(super) const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
const RETRY_COOLDOWN: Duration = Duration::from_secs(30);
const EXTERNAL_PERMITS: usize = 2;

pub(super) fn global() -> &'static Arc<Owner> {
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
    pub(super) fn for_test(
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

    pub(super) fn observe(&self, root: &Path) -> PythonSelection {
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

    pub(super) fn schedule_on(self: &Arc<Self>, root: &Path) {
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

    pub(super) async fn prepare(&self, root: PathBuf) {
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
}

impl Owner {
    pub(super) async fn run(
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

pub(super) struct SupervisedProbe;

impl ProgramProbe for SupervisedProbe {
    fn run(&self, request: ProgramRequest) -> BoxFuture<'_, Result<ProgramOutput, String>> {
        Box::pin(async move {
            let invocation = rustx_process_supervisor::invocation::ProgramInvocation {
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

impl Owner {
    pub(super) async fn stop(&self) {
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

#[cfg(test)]
#[allow(clippy::unnecessary_wraps)] // production returns None when HOME is missing
pub(super) fn state_dir() -> Option<PathBuf> {
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
pub(super) fn state_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if !home.is_absolute() {
        return None;
    }
    Some(home.join("rustx").join("runtime").join("workspace-python"))
}

pub(super) fn generation_of(root: &Path) -> Generation {
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

pub(super) fn stamp(path: &Path) -> Option<PathStamp> {
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

pub(super) fn lock_file(state: &Path, root: &Path) -> PathBuf {
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
