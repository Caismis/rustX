use super::guard::{
    GUARD_PYTHON, GUARD_PYTHON3, bounded_diagnostic, ensure_guard, guard_beside_executable,
    install_guard, provisioned_guard_for, selection_from_guard,
};
use super::host::{self as execution_environment, ExecutionHost, host_from_entries};
use super::interpreter::{
    FixedInterpreter, ParsedRequest, absolute_pin, creation_request, exact_pin_conflicts,
    parse_request, path_request, pin_matches, read_pin, uv_can_create,
};
use super::owner::{SupervisedProbe, generation_of, lock_file, stamp};
use super::{
    Owner, ProgramOutput, ProgramProbe, ProgramRequest, PythonSelection, Record, TestGate,
    pass_gate,
};
use futures_util::future::BoxFuture;
use pep440_rs::Version;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
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

impl RecordingSupervisedProbe {
    fn assert_invoked(&self, program: &Path) {
        let calls = self.calls.lock().unwrap();
        assert!(
            calls.iter().any(|argv| Path::new(&argv[0]) == program),
            "missing invocation of {program:?}: {calls:?}"
        );
    }
}

impl ProgramProbe for RecordingSupervisedProbe {
    fn run(&self, request: ProgramRequest) -> BoxFuture<'_, Result<ProgramOutput, String>> {
        self.calls.lock().unwrap().push(request.argv.clone());
        Box::pin(async move {
            let program = request.argv[0].clone();
            let result = SupervisedProbe.run(request).await;
            match &result {
                Ok(output) if output.status != 0 => eprintln!(
                    "Workspace Python fixture {program} exited {}: {}",
                    output.status,
                    bounded_diagnostic("stderr", &output.stderr)
                ),
                Err(message) => eprintln!(
                    "Workspace Python fixture {program}: {}",
                    bounded_diagnostic("probe failed", message)
                ),
                _ => {}
            }
            result
        })
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

fn retained_fixed_interpreter(owner: &Owner, root: &Path) -> Arc<FixedInterpreter> {
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
        std::fs::write(root.path().join(".venv/keep"), "installed dependency").expect("sentinel");
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
    assert_ne!(selection.path_prefix, [] as [std::path::PathBuf; 0]);
    let script = selection.path_prefix[0].join("python");
    let body = std::fs::read_to_string(&script).expect("guard");
    assert!(body.contains("exit 127"));
    assert!(!body.contains("UV_") && !body.contains("/users/"));
    let _ = ensure_guard("Workspace Python is not ready");
}

#[tokio::test]
async fn u16_second_process_holds_the_workspace_lock() {
    const NAME: &str = "workspace_python::tests::u16_second_process_holds_the_workspace_lock";
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
    let pin = read_pin(root.path()).unwrap();
    assert_eq!(pin.as_deref(), Some("3.12"));
    assert_eq!(
        creation_request(root.path(), pin.as_deref(), None).unwrap(),
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
        let source_stamp = stamp(&source);
        let base_stamp = stamp(&base);
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
            std::os::unix::fs::symlink(if change == "redirect" { &other } else { &base }, &entry)
                .unwrap();
        }
        assert_eq!(generation_of(root.path()), original);
        assert_eq!(
            stamp(&source),
            source_stamp,
            "root directory stamp is unchanged"
        );
        assert_eq!(stamp(&base), base_stamp, "old base executable is unchanged");
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
        gate: Mutex<Option<TestGate>>,
    }
    impl ProgramProbe for SecondProbeGate {
        fn run(&self, request: ProgramRequest) -> BoxFuture<'_, Result<ProgramOutput, String>> {
            Box::pin(async move {
                if Path::new(&request.argv[0]) == self.second {
                    pass_gate(&self.gate).await;
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
    assert_eq!(requested, &absolute_pin(&root_path, "./pinned-python"));
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
    assert_ne!(selection.path_prefix, [] as [std::path::PathBuf; 0]);
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
    assert!(install_guard(&not_directory, "disk full").is_none());
    let selection = selection_from_guard(None, "disk full");
    assert!(!selection.guard_unavailable);
    assert_ne!(selection.path_prefix, [] as [std::path::PathBuf; 0]);
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
    let entries = execution_environment::compose(&host, &[], &selection).expect("composed");
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
        install_guard(directory.path(), diagnostic).expect("initial guard");
        let python = directory.path().join("python");
        if altered {
            std::fs::write(&python, "#!/bin/sh\nexit 0\n").expect("alter guard");
        } else {
            std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o644))
                .expect("guard permissions");
        }
        let specific = install_guard(directory.path(), diagnostic);
        assert!(specific.is_none());
        let selection = selection_from_guard(specific, diagnostic);
        let entries = execution_environment::compose(
            &host_from_entries(vec![("PATH", "/usr/bin:/bin")]),
            &[],
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
    const NAME: &str =
        "workspace_python::tests::shutdown_stops_waiting_for_a_lock_another_process_still_holds";
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

#[tokio::test]
async fn ancestor_python_version_does_not_bind_workspace_discovery() {
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
                Arc::new(SupervisedProbe),
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
        assert!(super::interpreter::discovery_directory().starts_with(base.join("tmp")));
        return;
    }
    let base = workspace();
    std::fs::write(base.path().join(".python-version"), "9.99").unwrap();
    std::fs::create_dir(base.path().join("tmp")).unwrap();
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "workspace_python::tests::ancestor_python_version_does_not_bind_workspace_discovery",
            "--nocapture",
        ])
        .env(CASE, base.path())
        .env("TMPDIR", base.path().join("tmp"))
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
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
    assert!(
        super::interpreter::fresh_discovery(&mut cached, std::slice::from_ref(&file)).is_none()
    );
    assert!(cached.is_none());
    let good = tempfile::tempdir().expect("good");
    let first = super::interpreter::fresh_discovery(
        &mut cached,
        &[file.clone(), good.path().to_path_buf()],
    )
    .expect("later parent");
    assert!(first.is_dir());
    assert_eq!(
        super::interpreter::fresh_discovery(&mut cached, std::slice::from_ref(&file)).as_ref(),
        Some(&first)
    );
    std::fs::remove_dir(&first).expect("remove");
    let good_path = good.path().to_path_buf();
    let second = super::interpreter::fresh_discovery(&mut cached, std::slice::from_ref(&good_path))
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
    assert!(super::guard::claim_fresh_directory(&planted).is_none());
}

fn run_bash(entries: &[(String, String)], program: &str) -> std::process::Output {
    run_program(entries, "/bin/bash", &["-c", program])
}

fn run_program(entries: &[(String, String)], program: &str, args: &[&str]) -> std::process::Output {
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
    assert!(
        super::guard::install_process_guard(std::slice::from_ref(&file), "disk full").is_none()
    );
    let guard =
        super::guard::install_process_guard(&[file, good.path().to_path_buf()], "disk full")
            .expect("a later private directory holds the guard");
    let selection = PythonSelection {
        path_prefix: vec![guard.path().to_owned()],
        virtual_env: None,
        diagnostic: "disk full".to_owned(),
        guard_unavailable: false,
    };
    let host = host_from_entries(vec![("PATH", "/usr/bin:/bin"), ("HOME", "/home/ada")]);
    let entries = execution_environment::compose(&host, &[], &selection)
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

fn stage_relocated_executable(directory: &Path, name: &str) -> PathBuf {
    let source = std::env::current_exe().expect("test executable");
    let target = directory.join(name);
    if std::fs::hard_link(&source, &target).is_err() {
        std::fs::copy(&source, &target).expect("copy relocated executable");
    }
    target
}

fn record_guard_witness(expected_token: &str) {
    let witness_path = std::env::var_os("RUSTX_TEST_GUARD_WITNESS")
        .expect("RUSTX_TEST_GUARD_WITNESS must be set in relocated guard subprocess");
    std::fs::write(&witness_path, expected_token).expect("write guard witness");
}

fn verify_unprovisioned_guard_subprocess(host: &execution_environment::ExecutionHost) {
    assert!(super::guard::provisioned_guard().is_none());
    let selection = super::guard::finish_guard(None, "disk full");
    assert!(selection.guard_unavailable);
    assert!(
        execution_environment::compose(host, &[], &selection,).is_err(),
        "unprovisioned binary without writable guard storage must fail closed"
    );
    record_guard_witness("unprovisioned-ok");
}

fn verify_provisioned_guard_subprocess(host: &execution_environment::ExecutionHost) {
    let selection = super::guard::finish_guard(None, "disk full");
    assert!(!selection.guard_unavailable);
    let entries = execution_environment::compose(host, &[], &selection)
        .expect("composition stays available with provisioned sidecar");
    let deployed =
        super::guard::provisioned_guard().expect("guard beside the relocated test executable");
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
            .any(|(key, value)| key == super::GUARD_MESSAGE_KEY && value == "disk full")
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
    record_guard_witness("provisioned-ok");
}

fn set_guard_permissions(
    directory: &Path,
    dir_mode: u32,
    script_mode: u32,
    restore_dir_first: bool,
) {
    let set_dir = || {
        if let Ok(meta) = std::fs::metadata(directory) {
            let mut perms = meta.permissions();
            perms.set_mode(dir_mode);
            let _ = std::fs::set_permissions(directory, perms);
        }
    };
    let set_scripts = || {
        for name in ["python", "python3"] {
            let path = directory.join(name);
            if let Ok(meta) = std::fs::metadata(&path) {
                let mut perms = meta.permissions();
                perms.set_mode(script_mode);
                let _ = std::fs::set_permissions(&path, perms);
            }
        }
    };
    if restore_dir_first {
        set_dir();
        set_scripts();
    } else {
        set_scripts();
        set_dir();
    }
}

struct RestoreGuardPermissions(PathBuf);

impl Drop for RestoreGuardPermissions {
    fn drop(&mut self) {
        set_guard_permissions(&self.0, 0o755, 0o755, true);
    }
}

#[test]
fn no_guard_directory_rejects_path_lookup_and_keeps_other_commands() {
    let host = host_from_entries(vec![("PATH", "/usr/bin:/bin"), ("HOME", "/home/ada")]);
    match std::env::var("RUSTX_TEST_GUARD_MODE").ok().as_deref() {
        Some("unprovisioned") => {
            verify_unprovisioned_guard_subprocess(&host);
            return;
        }
        Some("provisioned") => {
            verify_provisioned_guard_subprocess(&host);
            return;
        }
        Some(other) => panic!("unexpected RUSTX_TEST_GUARD_MODE: {other}"),
        None => {}
    }

    // Compute the test filter dynamically so later module moves keep the exact selector valid.
    let module_suffix = module_path!()
        .split_once("::")
        .map_or(module_path!(), |(_, rest)| rest);
    let test_filter =
        format!("{module_suffix}::no_guard_directory_rejects_path_lookup_and_keeps_other_commands");
    let witness_dir = tempfile::tempdir().expect("witness");

    // First verify fail-closed behavior in an isolated relocated executable without sidecar.
    let unprovisioned_dir = tempfile::tempdir().expect("unprovisioned");
    let unprovisioned_exe =
        stage_relocated_executable(unprovisioned_dir.path(), "rustx-unprovisioned");
    assert!(provisioned_guard_for(&unprovisioned_exe).is_none());
    let unprovisioned_witness = witness_dir.path().join("unprovisioned.witness");
    let unprovisioned_out = std::process::Command::new(&unprovisioned_exe)
        .env("RUSTX_TEST_GUARD_MODE", "unprovisioned")
        .env("RUSTX_TEST_GUARD_WITNESS", &unprovisioned_witness)
        .args(["--exact", &test_filter, "--nocapture"])
        .output()
        .expect("spawn unprovisioned relocated runner");
    assert!(unprovisioned_out.status.success(), "{unprovisioned_out:?}");
    let unprovisioned_stdout = String::from_utf8_lossy(&unprovisioned_out.stdout);
    assert!(
        unprovisioned_stdout.contains("1 passed; 0 failed"),
        "expected unprovisioned child to run 1 test: {unprovisioned_stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(&unprovisioned_witness).expect("unprovisioned witness must exist"),
        "unprovisioned-ok"
    );

    // Next verify an explicitly pre-provisioned read-only executable-adjacent sidecar works.
    let provisioned_dir = tempfile::tempdir().expect("provisioned");
    let provisioned_exe = stage_relocated_executable(provisioned_dir.path(), "rustx-provisioned");
    let guard = provisioned_dir.path().join("python-guard");
    install_embedded_guard(&guard);
    let _restore = RestoreGuardPermissions(guard.clone());
    // Both the guard scripts and their containing directory must be read-only;
    // never chmod the relocated executable inode shared with the build artifact.
    set_guard_permissions(&guard, 0o555, 0o555, false);
    let resolved_guard = provisioned_guard_for(&provisioned_exe);
    let provisioned_witness = witness_dir.path().join("provisioned.witness");
    let provisioned_res = std::process::Command::new(&provisioned_exe)
        .env("RUSTX_TEST_GUARD_MODE", "provisioned")
        .env("RUSTX_TEST_GUARD_WITNESS", &provisioned_witness)
        .args(["--exact", &test_filter, "--nocapture"])
        .output();
    set_guard_permissions(&guard, 0o755, 0o755, true);
    assert_eq!(resolved_guard, Some(guard));
    let provisioned_out = provisioned_res.expect("spawn provisioned relocated runner");
    assert!(provisioned_out.status.success(), "{provisioned_out:?}");
    let provisioned_stdout = String::from_utf8_lossy(&provisioned_out.stdout);
    assert!(
        provisioned_stdout.contains("1 passed; 0 failed"),
        "expected provisioned child to run 1 test: {provisioned_stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(&provisioned_witness).expect("provisioned witness must exist"),
        "provisioned-ok"
    );
}

fn install_embedded_guard(directory: &Path) {
    std::fs::create_dir_all(directory).expect("guard directory");
    for (name, bytes) in [("python", GUARD_PYTHON), ("python3", GUARD_PYTHON3)] {
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
    assert!(guard_beside_executable(&executable).is_none());
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
    assert!(guard_beside_executable(&executable).is_none());
}

#[test]
fn relocated_install_rejects_python_without_the_build_tree() {
    let install = tempfile::tempdir().expect("install");
    let executable = install.path().join("rustx");
    std::fs::write(&executable, b"bin").expect("executable");
    let guard = install.path().join("python-guard");
    install_embedded_guard(&guard);
    let resolved = guard_beside_executable(&executable).expect("deployed guard");
    assert_eq!(resolved, guard);
    assert!(!resolved.starts_with(env!("CARGO_MANIFEST_DIR")));

    let blocked = tempfile::tempdir().expect("blocked");
    let file = blocked.path().join("not-a-directory");
    std::fs::write(&file, "x").expect("file");
    assert!(
        super::guard::install_process_guard(std::slice::from_ref(&file), "disk full").is_none()
    );
    let selection = super::guard::directory_guard(Some(resolved), "disk full");
    assert!(!selection.guard_unavailable);
    let host = host_from_entries(vec![("PATH", "/usr/bin:/bin"), ("HOME", "/home/ada")]);
    let entries = execution_environment::compose(&host, &[], &selection)
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

#[tokio::test]
async fn relative_interpreter_pin_is_bound_to_the_workspace() {
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
            Arc::new(RecordingSupervisedProbe::default()),
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

#[tokio::test]
async fn directory_pins_create_and_reuse_without_uv() {
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
        let probe = Arc::new(RecordingSupervisedProbe::default());
        let owner = Owner::for_test(probe.clone(), Some(Arc::new(host)), None, None);
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
        let selected = source.join(if spelling == "relative" {
            "bin/python3"
        } else {
            "bin/python"
        });
        probe.assert_invoked(&selected);
        owner.stop().await;

        let no_uv = workspace();
        let host = host_for(no_uv.path());
        assert!(super::interpreter::find_uv(&host, root.path()).is_none());
        let original = generation_of(root.path());
        let owner = Owner::for_test(Arc::new(SupervisedProbe), Some(host), None, None);
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

#[tokio::test]
async fn invalid_directory_pins_fail_without_changing_the_target() {
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
                std::os::unix::fs::symlink("/usr/bin/python3", source.join("bin/python3")).unwrap();
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
                super::owner::PROBE_TIMEOUT * 4,
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
    slot: &Mutex<Option<TestGate>>,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (entered, arrival) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    *slot.lock().unwrap() = Some(TestGate {
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
    // Replace the inode so this controlled generation change does not depend
    // on the filesystem assigning a new mtime to two same-length writes.
    let replacement = root.path().join("next-python-version");
    std::fs::write(&replacement, "3.11").unwrap();
    std::fs::rename(replacement, root.path().join(".python-version")).unwrap();
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
    std::os::unix::fs::symlink(external.path().join("venv"), root.path().join(".venv")).unwrap();
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
    let first = super::guard::cached_guard(&mut cached, &parents).unwrap();
    std::fs::remove_dir_all(&first).unwrap();
    let second = super::guard::cached_guard(&mut cached, &parents).unwrap();
    assert_ne!(first, second);
    std::fs::set_permissions(
        second.join("python3"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let third = super::guard::cached_guard(&mut cached, &parents).unwrap();
    assert_ne!(second, third);
    let host = host_from_entries(vec![("PATH", "/usr/bin:/bin")]);
    let selection = super::guard::directory_guard(Some(third), "disk full");
    let entries = execution_environment::compose(&host, &[], &selection).unwrap();
    assert_guard_rejected(&run_bash(&entries, "python3"));
    assert_guard_rejected(&run_bash(&entries, "/usr/bin/env python3"));
    assert!(run_bash(&entries, "ls /bin/sh >/dev/null").status.success());
}

#[tokio::test]
async fn binding_refuses_a_workspace_replaced_before_preparation() {
    let parent = workspace();
    let root = parent.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let original = std::fs::metadata(&root).unwrap();
    let bin = workspace();
    write_executable(&bin.path().join("uv"), "#!/bin/sh\nexit 0\n");
    let probe = Arc::new(Scripted::new("0.10.2"));
    let mut owner = Owner::for_test(probe.clone(), Some(host_for(bin.path())), None, None);
    owner.workspace_identity = Some((original.dev(), original.ino()));

    std::fs::rename(&root, parent.path().join("previous-workspace")).unwrap();
    std::fs::create_dir(&root).unwrap();
    assert_eq!(
        owner.observe(&root).diagnostic,
        "the bound workspace directory changed"
    );
    owner.prepare(root.clone()).await;
    assert!(probe.calls.lock().unwrap().is_empty());
    assert!(owner.records.lock().unwrap().is_empty());
    assert!(!root.join(".venv").exists());
}

#[tokio::test]
async fn replaced_root_while_create_waits_for_permit_is_not_written() {
    let parent = workspace();
    let root = parent.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let bin = workspace();
    write_executable(
        &bin.path().join("uv"),
        "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'uv 0.10.2\\n'; else /usr/bin/touch physical-start; fi\n",
    );
    let owner = Arc::new(Owner::for_test(
        Arc::new(SupervisedProbe),
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

#[tokio::test]
async fn shutdown_between_check_and_signal_cannot_start_a_program() {
    let root = workspace();
    write_venv(&root.path().join(".venv"));
    for name in ["python", "python3"] {
        write_executable(
            &root.path().join(".venv/bin").join(name),
            "#!/bin/sh\n/usr/bin/touch physical-start\n",
        );
    }
    let mut owner = Owner::for_test(Arc::new(SupervisedProbe), None, None, None);
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
    assert!(super::guard::cached_guard(&mut cached, &[file]).is_none());
    let selection = super::guard::directory_guard(None, "no guard resource");
    let host = host_from_entries(vec![("PATH", "/usr/bin:/bin")]);
    let error = execution_environment::compose(&host, &[], &selection).unwrap_err();
    assert!(error.contains("guard"));
}

fn installed_uv_path() -> String {
    let uv = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("uv"))
        .find(|path| path.is_file())
        .expect("uv required by boundary suite");
    format!("{}:/usr/bin:/bin", uv.parent().unwrap().display())
}

fn argv0_sensitive_python(root: &Path) -> (PathBuf, PathBuf) {
    let launcher = root.join("python-entry");
    let alias = root.join("pinned-python");
    write_executable(
        &launcher,
        "#!/bin/sh\nif [ \"$0\" != \"$PIN_EXPECTED_PROGRAM\" ]; then echo 'unexpected interpreter invocation path' >&2; exit 73; fi\nexec /usr/bin/python3 \"$@\"\n",
    );
    std::os::unix::fs::symlink(&launcher, &alias).unwrap();
    for (program, status) in [(&alias, 73), (&launcher, 0)] {
        let output = std::process::Command::new(program)
            .args(["-I", "-c", "import sys; print(sys.prefix)"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root)
            .env("PIN_EXPECTED_PROGRAM", &launcher)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(status),
            "{program:?}: {output:?}"
        );
    }
    (launcher, alias)
}

#[tokio::test]
async fn file_pin_invocation_resolves_alias_without_losing_its_witness() {
    let root = workspace();
    let (launcher, alias) = argv0_sensitive_python(root.path());
    std::fs::write(root.path().join(".python-version"), "./pinned-python").unwrap();
    let no_uv = workspace();
    for (reuse, path) in [
        (false, installed_uv_path()),
        (true, no_uv.path().display().to_string()),
    ] {
        assert_eq!(root.path().join(".venv").exists(), reuse);
        let original = generation_of(root.path());
        let host = host_from_entries(vec![
            ("PATH", &path),
            ("PIN_EXPECTED_PROGRAM", launcher.to_str().unwrap()),
            ("UV_OFFLINE", "1"),
            ("UV_PYTHON_DOWNLOADS", "never"),
        ]);
        if reuse {
            assert!(super::interpreter::find_uv(&host, root.path()).is_none());
        }
        let probe = Arc::new(RecordingSupervisedProbe::default());
        let owner = Owner::for_test(probe.clone(), Some(Arc::new(host)), None, None);
        owner.prepare(root.path().to_path_buf()).await;
        assert_eq!(
            owner.observe(root.path()).virtual_env,
            Some(root.path().join(".venv")),
            "reuse={reuse}: {:?}",
            owner.records.lock().unwrap()
        );
        assert_eq!(
            python_base_executable(&root.path().join(".venv/bin/python")),
            python_base_executable(Path::new("/usr/bin/python3"))
        );
        probe.assert_invoked(&launcher);
        {
            let calls = probe.calls.lock().unwrap();
            assert!(calls.iter().all(|argv| Path::new(&argv[0]) != alias));
        }
        if reuse {
            assert_eq!(generation_of(root.path()), original);
            let count = probe.calls.lock().unwrap().len();
            std::fs::rename(&alias, root.path().join("previous-alias")).unwrap();
            std::os::unix::fs::symlink("/usr/bin/python3", &alias).unwrap();
            assert!(owner.observe(root.path()).virtual_env.is_none());
            assert_eq!(probe.calls.lock().unwrap().len(), count);
            assert_eq!(generation_of(root.path()), original);
        }
        owner.stop().await;
    }
}

#[tokio::test]
async fn repaired_path_pin_recovers_without_rewriting_root_inputs() {
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
                std::fs::set_permissions(&pinned, std::fs::Permissions::from_mode(0o600)).unwrap();
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
                Arc::new(RecordingSupervisedProbe::default()),
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
                let Some(Record::Unavailable { retry_after, .. }) = records.get_mut(root.path())
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

#[tokio::test]
async fn real_uv_missing_interpreter_obeys_download_policy() {
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
        let owner = Owner::for_test(Arc::new(SupervisedProbe), Some(Arc::new(host)), None, None);
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

#[tokio::test]
async fn real_uv_does_not_seed_or_select_an_external_project() {
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
    let owner = Owner::for_test(Arc::new(SupervisedProbe), Some(Arc::new(host)), None, None);
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

#[tokio::test]
async fn real_dual_venv_launcher_variables_cannot_change_prefix() {
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
            let owner = Owner::for_test(Arc::new(SupervisedProbe), Some(host.clone()), None, None);
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
            let base = if configured {
                vec![(key.to_owned(), foreign_python.display().to_string())]
            } else {
                vec![]
            };
            let entries =
                execution_environment::compose(&host, &base, &owner.observe(root.path())).unwrap();
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
        let actual = creation_request(Path::new("/workspace"), pin, requires).unwrap();
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

#[tokio::test]
async fn targets_added_while_creation_waits_for_capacity_are_never_initialized() {
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
            Arc::new(SupervisedProbe),
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

#[tokio::test]
async fn launcher_selection_change_between_probes_cannot_publish_ready() {
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
    let stamps = paths.map(|path| stamp(path));
    let host = host_from_entries(vec![
        ("PATH", "/usr/bin:/bin"),
        ("PATH_PIN_SELECTOR", selector.to_str().unwrap()),
    ]);
    let owner = Arc::new(Owner::for_test(
        Arc::new(SupervisedProbe),
        Some(Arc::new(host)),
        None,
        None,
    ));
    let (entered, release) = arm_gate(&owner.after_interpreter);
    let preparing = owner.clone();
    let path = root.path().to_path_buf();
    let task = tokio::spawn(async move { preparing.prepare(path).await });
    tokio::time::timeout(super::owner::PROBE_TIMEOUT * 4, entered)
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
        paths.map(|path| stamp(path)),
        stamps,
        "all recorded metadata stayed unchanged"
    );
    owner.stop().await;
}

#[tokio::test]
async fn real_multi_interpreter_selection_satisfies_both_root_inputs() {
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
        .envs(execution_environment::bootstrap_environment(&host))
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
        let owner = Owner::for_test(Arc::new(SupervisedProbe), Some(host.clone()), None, None);
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
        let owner = Owner::for_test(Arc::new(SupervisedProbe), Some(host.clone()), None, None);
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
        gate: Mutex<Option<TestGate>>,
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
                        pass_gate(&self.gate).await;
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
