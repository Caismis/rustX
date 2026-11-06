//! Real Git read-only enumeration proofs for the runtime-owned optional observation.
use crate::runtime::conversation_runtime::test_support::*;
use std::{
    fs,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct WriteTracked {
    configuration: HeldConfiguration,
}
impl ToolExecutor for WriteTracked {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        let configuration = self.configuration.clone();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                fs::write(context.workspace.root().join("tracked.txt"), "after\n").unwrap();
                configuration.arm();
                success_result("changed")
            }),
            cancellation,
        )
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

#[derive(Clone)]
struct HeldConfiguration {
    root: PathBuf,
    fifo: PathBuf,
    original: Vec<u8>,
}

impl HeldConfiguration {
    fn arm(&self) {
        let mut configuration = self.original.clone();
        let path = self
            .fifo
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        configuration.extend_from_slice(format!("\n[include]\n\tpath = \"{path}\"\n").as_bytes());
        fs::write(self.root.join(".git/config"), configuration).unwrap();
    }

    fn restore(&self) {
        fs::write(self.root.join(".git/config"), &self.original).unwrap();
    }

    async fn reader(&self) -> (fs::File, i32) {
        // Opening this writer proves that a Git reader has reached the FIFO.
        // Keeping it open without data prevents EOF and holds that exact read.
        let writer = bounded(async {
            loop {
                match fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&self.fifo)
                {
                    Ok(writer) => break writer,
                    Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {}
                    Err(error) => panic!("open held configuration: {error}"),
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        let pid = bounded(async {
            loop {
                for entry in fs::read_dir("/proc").unwrap().flatten() {
                    let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
                        continue;
                    };
                    if fs::read_link(entry.path().join("exe"))
                        .ok()
                        .and_then(|path| path.file_name().map(|name| name == "git"))
                        != Some(true)
                    {
                        continue;
                    }
                    if fs::read_dir(entry.path().join("fd")).is_ok_and(|fds| {
                        fds.flatten()
                            .any(|fd| fs::read_link(fd.path()).ok().as_ref() == Some(&self.fifo))
                    }) {
                        return pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        (writer, pid)
    }
}

fn configure_repository(dir: &tempfile::TempDir) -> HeldConfiguration {
    let root = dir.path().join("workspace");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    fs::write(root.join("tracked.txt"), "before\n").unwrap();
    git(&root, &["add", "tracked.txt"]);
    git(
        &root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "initial",
        ],
    );
    let fifo = dir.path().join("held-config");
    nix::unistd::mkfifo(
        &fifo,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    let original = fs::read(root.join(".git/config")).unwrap();
    HeldConfiguration {
        root,
        fifo,
        original,
    }
}

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    // A guard below the independent Git timeout, not a performance assertion:
    // the held read has no data or EOF and cannot complete by itself.
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("cancelled observation must not await the held read or its 30s timeout")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_attempt_does_not_start_a_final_git_read() {
    let dir = tempfile::tempdir().unwrap();
    let configuration = configure_repository(&dir);
    let (registry, tool_script) = cause_probe_registry_and_script(Arc::new(WriteTracked {
        configuration: configuration.clone(),
    }));
    let (runtime, model) = headless_runtime(
        &dir,
        vec![
            tool_script,
            vec![
                FakeStep::Emit(crate::model::ModelEvent::Started),
                FakeStep::ParkUntilCancelled,
            ],
            one_turn_script(),
        ],
        Some(registry),
        None,
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .unwrap();
    runtime.activate();
    runtime
        .submit_inbound(text_content("change the file"))
        .unwrap();
    let mut parked = model.parked();
    bounded(parked.wait_for(|parked| *parked)).await.unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("workspace/tracked.txt")).unwrap(),
        "after\n"
    );
    let attempt = runtime.cancel_active_attempt().unwrap();
    let settlement = bounded(runtime.child_attempt_settlement(0)).await;
    assert_eq!(settlement.attempt_id, attempt);
    assert!(matches!(
        settlement.outcome,
        crate::runtime::conversation_runtime::NativeAttemptOutcome::Cancelled
    ));
    let observations = bounded(await_observation(&pending, |observation| {
        matches!(
            observation,
            ConversationObservation::Event {
                event: RuntimeEvent::AttemptCancelled { .. },
                ..
            }
        )
    }))
    .await;
    assert_eq!(count_terminal(&observations), 1);
    let reader = fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&configuration.fifo);
    assert_eq!(
        reader.unwrap_err().raw_os_error(),
        Some(libc::ENXIO),
        "a cancelled Attempt must not start its final Git read"
    );
    assert!(
        runtime
            .tool_runtime()
            .workspace_changes
            .summary(&attempt)
            .is_none()
    );
    configuration.restore();
    runtime.submit_inbound(text_content("next task")).unwrap();
    assert_eq!(
        bounded(runtime.child_attempt_settlement(1)).await.ordinal,
        2
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_final_git_read_releases_terminal_and_handoff_before_read_release() {
    let dir = tempfile::tempdir().unwrap();
    let configuration = configure_repository(&dir);
    let (registry, tool_script) = cause_probe_registry_and_script(Arc::new(WriteTracked {
        configuration: configuration.clone(),
    }));
    let (runtime, _) = headless_runtime(
        &dir,
        vec![tool_script, one_turn_script(), one_turn_script()],
        Some(registry),
        None,
    )
    .await;
    let pending = Arc::new(PendingObservations::new());
    runtime
        .install_fixture_observation_bridge(&pending)
        .unwrap();
    runtime.activate();
    runtime
        .submit_inbound(text_content("change the file"))
        .unwrap();
    let (_held_writer, pid) = configuration.reader().await;
    let attempt = runtime.cancel_active_attempt().unwrap();
    let settlement = bounded(runtime.child_attempt_settlement(0)).await;
    assert_eq!(settlement.attempt_id, attempt);
    // The successful model selected completion before observation began.
    // Cancellation releases the optional observation, never relabels it.
    assert!(matches!(
        settlement.outcome,
        crate::runtime::conversation_runtime::NativeAttemptOutcome::Completed
    ));
    let observations = bounded(await_observation(&pending, |observation| matches!(observation, ConversationObservation::Event { event, .. } if is_terminal_event(event)))).await;
    assert_eq!(count_terminal(&observations), 1);
    assert!(
        runtime
            .tool_runtime()
            .workspace_changes
            .summary(&attempt)
            .is_none()
    );
    // A new task completes while the old FIFO writer still supplies neither
    // data nor EOF. Restore configuration only for subsequent Git commands.
    configuration.restore();
    runtime.submit_inbound(text_content("next task")).unwrap();
    assert_eq!(
        bounded(runtime.child_attempt_settlement(1)).await.ordinal,
        2
    );
    bounded(async {
        while Path::new(&format!("/proc/{pid}")).exists() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    runtime.shutdown().await.unwrap();
    assert!(
        runtime
            .tool_runtime()
            .workspace_changes
            .summary(&attempt)
            .is_none(),
        "a cancelled recorder cannot publish late"
    );
}
