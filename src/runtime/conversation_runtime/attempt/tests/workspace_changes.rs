//! Cancellation boundaries of the runtime-owned optional workspace observation.
use crate::runtime::conversation_runtime::test_support::*;
use std::{fs, path::Path, sync::Mutex, time::Duration};

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
    configuration: HeldObservation,
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
                configuration.arm(context.workspace_changes.as_ref().unwrap());
                success_result("changed")
            }),
            cancellation,
        )
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

#[derive(Clone, Default)]
struct HeldObservation {
    gate: Arc<Mutex<Option<ObservationGate>>>,
    armed: Arc<tokio::sync::Notify>,
}

type ObservationGate = (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
);

impl HeldObservation {
    fn arm(&self, recording: &crate::workspace_changes::ChangeRecording) {
        *self.gate.lock().unwrap() = Some(recording.hold_final_observation());
        self.armed.notify_one();
    }

    async fn wait_until_started(&self) -> tokio::sync::oneshot::Sender<()> {
        bounded(self.armed.notified()).await;
        let (entered, release) = self.gate.lock().unwrap().take().unwrap();
        bounded(entered).await.unwrap();
        release
    }

    fn assert_not_started(&self) {
        let (mut entered, _release) = self.gate.lock().unwrap().take().unwrap();
        assert!(
            entered.try_recv().is_err(),
            "cancelled attempt must not start its final observation"
        );
    }
}

fn configure_repository(dir: &tempfile::TempDir) -> HeldObservation {
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
    HeldObservation::default()
}

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    // A deadlock guard, not a performance assertion. The observation gate
    // cannot complete until explicitly released by the test.
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("cancelled observation must not await the held final observation")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_attempt_does_not_start_a_final_workspace_observation() {
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
    configuration.assert_not_started();
    assert!(
        runtime
            .tool_runtime()
            .workspace_changes
            .summary(&attempt)
            .is_none()
    );
    runtime.submit_inbound(text_content("next task")).unwrap();
    assert_eq!(
        bounded(runtime.child_attempt_settlement(1)).await.ordinal,
        2
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_final_observation_releases_terminal_and_handoff_before_observation_release() {
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
    let release = configuration.wait_until_started().await;
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
    // A new task completes while the old observation is still held.
    runtime.submit_inbound(text_content("next task")).unwrap();
    assert_eq!(
        bounded(runtime.child_attempt_settlement(1)).await.ordinal,
        2
    );
    let _ = release.send(());
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
