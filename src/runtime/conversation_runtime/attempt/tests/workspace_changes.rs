//! Real Git/filter boundary proofs for the runtime-owned optional observation.
use crate::runtime::conversation_runtime::test_support::*;
use std::{fs, path::Path, time::Duration};
use tokio::io::{AsyncBufReadExt, BufReader};

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

struct WriteTracked;
impl ToolExecutor for WriteTracked {
    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                fs::write(context.workspace.root().join("tracked.txt"), "after\n").unwrap();
                success_result("changed")
            }),
            cancellation,
        )
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

fn configure_filter(dir: &tempfile::TempDir) -> tokio::net::unix::pipe::Receiver {
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
    for name in ["ready", "hold"] {
        nix::unistd::mkfifo(
            &dir.path().join(name),
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
    }
    // Only changed content blocks. The baseline finishes and tools run before
    // the final filter writes its readiness receipt and parks on the hold FIFO.
    let script = dir.path().join("filter.sh");
    fs::write(&script, format!(
        "IFS= read -r first\nif [ \"$first\" = after ]; then\nprintf entered > '{}/entered'\nprintf '%s\\n%s\\n' \"$$\" \"$GIT_INDEX_FILE\" > '{}/ready'\nread -r release < '{}/hold'\nfi\nprintf '%s\\n' \"$first\"\ncat\n",
        dir.path().display(), dir.path().display(), dir.path().display(),
    )).unwrap();
    git(
        &root,
        &[
            "config",
            "filter.held.clean",
            &format!("sh '{}'", script.display()),
        ],
    );
    git(&root, &["config", "filter.held.required", "true"]);
    fs::write(root.join(".gitattributes"), "tracked.txt filter=held\n").unwrap();
    tokio::net::unix::pipe::OpenOptions::new()
        .read_write(true)
        .open_receiver(dir.path().join("ready"))
        .unwrap()
}

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    // A guard below the independent Git timeout, not a performance assertion:
    // the held filter has no release and cannot complete by itself.
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("cancelled observation must not await the held filter or its 30s timeout")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_attempt_does_not_start_a_final_git_filter() {
    let dir = tempfile::tempdir().unwrap();
    let _ready = configure_filter(&dir);
    let (registry, tool_script) = cause_probe_registry_and_script(Arc::new(WriteTracked));
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
    assert!(
        !dir.path().join("entered").exists(),
        "a cancelled Attempt must not launch its final filter"
    );
    assert!(
        runtime
            .tool_runtime()
            .workspace_changes
            .summary(&attempt)
            .is_none()
    );
    fs::remove_file(dir.path().join("workspace/.gitattributes")).unwrap();
    runtime.submit_inbound(text_content("next task")).unwrap();
    assert_eq!(
        bounded(runtime.child_attempt_settlement(1)).await.ordinal,
        2
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_final_git_filter_releases_terminal_and_handoff_before_filter_release() {
    let dir = tempfile::tempdir().unwrap();
    let ready = configure_filter(&dir);
    let (registry, tool_script) = cause_probe_registry_and_script(Arc::new(WriteTracked));
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
    let mut lines = BufReader::new(ready).lines();
    let pid: i32 = bounded(lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .parse()
        .unwrap();
    let index = bounded(lines.next_line()).await.unwrap().unwrap();
    assert!(Path::new(&index).exists());
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
    // A new task is admitted and completes while the old hold FIFO was never
    // opened/released. Remove the attribute only to avoid blocking its baseline.
    fs::remove_file(dir.path().join("workspace/.gitattributes")).unwrap();
    runtime.submit_inbound(text_content("next task")).unwrap();
    assert_eq!(
        bounded(runtime.child_attempt_settlement(1)).await.ordinal,
        2
    );
    bounded(async {
        while Path::new(&format!("/proc/{pid}")).exists()
            || Path::new(&index).parent().unwrap().exists()
        {
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
