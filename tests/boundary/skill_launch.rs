#![cfg(target_os = "linux")]

//! Linux launch accounting for Skill discovery.
//!
//! The invariant is a real process boundary: selecting and reloading Skills
//! that carry Python and Node metadata performs no `posix_spawn` or `exec`.
//! The recorder is loaded with `LD_PRELOAD` into one fresh copy of this test
//! binary, so the record is written on the launching thread before libc
//! returns. The observer reads the log only after an awaited probe has exited.

use std::path::PathBuf;

const OBSERVATION: &str = "boundary_suites::skill_launch::skill_discovery_launches_nothing_and_the_recorder_sees_an_exited_probe";

/// Compiles the libc launch recorder and runs this test again with that
/// library preloaded. A harness that runs zero tests is a failure.
#[cfg(target_os = "linux")]
fn run_isolated_observation() {
    let library = compile_launch_recorder();
    let log = tempfile::NamedTempFile::new().expect("launch log");
    let output =
        std::process::Command::new(std::env::current_exe().expect("current test executable"))
            .args(["--exact", OBSERVATION, "--test-threads=1"])
            .env("RUSTX_SKILL_PROBE_CHILD", "1")
            .env("RUSTX_LAUNCH_LOG", log.path())
            .env("LD_PRELOAD", &library)
            .output()
            .expect("spawn the isolated launch observation");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed"),
        "isolated observation failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[cfg(target_os = "linux")]
fn compile_launch_recorder() -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("rustx-launch-recorder-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("launch recorder directory");
    let library = directory.join("librustx_launch_recorder.so");
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/boundary/launch_recorder_preload.rs");
    let output = std::process::Command::new("rustc")
        .args([
            "--crate-type",
            "cdylib",
            "--edition",
            "2021",
            "-C",
            "panic=abort",
            "-C",
            "opt-level=3",
            "-o",
        ])
        .arg(&library)
        .arg(&source)
        .output()
        .expect("run rustc for the launch recorder");
    assert!(
        output.status.success(),
        "launch recorder failed to compile\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    library
}

/// The preloaded recorder must be mapped. An absent recorder is a failure,
/// never an empty launch log.
#[cfg(target_os = "linux")]
fn launch_log_path() -> PathBuf {
    let path = std::env::var("RUSTX_LAUNCH_LOG").expect("RUSTX_LAUNCH_LOG");
    let maps = std::fs::read_to_string("/proc/self/maps").expect("process maps");
    assert!(
        maps.contains("librustx_launch_recorder"),
        "launch recorder is not loaded; refusing to report zero starts"
    );
    PathBuf::from(path)
}

/// Reads the launch log after every launch the caller awaited has returned.
#[cfg(target_os = "linux")]
fn drain_launch_log(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).expect("launch log")
}

/// Spawns `python3 --version`, waits until it has exited, and returns its pid.
#[cfg(target_os = "linux")]
fn await_exited_python_probe(fork_exec: bool) -> u32 {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new("python3");
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if fork_exec {
        // An empty `pre_exec` selects std's fork/`execvp` path. The closure
        // runs between fork and exec and performs no work.
        #[allow(unsafe_code)]
        unsafe {
            command.pre_exec(|| Ok(()));
        }
    }
    let mut child = command.spawn().expect("spawn python3 --version");
    let pid = child.id();
    let status = child.wait().expect("wait for python3 --version");
    assert!(status.success(), "{status}");
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "the probe must already have exited"
    );
    pid
}

/// Proves the gated reader still sees both libc paths after the probes exit.
#[cfg(target_os = "linux")]
fn prove_exited_probes_stay_in_the_log(path: &std::path::Path) {
    std::fs::write(path, "").expect("truncate launch log");
    let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let reader_path = path.to_path_buf();
    let reader_released = std::sync::Arc::clone(&released);
    let reader = std::thread::spawn(move || {
        gate.recv().expect("observation gate");
        reader_released.store(true, std::sync::atomic::Ordering::SeqCst);
        drain_launch_log(&reader_path)
    });
    let _spawn_pid = await_exited_python_probe(false);
    let _exec_pid = await_exited_python_probe(true);
    assert!(
        !released.load(std::sync::atomic::Ordering::SeqCst),
        "the observer read the launch log before both probes had exited"
    );
    release.send(()).expect("release the observer");
    let launches = reader.join().expect("observer joins after the probes exit");
    assert!(
        launches.contains("posix_spawnp\tpython3\n"),
        "missing posix_spawnp attempt after the probe exited:\n{launches}"
    );
    assert!(
        launches.contains("execvp\tpython3\n"),
        "missing execvp attempt after the probe exited:\n{launches}"
    );
}

/// S01/S02: the recorder sees an exited interpreter, and Skill discovery,
/// preparation, and reload record no later launch.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn skill_discovery_launches_nothing_and_the_recorder_sees_an_exited_probe() {
    if std::env::var_os("RUSTX_SKILL_PROBE_CHILD").is_none() {
        run_isolated_observation();
        return;
    }
    let path = launch_log_path();
    prove_exited_probes_stay_in_the_log(&path);
    let before = drain_launch_log(&path);
    crate::scripted_suites::exercise_skill_catalog_without_environments().await;
    let after = drain_launch_log(&path);
    assert!(
        after.starts_with(&before),
        "the launch log was replaced during skill discovery"
    );
    let started = &after[before.len()..];
    assert!(
        started.is_empty(),
        "skill discovery launched processes:\n{started}"
    );
}
