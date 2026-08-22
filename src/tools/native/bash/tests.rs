use super::{BashTestControl, BashTool, NAME};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
use crate::runtime::process_runner::{
    MSG_ALL_CHILDREN_REAPED, SupervisorEvent, read_supervisor_event,
};
use crate::runtime::types::CancellationReason;
use crate::tools::artifacts::ArtifactStore;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
use crate::tools::managed_output::ManagedToolOutput;
use crate::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolProgress,
};
use crate::tools::workspace::Workspace;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

struct NoopProgress;

impl ProgressReporter for NoopProgress {
    fn report(&self, _progress: ToolProgress) {}
}

fn fixture() -> (
    tempfile::TempDir,
    ArtifactStore,
    ManagedToolOutput,
    Workspace,
) {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let artifacts = ArtifactStore::new(ConversationId::new("conv-1"), dir.path().join("artifacts"))
        .expect("artifacts");
    let tool_output = ManagedToolOutput::new(
        ConversationId::new("conv-1"),
        dir.path().join("tool-output"),
    )
    .expect("managed tool output");
    let workspace = Workspace::new(&workspace_root).expect("workspace");
    (dir, artifacts, tool_output, workspace)
}

fn invocation(command: &str) -> ToolInvocation {
    ToolInvocation {
        call_id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-bash"),
        tool_name: NAME.to_owned(),
        mode: ToolInvocationMode::Foreground,
        arguments: serde_json::json!({"command": command}),
    }
}

/// The model-facing Bash deadline is measured in seconds; the tool
/// boundary converts it to the internal `Duration`.
fn invocation_with_timeout(command: &str, timeout_seconds: u64) -> ToolInvocation {
    ToolInvocation {
        call_id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-bash"),
        tool_name: NAME.to_owned(),
        mode: ToolInvocationMode::Foreground,
        arguments: serde_json::json!({"command": command, "timeout": timeout_seconds}),
    }
}

async fn run_with(
    command: &str,
    artifacts: &ArtifactStore,
    tool_output: &ManagedToolOutput,
    workspace: &Workspace,
) -> ToolExecutionResult {
    run_with_control(
        command.to_owned(),
        BashTestControl::new(),
        CancellationSignal::new(),
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        None,
    )
    .await
}

/// Executes one invocation through the executor with explicit test
/// control seams and a caller-controlled cancellation signal. Takes
/// owned values so it can be spawned without borrowing.
#[allow(clippy::too_many_arguments)] // a bounded test-only fixture surface
async fn run_with_control(
    command: String,
    control: BashTestControl,
    cancellation: CancellationSignal,
    artifacts: ArtifactStore,
    tool_output: ManagedToolOutput,
    workspace: Workspace,
    timeout_seconds: Option<u64>,
) -> ToolExecutionResult {
    let tool = BashTool::with_test_control(control);
    let reporter = NoopProgress;
    let context = ToolExecutionContext {
        conversation_id: &ConversationId::new("conv-1"),
        execution_id: None,
        cancellation: crate::runtime::cancellation::ExecutionCancellation::detached(
            cancellation,
            CancellationReason::UserRequested,
        ),
        workspace: &workspace,
        progress: &reporter,
        artifacts: &artifacts,
        tool_output: &tool_output,
        environment: &ToolEnvironment::new(),
        skill_resources: None,
    };
    let invocation = match timeout_seconds {
        Some(seconds) => invocation_with_timeout(&command, seconds),
        None => invocation(&command),
    };
    tool.execute(invocation, context).await
}

/// Whether a specific process still exists (signal-0 probe).
#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Err(nix::errno::Errno::ESRCH) => false,
        Ok(()) | Err(_) => true,
    }
}

/// The process-group id of a fixture process, from `/proc/<pid>/stat`
/// (test-only fixture-topology inspection; `/proc` is never the
/// production ownership authority).
#[cfg(unix)]
#[cfg(target_os = "linux")]
fn pgrp_of(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    fields.get(2)?.parse().ok()
}

#[cfg(target_os = "linux")]
fn process_capable_of_executing(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(close) = stat.rfind(')') else {
        return false;
    };
    !matches!(stat[close + 1..].split_whitespace().next(), Some("Z" | "X"))
}

#[cfg(target_os = "linux")]
async fn wait_until_not_executing(pid: i32) {
    for _ in 0..1000 {
        if !process_capable_of_executing(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("process {pid} remains capable of executing");
}

#[cfg(target_os = "linux")]
async fn start_supervisor_loss_fixture(
    cancellation: CancellationSignal,
) -> (
    tempfile::TempDir,
    tokio::task::JoinHandle<ToolExecutionResult>,
    crate::runtime::process_runner::RunnerChannelEofHook,
    i32,
    i32,
    i32,
) {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let inner_pid_file = root.join("inner.pid");
    let outer_pid_file = root.join("outer.pid");
    let ready_file = root.join("ready");
    let command = format!(
        "inner=$PPID; outer=$(awk '/^PPid:/ {{print $2}}' /proc/$inner/status); \
         echo $$ > {}; echo $inner > {}; echo $outer > {}; touch {}; \
         exec >/dev/null 2>&1; kill -KILL $outer; kill -KILL $inner; sleep 30",
        shell_pid_file.display(),
        inner_pid_file.display(),
        outer_pid_file.display(),
        ready_file.display()
    );
    let control = BashTestControl::new().observe_channel_eof();
    let eof = control.channel_eof().expect("EOF hook").clone();
    let task = tokio::spawn(run_with_control(
        command,
        control,
        cancellation,
        artifacts,
        tool_output,
        workspace,
        None,
    ));
    for _ in 0..1000 {
        if ready_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(ready_file.exists(), "supervisor-loss fixture not ready");
    let read_pid = |path: &std::path::Path| {
        std::fs::read_to_string(path)
            .expect("pid file")
            .trim()
            .parse::<i32>()
            .expect("pid")
    };
    let shell_pid = read_pid(&shell_pid_file);
    let inner_pid = read_pid(&inner_pid_file);
    let outer_pid = read_pid(&outer_pid_file);
    wait_until_not_executing(inner_pid).await;
    wait_until_not_executing(outer_pid).await;
    tokio::time::timeout(Duration::from_secs(8), eof.await_seen())
        .await
        .expect("control EOF was not observed");
    assert!(process_alive(shell_pid));
    assert_eq!(pgrp_of(shell_pid), Some(inner_pid));
    (dir, task, eof, shell_pid, inner_pid, outer_pid)
}

/// Polls a process until it is provably gone (ESRCH), with a strict
/// deadlock guard. Polling the authoritative OS process state with a
/// deadline is the test's proof; there is no assumed interleaving.
#[cfg(unix)]
async fn wait_for_process_death(pid: i32) {
    for _ in 0..1000 {
        if !process_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("process {pid} is still alive after the deadline");
}

/// Polls the invocation's process group until it is provably gone, with
/// a strict deadlock guard. Valid only after the invocation has
/// settled: the supervisor's final reap has then removed the group.
#[cfg(unix)]
async fn wait_for_group_death(pgid: i32) {
    use nix::errno::Errno;
    use nix::sys::signal::killpg;
    use nix::unistd::Pid;
    for _ in 0..1000 {
        match killpg(Pid::from_raw(pgid), None) {
            Ok(()) | Err(Errno::EPERM) => {}
            Err(_) => return,
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("process group {pgid} is still alive after the deadline");
}

/// A spill write failure is represented explicitly: the invocation fails
/// instead of reporting ordinary success while losing the promised retained
/// output. The command provably crosses the preview bound, so the lazy
/// spill allocation is actually reached.
#[tokio::test]
async fn spill_write_failure_fails_the_invocation_explicitly() {
    let (_dir, artifacts, tool_output, workspace) = fixture();
    tool_output.set_force_open_failures(true);
    let result = run_with(
        "yes x | head -c 40000",
        &artifacts,
        &tool_output,
        &workspace,
    )
    .await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "a spill capture failure must be an explicit failed result, got {:?}",
        result.status
    );
    assert!(
        !matches!(result.status, ToolExecutionStatus::Success),
        "successful retention must never be reported while full output is lost"
    );
}

/// A spill allocation failure (sequence exhaustion) is represented
/// explicitly as well.
#[tokio::test]
async fn spill_allocation_failure_fails_the_invocation_explicitly() {
    let (_dir, artifacts, tool_output, workspace) = fixture();
    tool_output.exhaust_sequence();
    let result = run_with(
        "yes x | head -c 40000",
        &artifacts,
        &tool_output,
        &workspace,
    )
    .await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "a spill allocation failure must be an explicit failed result, got {:?}",
        result.status
    );
    assert!(
        !matches!(result.status, ToolExecutionStatus::Success),
        "successful retention must never be reported while full output is lost"
    );
}

/// A spill WRITE failure after the spill was already allocated (the
/// deterministic `fail_writes_after` seam, not an open failure) is
/// represented explicitly: a successful process can never become a lossy
/// success, and no complete-output locator is advertised.
#[tokio::test]
async fn spill_write_failure_after_allocation_fails_the_invocation_explicitly() {
    let (_dir, artifacts, tool_output, workspace) = fixture();
    tool_output.fail_writes_after(0);
    let result = run_with(
        "yes x | head -c 40000",
        &artifacts,
        &tool_output,
        &workspace,
    )
    .await;
    let ToolExecutionStatus::Failed { error } = &result.status else {
        panic!(
            "a post-allocation spill write failure must be an explicit failed result, got {:?}",
            result.status
        );
    };
    assert!(
        error.contains("output capture"),
        "the failure names the capture: {error}"
    );
    for block in &result.content {
        let text = format!("{block:?}");
        assert!(
            !text.contains("full_output"),
            "no complete-output locator is advertised: {text}"
        );
    }
    assert!(
        result.managed_output.is_none(),
        "the failed invocation carries no continuation metadata"
    );
    // The incomplete spill reached exactly one terminal storage state: the
    // failed invocation settled AFTER the capture cleanup, so no partial
    // file survives in the managed store.
    assert!(
        std::fs::read_dir(tool_output.root().join("results"))
            .expect("results dir")
            .next()
            .is_none(),
        "a failed foreground spill leaves no partial file behind"
    );
}

/// Cancellation owns the terminal outcome even when the capture later
/// fails: the semantic terminal winner stays `Cancelled`, the bounded
/// diagnostic is retained, and the partial spill is never advertised as
/// complete — no complete-output locator and no "complete output" claim.
#[cfg(unix)]
#[tokio::test]
async fn cancellation_owns_the_outcome_and_a_failed_spill_is_never_advertised() {
    let (_dir, artifacts, tool_output, workspace) = fixture();
    // The spill opens at the crossing and its very first (prefix) write
    // fails: the partial file exists while the process keeps running.
    tool_output.fail_writes_after(0);
    let cancellation = CancellationSignal::new();
    let control = BashTestControl::new();
    let mut spill_started = control.spill_started_watcher();
    let task = tokio::spawn(run_with_control(
        "yes x".to_owned(),
        control,
        cancellation.clone(),
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        None,
    ));
    // Wait until the spill provably opened (and its write provably
    // failed) at the exact overflow transition, then cancel: cancellation
    // owns settlement. No filesystem polling, no timing assumption.
    tokio::time::timeout(
        Duration::from_secs(30),
        spill_started.wait_for(|started| *started),
    )
    .await
    .expect("the spill transition happens (liveness guard)")
    .expect("the spill watch stays open");
    let partial = tool_output.root().join("results/result_1.txt");
    assert!(partial.exists(), "the spill was allocated at the crossing");
    cancellation.cancel();
    let result = tokio::time::timeout(Duration::from_secs(30), task)
        .await
        .expect("the invocation settles")
        .expect("executor task");
    assert!(
        matches!(result.status, ToolExecutionStatus::Cancelled { .. }),
        "cancellation remains the semantic terminal winner, got {:?}",
        result.status
    );
    // The tool-owned JSON carries no magic output keys; the
    // complete-vs-partial truth is typed runtime-owned metadata, and the
    // removed partial spill means no locator exists at all.
    let content = result
        .content
        .iter()
        .find_map(|block| match block {
            crate::tools::types::ToolResultContent::Json { value } => Some(value.clone()),
            _ => None,
        })
        .expect("json content");
    assert!(
        content.get("full_output").is_none() && content.get("note").is_none(),
        "no magic continuation keys exist in tool-owned JSON: {content}"
    );
    let Some(crate::tools::types::ManagedOutputContinuation::Unavailable { diagnostic }) =
        &result.managed_output
    else {
        panic!(
            "a partial spill removed best-effort is typed Unavailable, got {:?}",
            result.managed_output
        );
    };
    assert!(
        diagnostic.contains("capture"),
        "the bounded capture diagnostic is retained: {diagnostic}"
    );
    // The foreground result presents its own continuation as ordinary
    // tool-owned text so the model learns the truth from the result.
    let note = result
        .content
        .iter()
        .find_map(|block| match block {
            crate::tools::types::ToolResultContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("the foreground continuation text block");
    assert!(
        note.contains("did not complete"),
        "the capture diagnostic reaches the model: {note}"
    );
    assert!(
        !note.contains("Complete output:"),
        "no complete-retention claim survives a failed spill: {note}"
    );
    assert!(
        !partial.exists(),
        "the partial spill file was removed best-effort"
    );
    let truncation = result
        .truncation
        .expect("an incomplete capture is truncated");
    assert!(truncation.truncated);
    assert_eq!(
        truncation.original_bytes, None,
        "the complete byte count is unknown for a partial capture"
    );
}

/// Small output never touches the managed tool-output store: the lazy
/// spill is not merely absent from the result, no file exists.
#[tokio::test]
async fn small_output_creates_no_spill_file() {
    let (_dir, artifacts, tool_output, workspace) = fixture();
    let result = run_with("echo hello", &artifacts, &tool_output, &workspace).await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert!(result.artifacts.is_empty(), "no semantic artifact exists");
    assert!(result.truncation.is_none(), "small output is not truncated");
    assert!(
        std::fs::read_dir(tool_output.root().join("results"))
            .expect("managed results root")
            .next()
            .is_none(),
        "small foreground output creates no result spill"
    );
    assert!(
        std::fs::read_dir(tool_output.root().join("tasks"))
            .expect("managed tasks root")
            .next()
            .is_none(),
        "a foreground execution owns no background live-output file"
    );
}

/// Output crossing the preview bound spills exactly once: the model-facing
/// result stays bounded and carries the absolute spill locator, and the
/// spill file retains the complete output from byte zero.
#[tokio::test]
async fn large_output_spills_lazily_with_an_absolute_locator() {
    let (_dir, artifacts, tool_output, workspace) = fixture();
    let result = run_with(
        "seq 1 9000; printf done",
        &artifacts,
        &tool_output,
        &workspace,
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert!(result.artifacts.is_empty(), "no semantic artifact exists");
    let truncation = result.truncation.expect("truncation state");
    assert!(truncation.truncated);
    let Some(original_bytes) = truncation.original_bytes else {
        panic!("the complete byte count is known");
    };
    let content = result
        .content
        .iter()
        .find_map(|block| match block {
            crate::tools::types::ToolResultContent::Json { value } => Some(value.clone()),
            _ => None,
        })
        .expect("json content");
    assert!(
        content["combined"].as_str().expect("combined").len()
            <= crate::tools::limits::BASH_STREAM_PREVIEW_BYTES,
        "the model-facing preview stays bounded"
    );
    assert!(
        content.get("full_output").is_none() && content.get("note").is_none(),
        "the tool-owned JSON carries no magic continuation keys: {content}"
    );
    let Some(crate::tools::types::ManagedOutputContinuation::Complete { locator }) =
        &result.managed_output
    else {
        panic!(
            "a complete foreground spill is typed Complete, got {:?}",
            result.managed_output
        );
    };
    let full_output = locator.to_str().expect("utf8 spill locator");
    assert!(
        std::path::Path::new(full_output).is_absolute(),
        "the spill locator is absolute: {full_output}"
    );
    assert!(
        full_output.starts_with(tool_output.root().to_str().expect("utf8 managed root")),
        "the spill locator lives under the managed root: {full_output}"
    );
    // The foreground result presents the locator to the model as ordinary
    // tool-owned text, including the Read/Grep continuation guidance.
    let continuation_text = result
        .content
        .iter()
        .find_map(|block| match block {
            crate::tools::types::ToolResultContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("the foreground continuation text block");
    assert!(
        continuation_text.contains(&format!("Complete output: {full_output}")),
        "the model-facing text carries the exact locator: {continuation_text}"
    );
    assert!(
        continuation_text.contains("Read or Grep"),
        "the model-facing text carries the continuation guidance: {continuation_text}"
    );
    let spilled = std::fs::read(full_output).expect("spill bytes");
    assert_eq!(spilled.len() as u64, original_bytes);
    let mut expected = String::new();
    for n in 1..=9000 {
        expected.push_str(&n.to_string());
        expected.push('\n');
    }
    expected.push_str("done");
    assert_eq!(
        spilled,
        expected.as_bytes(),
        "the complete output is retained verbatim from byte zero"
    );
    assert_eq!(
        std::fs::read_dir(tool_output.root().join("results"))
            .expect("managed results root")
            .count(),
        1,
        "exactly one spill file exists"
    );
}

/// A shell parent that exits while a redirected descendant stays in the
/// owned process domain (`sleep 30 >/dev/null 2>&1 & exit 0`) cannot
/// settle the invocation: the descendant no longer holds the rustX
/// pipes, so the capture alone would finish — but the supervisor still
/// owns the descendant and the invocation stays active until the
/// timeout settles it.
#[cfg(unix)]
#[tokio::test]
async fn redirected_descendant_does_not_escape_the_owned_domain() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let desc_pid_file = root.join("desc.pid");
    let anchor_pid_file = root.join("anchor.pid");
    let command = format!(
        "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
        shell_pid_file.display(),
        desc_pid_file.display()
    );
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_with_control(
            command,
            BashTestControl::new().anchor_pid_file(anchor_pid_file.clone()),
            CancellationSignal::new(),
            artifacts,
            tool_output,
            workspace,
            Some(1),
        ),
    )
    .await
    .expect("the invocation settles exactly once");
    assert_eq!(
        result.status,
        ToolExecutionStatus::TimedOut,
        "a redirected descendant must not let the invocation settle as Success"
    );
    let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
        .expect("descendant pid file")
        .trim()
        .parse()
        .expect("descendant pid");
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    // The owned process group is quiescent and the descendant is gone.
    wait_for_group_death(anchor_pid).await;
    wait_for_process_death(descendant_pid).await;
    let _ = dir;
}

/// The exact shell-exit boundary regression: the executor provably
/// observed the shell parent's natural exit (the supervisor's report)
/// and parked before any settlement handling; the descendant is
/// provably alive at that boundary; only then does cancellation become
/// observable. The result is `Cancelled` and the owned group is
/// terminated.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancellation_after_exact_shell_exit_boundary_terminates_the_owned_group() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let desc_pid_file = root.join("desc.pid");
    let anchor_pid_file = root.join("anchor.pid");
    let command = format!(
        "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
        shell_pid_file.display(),
        desc_pid_file.display()
    );
    let control = BashTestControl::new()
        .pause_at_shell_exit()
        .anchor_pid_file(anchor_pid_file.clone());
    let hook = control.lifecycle().clone();
    let cancellation = CancellationSignal::new();
    let cancelling = cancellation.clone();
    let task = tokio::spawn(run_with_control(
        command,
        control,
        cancellation,
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        None,
    ));
    // 1. The exact boundary: the shell parent exited and the executor
    //    is parked before natural settlement handling.
    tokio::time::timeout(Duration::from_secs(15), hook.await_shell_exit())
        .await
        .expect("the shell-exit boundary is observed");
    // 2. The descendant is provably still alive at the boundary.
    let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
        .expect("descendant pid file")
        .trim()
        .parse()
        .expect("descendant pid");
    assert!(
        process_alive(descendant_pid),
        "the descendant must still be alive at the exact shell-exit boundary"
    );
    // 3. Cancellation becomes observable after the boundary.
    cancelling.cancel();
    // 4. The executor resumes.
    hook.release();
    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("the invocation settles exactly once")
        .expect("executor task");
    assert!(
        matches!(result.status, ToolExecutionStatus::Cancelled { .. }),
        "late cancellation after the shell-parent exit must be Cancelled, got {:?}",
        result.status
    );
    // 5. The owned group is terminated and quiescent; the descendant is
    //    gone.
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    wait_for_group_death(anchor_pid).await;
    wait_for_process_death(descendant_pid).await;
    let _ = dir;
}

/// Natural settlement requires the supervisor's terminal child set: at
/// the exact shell-exit boundary the invocation is provably not yet
/// settled while the descendant is alive; once the descendant exits
/// naturally and the supervisor reaps it, the shell's natural
/// successful exit settles the invocation as `Success`.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn natural_success_requires_terminal_child_ownership() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let desc_pid_file = root.join("desc.pid");
    let anchor_pid_file = root.join("anchor.pid");
    let command = format!(
        "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
        shell_pid_file.display(),
        desc_pid_file.display()
    );
    let control = BashTestControl::new()
        .pause_at_shell_exit()
        .anchor_pid_file(anchor_pid_file.clone());
    let hook = control.lifecycle().clone();
    let task = tokio::spawn(run_with_control(
        command,
        control,
        CancellationSignal::new(),
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        None,
    ));
    // 1. The exact boundary: shell exited, executor parked, descendant
    //    still alive — the invocation must not have settled yet.
    tokio::time::timeout(Duration::from_secs(15), hook.await_shell_exit())
        .await
        .expect("the shell-exit boundary is observed");
    let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
        .expect("descendant pid file")
        .trim()
        .parse()
        .expect("descendant pid");
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    assert!(process_alive(descendant_pid));
    // 2. The test terminates the invocation group directly (test-side
    //    process control, deterministic: no timing assumption). The
    //    inner supervisor dies with it; its children reparent to the
    //    outer supervisor, which reaps them to its ECHILD.
    nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(anchor_pid),
        nix::sys::signal::Signal::SIGKILL,
    )
    .expect("test kills the owned group");
    wait_for_process_death(descendant_pid).await;
    // 3. The executor resumes and observes the terminal child set.
    hook.release();
    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("the invocation settles exactly once")
        .expect("executor task");
    assert_eq!(
        result.status,
        ToolExecutionStatus::Success,
        "once the owned child set is terminal, the shell's natural exit settles Success"
    );
    let exit_code = result
        .content
        .iter()
        .find_map(|content| match content {
            crate::tools::types::ToolResultContent::Json { value } => value["exit_code"].as_i64(),
            _ => None,
        })
        .expect("exit code in the JSON result");
    assert_eq!(exit_code, 0);
    let _ = dir;
}

/// A signaling failure during cancellation is an explicit failed
/// result — and the failure is terminal with respect to the owned
/// process tree. The inner supervisor refuses the group signal and
/// escalates containment to the outer supervisor, which emits exactly
/// one structurally-anchored fallback `SIGKILL` against the owned
/// group; `Failed` is returned only after the shell, the descendant,
/// and the whole group are provably gone. No test-side process control
/// is involved after the result settles.
#[cfg(unix)]
#[tokio::test]
async fn signal_failure_settles_as_an_explicit_failed_result() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let desc_pid_file = root.join("desc.pid");
    let anchor_pid_file = root.join("anchor.pid");
    // The shell records its own pid and the descendant's pid so the
    // test can prove both are terminal when the result exists.
    let command = format!(
        "echo $$ > {}; sleep 30 & echo $! > {}; wait",
        shell_pid_file.display(),
        desc_pid_file.display()
    );
    let cancellation = CancellationSignal::new();
    let cancelling = cancellation.clone();
    let control = BashTestControl::new()
        .fail_signal()
        .anchor_pid_file(anchor_pid_file.clone());
    let task = tokio::spawn(run_with_control(
        command,
        control.clone(),
        cancellation,
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        None,
    ));
    // The shell provably started (its pid file exists) before the
    // cancellation becomes observable.
    for _ in 0..1000 {
        if shell_pid_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(shell_pid_file.exists(), "the shell pid file never appeared");
    cancelling.cancel();
    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("the invocation settles")
        .expect("executor task");
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "an injected signaling failure must be an explicit failed result, got {:?}",
        result.status
    );
    // The failure was terminal: the shell, the descendant, and the
    // whole owned group are provably gone by the time the result
    // exists. The outer supervisor's fallback containment did the
    // work; there is no test-side kill to perform.
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
        .expect("descendant pid file")
        .trim()
        .parse()
        .expect("descendant pid");
    let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
        .expect("shell pid file")
        .trim()
        .parse()
        .expect("shell pid");
    wait_for_group_death(anchor_pid).await;
    wait_for_process_death(shell_pid).await;
    wait_for_process_death(descendant_pid).await;
    // The containment path is the recorded proof: the inner refused
    // the group TERM (emitted == false), and the outer's fallback
    // emitted exactly one SIGKILL against exactly the anchored pgid.
    let recorded = control.recorded_signals();
    let refusals: Vec<_> = recorded.iter().filter(|attempt| !attempt.emitted).collect();
    assert!(
        !refusals.is_empty(),
        "the injected signaling failure must have refused the group TERM"
    );
    let kills: Vec<_> = recorded.iter().filter(|attempt| attempt.emitted).collect();
    assert_eq!(
        kills.len(),
        1,
        "fallback containment emits exactly one SIGKILL, got: {recorded:?}"
    );
    assert_eq!(kills[0].signal, "SIGKILL");
    assert_eq!(kills[0].pgid, anchor_pid);
    let _ = dir;
}

/// A supervisor setup failure is an explicit failed result: the
/// invocation never claims a lifecycle it cannot establish.
#[tokio::test]
async fn supervisor_setup_failure_settles_as_an_explicit_failed_result() {
    let (_dir, artifacts, tool_output, workspace) = fixture();
    let result = run_with_control(
        "echo hi".to_owned(),
        BashTestControl::new().fail_supervisor_spawn(),
        CancellationSignal::new(),
        artifacts,
        tool_output,
        workspace,
        None,
    )
    .await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "an injected supervisor setup failure must be an explicit failed result, got {:?}",
        result.status
    );
    assert!(
        !matches!(result.status, ToolExecutionStatus::Success),
        "a failed supervisor setup must never be reported as success"
    );
}

/// A bash spawn failure inside the supervisor is an explicit failed
/// result as well.
#[tokio::test]
async fn bash_spawn_failure_settles_as_an_explicit_failed_result() {
    let (_dir, artifacts, tool_output, workspace) = fixture();
    let result = run_with_control(
        "echo hi".to_owned(),
        BashTestControl::new().fail_bash_spawn(),
        CancellationSignal::new(),
        artifacts,
        tool_output,
        workspace,
        None,
    )
    .await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "an injected bash spawn failure must be an explicit failed result, got {:?}",
        result.status
    );
    assert!(
        !matches!(result.status, ToolExecutionStatus::Success),
        "a failed bash spawn must never be reported as success"
    );
}

/// A SIGTERM handler-installation failure inside the supervisor is a
/// pre-ownership setup failure: no bash tree exists, so the explicit
/// failed result is the correct settlement.
#[tokio::test]
async fn sigterm_handler_setup_failure_settles_as_an_explicit_failed_result() {
    let (_dir, artifacts, tool_output, workspace) = fixture();
    let result = run_with_control(
        "echo hi".to_owned(),
        BashTestControl::new().fail_sigterm_handler(),
        CancellationSignal::new(),
        artifacts,
        tool_output,
        workspace,
        None,
    )
    .await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "an injected SIGTERM handler failure must be an explicit failed result, got {:?}",
        result.status
    );
    assert!(
        !matches!(result.status, ToolExecutionStatus::Success),
        "a failed SIGTERM handler setup must never be reported as success"
    );
}

/// A wait/reap failure after ownership is established is an explicit
/// failed result — and the failure is terminal with respect to the
/// owned process tree. The fixture has a real descendant: the inner
/// supervisor fails the shell wait and escalates containment to the
/// outer supervisor, which terminates the owned group; `Failed` is
/// returned only after the descendant and the group are provably gone.
/// No test-side process control follows the result.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn wait_failure_settles_as_an_explicit_failed_result() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let desc_pid_file = root.join("desc.pid");
    let anchor_pid_file = root.join("anchor.pid");
    // The shell exits immediately, so the injected wait failure fires
    // while the redirected descendant is still owned and alive.
    let command = format!(
        "sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
        desc_pid_file.display()
    );
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_with_control(
            command,
            BashTestControl::new()
                .fail_wait()
                .anchor_pid_file(anchor_pid_file.clone()),
            CancellationSignal::new(),
            artifacts,
            tool_output,
            workspace,
            None,
        ),
    )
    .await
    .expect("the invocation settles exactly once");
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "an injected wait failure must be an explicit failed result, got {:?}",
        result.status
    );
    // The failure was terminal: the descendant and the whole owned
    // group are provably gone by the time the result exists, with no
    // test-side kill.
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
        .expect("descendant pid file")
        .trim()
        .parse()
        .expect("descendant pid");
    wait_for_group_death(anchor_pid).await;
    wait_for_process_death(descendant_pid).await;
    let _ = dir;
}

/// The PGID-reuse fail-safe regression, strengthened for fallback
/// containment: the inner supervisor's ownership anchor reads as lost
/// (the exact test seam, never a probabilistic PID reuse), so the
/// inner refuses every group signal. Containment escalates to the
/// outer supervisor, whose structural anchor — the un-reaped inner
/// pid, which is the invocation's process-group id — is still provably
/// held, and which emits exactly one fallback `SIGKILL` against
/// exactly that anchored pgid. The owned tree dies with it and the
/// invocation settles `Failed`; no foreign process group is ever
/// signaled and no test-side kill is involved.
#[cfg(unix)]
#[tokio::test]
async fn no_signals_are_issued_after_ownership_loss() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let desc_pid_file = root.join("desc.pid");
    let anchor_pid_file = root.join("anchor.pid");
    let command = format!(
        "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; wait",
        shell_pid_file.display(),
        desc_pid_file.display()
    );
    let control = BashTestControl::new().anchor_pid_file(anchor_pid_file.clone());
    // 1. The ownership anchor reads as lost from the start: the inner
    //    supervisor behaves as if the owned group's lifetime had ended
    //    and the numeric pgid might name a foreign group.
    control
        .force_anchor_loss_handle()
        .store(true, Ordering::SeqCst);
    let cancellation = CancellationSignal::new();
    let cancelling = cancellation.clone();
    let task = tokio::spawn(run_with_control(
        command,
        control.clone(),
        cancellation,
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        None,
    ));
    // 2. The owned group provably exists: the shell is running.
    for _ in 0..1000 {
        if shell_pid_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(shell_pid_file.exists(), "the shell pid file never appeared");
    // 3. Cancellation becomes observable; the inner supervisor refuses
    //    to signal the (per its seam, possibly foreign) numeric pgid.
    cancelling.cancel();
    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("the invocation settles")
        .expect("executor task");
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "lost ownership must settle explicitly Failed, got {:?}",
        result.status
    );
    // 4. The inner supervisor emitted zero signals: every inner attempt
    //    was refused (emitted == false) and targeted the numeric pgid
    //    under question. The single emitted signal is the outer
    //    supervisor's fallback containment SIGKILL against exactly the
    //    structurally anchored pgid, issued only after the inner's
    //    refusals.
    let recorded = control.recorded_signals();
    assert!(
        !recorded.is_empty(),
        "a cancellation was attempted and must have reached the signal path"
    );
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    for attempt in &recorded {
        assert_eq!(
            attempt.pgid, anchor_pid,
            "every attempt targets the numeric pgid under question"
        );
    }
    let first_emitted = recorded
        .iter()
        .position(|attempt| attempt.emitted)
        .expect("the outer fallback containment must emit exactly one signal");
    assert_eq!(
        recorded.iter().filter(|attempt| attempt.emitted).count(),
        1,
        "the only emitted signal is the outer's structural fallback containment, got: {recorded:?}"
    );
    assert_eq!(recorded[first_emitted].signal, "SIGKILL");
    assert!(
        recorded[..first_emitted]
            .iter()
            .all(|attempt| !attempt.emitted),
        "every inner attempt before the fallback containment was refused"
    );
    // 5. The owned group was contained and is terminal: the group and
    //    the descendant are provably gone without any test-side kill.
    wait_for_group_death(anchor_pid).await;
    let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
        .expect("descendant pid file")
        .trim()
        .parse()
        .expect("descendant pid");
    wait_for_process_death(descendant_pid).await;
    let _ = dir;
}

/// The control-channel abandonment regression: the rustX-side owner of
/// the invocation disappears (the execution future is dropped, closing
/// the rustX end of the control channel) while the owned tree is
/// running. The inner supervisor interprets the channel EOF as a
/// fail-safe instruction to contain the invocation, and the outer
/// supervisor terminates the owned group. Dropping the rustX-side
/// execution future can therefore never detach an uncontrolled Bash
/// tree; the test performs no process control of its own.
#[cfg(unix)]
#[tokio::test]
async fn control_channel_abandonment_contains_the_owned_tree() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let desc_pid_file = root.join("desc.pid");
    let anchor_pid_file = root.join("anchor.pid");
    let command = format!(
        "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; wait",
        shell_pid_file.display(),
        desc_pid_file.display()
    );
    let task = tokio::spawn(run_with_control(
        command,
        BashTestControl::new().anchor_pid_file(anchor_pid_file.clone()),
        CancellationSignal::new(),
        artifacts,
        tool_output,
        workspace,
        None,
    ));
    // The owned tree provably exists before the owner disappears. The
    // descendant pid file is part of the readiness condition: the shell
    // writes it strictly after its own pid file, so waiting only for the
    // latter would race the descendant's `echo $!`.
    for _ in 0..1000 {
        if shell_pid_file.exists() && anchor_pid_file.exists() && desc_pid_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(shell_pid_file.exists(), "the shell pid file never appeared");
    assert!(
        desc_pid_file.exists(),
        "the descendant pid file never appeared"
    );
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
        .expect("descendant pid file")
        .trim()
        .parse()
        .expect("descendant pid");
    assert!(
        process_alive(descendant_pid),
        "the descendant must be alive when the owner disappears"
    );
    // The execution owner disappears: the future is dropped with no
    // cancellation request and no result. The test is about ownership
    // containment, not about a returned ToolExecutionResult.
    task.abort();
    let _ = task.await;
    // The supervisor fail-safe-contained the invocation: the
    // descendant and the whole owned group are provably gone without
    // any test-side kill.
    wait_for_process_death(descendant_pid).await;
    wait_for_group_death(anchor_pid).await;
    let _ = dir;
}

/// The fallback-containment counterpart of the unrelated-process
/// regression: when the inner supervisor fails to signal and the outer
/// supervisor must contain the invocation, only the invocation's own
/// session-isolated process group is terminated; an unrelated process
/// in the test's own process group survives.
#[cfg(unix)]
#[tokio::test]
async fn fallback_containment_does_not_kill_unrelated_processes() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let anchor_pid_file = root.join("anchor.pid");
    // An unrelated process in the test's own process group.
    let unrelated = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("unrelated sleep");
    let unrelated_pid = unrelated.id();
    let command = format!("echo $$ > {}; sleep 30", shell_pid_file.display());
    let cancellation = CancellationSignal::new();
    let cancelling = cancellation.clone();
    let task = tokio::spawn(run_with_control(
        command,
        BashTestControl::new()
            .fail_signal()
            .anchor_pid_file(anchor_pid_file.clone()),
        cancellation,
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        None,
    ));
    // The owned shell provably started before cancellation.
    for _ in 0..1000 {
        if shell_pid_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(shell_pid_file.exists(), "the shell pid file never appeared");
    cancelling.cancel();
    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("the invocation settles")
        .expect("executor task");
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "an injected signaling failure must be an explicit failed result, got {:?}",
        result.status
    );
    // The unrelated process in the test's own process group survived
    // the fallback containment of the invocation's session-isolated
    // group.
    let mut unrelated = unrelated;
    assert!(
        unrelated.try_wait().expect("try_wait").is_none(),
        "the unrelated process (pid {unrelated_pid}) must survive fallback containment"
    );
    let _ = unrelated.kill();
    let _ = unrelated.wait();
    // The owned group is terminal.
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    wait_for_group_death(anchor_pid).await;
    let _ = dir;
}

/// The positive counterpart: during a real cancellation every emitted
/// process-group signal targets exactly the invocation's own pgid (the
/// inner supervisor's pid) and only occurs while the ownership anchor
/// is held.
#[cfg(unix)]
#[tokio::test]
async fn cancellation_signals_only_target_the_owned_group() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let anchor_pid_file = root.join("anchor.pid");
    let command = format!(
        "trap '' TERM; echo $$ > {}; sleep 30",
        shell_pid_file.display()
    );
    let control = BashTestControl::new().anchor_pid_file(anchor_pid_file.clone());
    let cancellation = CancellationSignal::new();
    let cancelling = cancellation.clone();
    let task = tokio::spawn(run_with_control(
        command,
        control.clone(),
        cancellation,
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        None,
    ));
    for _ in 0..1000 {
        if shell_pid_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(shell_pid_file.exists(), "the shell pid file never appeared");
    cancelling.cancel();
    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("the invocation settles")
        .expect("executor task");
    assert!(
        matches!(result.status, ToolExecutionStatus::Cancelled { .. }),
        "a TERM-ignoring shell is KILLed and the cancellation settles, got {:?}",
        result.status
    );
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    let recorded = control.recorded_signals();
    assert!(
        !recorded.is_empty(),
        "the termination path must have emitted TERM and/or KILL"
    );
    for attempt in &recorded {
        assert!(
            attempt.emitted,
            "every attempt during a held anchor is emitted: {attempt:?}"
        );
        assert_eq!(
            attempt.pgid, anchor_pid,
            "every emitted signal targets the owned process-group id"
        );
    }
    wait_for_group_death(anchor_pid).await;
    let _ = dir;
}

/// The descendant-replacement race regression: A (a subshell) creates B
/// (a redirected descendant), A exits, B remains owned. At the exact
/// shell-exit boundary the executor is parked with B provably alive, so
/// the invocation cannot settle; only the invocation timeout can settle
/// it, and only after the supervisor has reaped B. This is the race the
/// old `/proc` walk could not prove: settlement is gated on the
/// supervisor's kernel child-wait terminal state, not on an
/// observational membership scan.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn descendant_replacement_keeps_the_invocation_active_until_reaped() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let a_pid_file = root.join("a.pid");
    let b_pid_file = root.join("b.pid");
    let anchor_pid_file = root.join("anchor.pid");
    let command = format!(
        "(sleep 30 >/dev/null 2>&1 & echo $! > {}) & echo $! > {}; wait; exit 0",
        b_pid_file.display(),
        a_pid_file.display()
    );
    let control = BashTestControl::new()
        .pause_at_shell_exit()
        .anchor_pid_file(anchor_pid_file.clone());
    let hook = control.lifecycle().clone();
    let task = tokio::spawn(run_with_control(
        command,
        control,
        CancellationSignal::new(),
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        Some(1),
    ));
    // 1. The exact boundary: the shell exited after waiting for A; the
    //    executor is parked before any settlement handling.
    tokio::time::timeout(Duration::from_secs(15), hook.await_shell_exit())
        .await
        .expect("the shell-exit boundary is observed");
    // 2. A is gone and B (A's replacement) is provably still alive and
    //    owned by the invocation supervisor.
    let a_pid: i32 = std::fs::read_to_string(&a_pid_file)
        .expect("a pid file")
        .trim()
        .parse()
        .expect("a pid");
    let b_pid: i32 = std::fs::read_to_string(&b_pid_file)
        .expect("b pid file")
        .trim()
        .parse()
        .expect("b pid");
    assert!(
        !process_alive(a_pid),
        "A must be terminal at the shell-exit boundary"
    );
    assert!(
        process_alive(b_pid),
        "B must still be owned and alive at the shell-exit boundary"
    );
    // 3. The executor resumes; the invocation must NOT settle while B is
    //    owned — only the invocation timeout can settle it.
    hook.release();
    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("the invocation settles exactly once")
        .expect("executor task");
    assert_eq!(
        result.status,
        ToolExecutionStatus::TimedOut,
        "the invocation must stay active while the supervisor owns B"
    );
    // 4. After the termination the whole owned domain is terminal.
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    wait_for_group_death(anchor_pid).await;
    wait_for_process_death(b_pid).await;
    let _ = dir;
}

/// The direct `setsid` escape-attempt regression: membership mutation
/// is rejected for bash descendants (the inherited syscall restriction),
/// so `setsid sleep 30` cannot leave the invocation group. The `setsid`
/// utility fails deterministically with `EPERM` and exits non-zero; the
/// recorded pid is provably terminal afterwards — nothing escaped the
/// owned domain — and the shell's natural exit settles ordinary
/// `Success` once the owned group is terminal.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn setsid_escape_attempt_is_rejected_and_nothing_escapes() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let attempt_pid_file = root.join("attempt.pid");
    let anchor_pid_file = root.join("anchor.pid");
    let command = format!(
        "echo $$ > {}; setsid sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
        shell_pid_file.display(),
        attempt_pid_file.display()
    );
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_with_control(
            command,
            BashTestControl::new().anchor_pid_file(anchor_pid_file.clone()),
            CancellationSignal::new(),
            artifacts,
            tool_output,
            workspace,
            Some(10),
        ),
    )
    .await
    .expect("the invocation settles exactly once (bounded)");
    assert_eq!(
        result.status,
        ToolExecutionStatus::Success,
        "the rejected escape attempt leaves the owned group terminal; the natural exit settles Success, got {:?}",
        result.status
    );
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
        .expect("shell pid file")
        .trim()
        .parse()
        .expect("shell pid");
    let attempt_pid: i32 = std::fs::read_to_string(&attempt_pid_file)
        .expect("attempt pid file")
        .trim()
        .parse()
        .expect("attempt pid");
    // The owned group is terminal and the shell is gone; the escaped
    // `sleep` never came into existence — the recorded attempt pid is
    // provably dead instead of alive out of domain.
    wait_for_group_death(anchor_pid).await;
    wait_for_process_death(shell_pid).await;
    wait_for_process_death(attempt_pid).await;
    let _ = dir;
}

/// The direct `setsid` escape-attempt timeout regression: the shell
/// stays alive in the owned group while the rejected `setsid` attempt
/// cannot leave it. The invocation timeout owns the outcome and settles
/// `TimedOut` in bounded time with the whole owned group terminal; the
/// recorded attempt pid is provably dead afterwards.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn setsid_escape_attempt_times_out_with_the_owned_group() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let attempt_pid_file = root.join("attempt.pid");
    let anchor_pid_file = root.join("anchor.pid");
    let command = format!(
        "echo $$ > {}; setsid sleep 30 >/dev/null 2>&1 & echo $! > {}; sleep 30",
        shell_pid_file.display(),
        attempt_pid_file.display()
    );
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_with_control(
            command,
            BashTestControl::new().anchor_pid_file(anchor_pid_file.clone()),
            CancellationSignal::new(),
            artifacts,
            tool_output,
            workspace,
            Some(1),
        ),
    )
    .await
    .expect("the invocation settles exactly once (bounded)");
    assert_eq!(
        result.status,
        ToolExecutionStatus::TimedOut,
        "the timeout owns the owned group; the rejected attempt must not leave anything, got {:?}",
        result.status
    );
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
        .expect("shell pid file")
        .trim()
        .parse()
        .expect("shell pid");
    let attempt_pid: i32 = std::fs::read_to_string(&attempt_pid_file)
        .expect("attempt pid file")
        .trim()
        .parse()
        .expect("attempt pid");
    wait_for_group_death(anchor_pid).await;
    wait_for_process_death(shell_pid).await;
    wait_for_process_death(attempt_pid).await;
    let _ = dir;
}

/// The direct `setsid` escape-attempt cancellation regression:
/// cancellation terminates the owned group — the rejected attempt
/// cannot survive it — and settles `Cancelled` in bounded time with the
/// recorded attempt pid provably dead.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn setsid_escape_attempt_cancels_with_the_owned_group() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let attempt_pid_file = root.join("attempt.pid");
    let anchor_pid_file = root.join("anchor.pid");
    let command = format!(
        "echo $$ > {}; setsid sleep 30 >/dev/null 2>&1 & echo $! > {}; sleep 30",
        shell_pid_file.display(),
        attempt_pid_file.display()
    );
    let cancellation = CancellationSignal::new();
    let cancelling = cancellation.clone();
    let task = tokio::spawn(run_with_control(
        command,
        BashTestControl::new().anchor_pid_file(anchor_pid_file.clone()),
        cancellation,
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        None,
    ));
    // The shell provably started before cancellation becomes observable.
    for _ in 0..1000 {
        if shell_pid_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(shell_pid_file.exists(), "the shell pid file never appeared");
    cancelling.cancel();
    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("the invocation settles exactly once (bounded)")
        .expect("executor task");
    assert!(
        matches!(result.status, ToolExecutionStatus::Cancelled { .. }),
        "cancellation owns the owned group; the rejected attempt must not survive it, got {:?}",
        result.status
    );
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    let attempt_pid: i32 = std::fs::read_to_string(&attempt_pid_file)
        .expect("attempt pid file")
        .trim()
        .parse()
        .expect("attempt pid");
    wait_for_group_death(anchor_pid).await;
    wait_for_process_death(attempt_pid).await;
    let _ = dir;
}

/// The mandatory hidden-grandchild regression (reproducer): A (a
/// subshell) creates B (a redirected descendant), A itself attempts to
/// leave the invocation group/session via `exec setsid`, the main shell
/// exits. The invocation must NOT settle while B is owned: no canonical
/// result may become terminal while any process still belongs to the
/// invocation-owned process group. At the exact shell-exit boundary the
/// test proves the fixture topology before evaluating settlement.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn hidden_group_descendant_cannot_be_hidden_by_a_setsid_escape_attempt() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let a_pid_file = root.join("a.pid");
    let b_pid_file = root.join("b.pid");
    let anchor_pid_file = root.join("anchor.pid");
    let command = format!(
        "echo $$ > {}; sh -c 'sleep 30 >/dev/null 2>&1 & echo $! > {}; \
         exec setsid sleep 30 >/dev/null 2>&1' & echo $! > {}; exit 0",
        shell_pid_file.display(),
        b_pid_file.display(),
        a_pid_file.display()
    );
    let control = BashTestControl::new()
        .pause_at_shell_exit()
        .anchor_pid_file(anchor_pid_file.clone());
    let hook = control.lifecycle().clone();
    let task = tokio::spawn(run_with_control(
        command,
        control,
        CancellationSignal::new(),
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        Some(1),
    ));
    // 1. The exact shell-exit boundary: the main shell exited after
    //    backgrounding A; the executor is parked before any settlement
    //    handling.
    tokio::time::timeout(Duration::from_secs(15), hook.await_shell_exit())
        .await
        .expect("the shell-exit boundary is observed");
    // 2. A and B provably exist (A creates B before its own escape
    //    attempt). The poll queries the fixture's own pid files — the
    //    authoritative process state — with a strict deadlock guard.
    for _ in 0..1000 {
        if a_pid_file.exists() && b_pid_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
        .expect("shell pid file")
        .trim()
        .parse()
        .expect("shell pid");
    let a_pid: i32 = std::fs::read_to_string(&a_pid_file)
        .expect("a pid file")
        .trim()
        .parse()
        .expect("a pid");
    let b_pid: i32 = std::fs::read_to_string(&b_pid_file)
        .expect("b pid file")
        .trim()
        .parse()
        .expect("b pid");
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    assert!(
        !process_alive(shell_pid),
        "the shell parent must be terminal at the boundary"
    );
    assert!(
        process_alive(b_pid),
        "B must still be alive at the shell-exit boundary"
    );
    // 3. The authoritative fixture topology: B belongs to the
    //    invocation-owned process group, and A — if it is still alive —
    //    must belong to it too. No state may exist where A is out of
    //    group while a live B remains hidden inside the owned group.
    assert_eq!(
        pgrp_of(b_pid),
        Some(anchor_pid),
        "B must still belong to the invocation-owned process group"
    );
    if process_alive(a_pid) {
        assert_eq!(
            pgrp_of(a_pid),
            Some(anchor_pid),
            "A must still belong to the invocation-owned process group"
        );
    }
    // 4. The executor resumes; the invocation must NOT settle while B
    //    is owned — only the invocation timeout can settle it.
    hook.release();
    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("the invocation settles exactly once")
        .expect("executor task");
    assert_eq!(
        result.status,
        ToolExecutionStatus::TimedOut,
        "the invocation must stay active while B is owned, got {:?}",
        result.status
    );
    // 5. After the timeout-driven termination the whole owned domain is
    //    terminal: B and the group are gone (A either died in the group
    //    or was terminated with it).
    wait_for_process_death(b_pid).await;
    wait_for_group_death(anchor_pid).await;
    let _ = dir;
}

/// The bounded-settlement regression: the stdout capture reader is held
/// open deterministically past the bounded confirmation window (the
/// test-only seam, never a production configuration). The owned process
/// tree is already terminal, so only the capture can be wedged; the
/// state machine must still settle within the strict outer bound — the
/// capture is force-finalized (the reader task is aborted) and the
/// invocation settles as an explicit bounded `Failed`. No unbounded
/// wait remains.
#[cfg(unix)]
#[tokio::test]
async fn stuck_capture_settles_boundedly_as_an_explicit_failure() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let control = BashTestControl::new().hold_stdout_capture();
    let hold = control.capture_hold().expect("capture hold seam").clone();
    let task = tokio::spawn(run_with_control(
        "echo hello".to_owned(),
        control,
        CancellationSignal::new(),
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        None,
    ));
    // The stdout reader provably parked after EOF; the shell exited and
    // the owned group is terminal, so only the capture can be wedged.
    tokio::time::timeout(Duration::from_secs(15), hold.await_parked())
        .await
        .expect("the stdout reader parks after EOF");
    // The bounded confirmation window expires into an explicit bounded
    // failure within the strict outer bound.
    let result = tokio::time::timeout(Duration::from_secs(25), task)
        .await
        .expect("the invocation settles within the strict outer bound")
        .expect("executor task");
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "a wedged capture must settle as an explicit bounded failure, got {:?}",
        result.status
    );
    let _ = dir;
}

/// The process-confirmation watchdog records failure intent but cannot
/// commit a result before the authoritative terminal event is admitted.
#[cfg(unix)]
#[tokio::test]
async fn quiescence_watchdog_cannot_bypass_process_terminality() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let ready = workspace.root().join("ready");
    let control = BashTestControl::new().hold_terminal_event();
    let hold = control.terminal_hold().expect("terminal hold seam").clone();
    let cancellation = CancellationSignal::new();
    let task = tokio::spawn(run_with_control(
        format!("touch {}; sleep 30", ready.display()),
        control,
        cancellation.clone(),
        artifacts,
        tool_output,
        workspace,
        None,
    ));
    for _ in 0..1000 {
        if ready.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(ready.exists(), "the Bash fixture never became ready");
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(15), hold.await_held())
        .await
        .expect("the authoritative terminal event is held");
    tokio::time::timeout(Duration::from_secs(15), hold.await_watchdog())
        .await
        .expect("the quiescence watchdog expires");
    assert!(
        !task.is_finished(),
        "no ToolExecutionResult may commit while children_terminal is false"
    );
    hold.release();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the invocation settles after terminality is released")
        .expect("executor task");
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { ref error }
            if error.contains("did not become terminal")),
        "quiescence failure must outrank cancellation after terminality, got {:?}",
        result.status
    );
    let _ = dir;
}

/// The stopped-anchor regression: a `SIGSTOP` of the inner supervisor
/// freezes the whole containment chain (TERMINATE is never processed).
/// The outer supervisor detects the frozen anchor, un-wedges it with
/// `SIGKILL`, contains the invocation group, and the cancellation
/// settles normally with the owned group terminal — the bounded
/// confirmation path is never reached.
#[cfg(unix)]
#[tokio::test]
async fn stopped_anchor_supervisor_is_contained_by_the_outer() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let anchor_pid_file = root.join("anchor.pid");
    // The fixture freezes its own supervisor: bash's parent is the
    // inner supervisor (the invocation's anchor). `sleep 30` keeps the
    // owned group alive while the anchor is stopped.
    let command = format!(
        "echo $$ > {}; kill -STOP $PPID; sleep 30",
        shell_pid_file.display()
    );
    let cancellation = CancellationSignal::new();
    let cancelling = cancellation.clone();
    let task = tokio::spawn(run_with_control(
        command,
        BashTestControl::new().anchor_pid_file(anchor_pid_file.clone()),
        cancellation,
        artifacts.clone(),
        tool_output.clone(),
        workspace.clone(),
        None,
    ));
    for _ in 0..1000 {
        if shell_pid_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(shell_pid_file.exists(), "the shell pid file never appeared");
    cancelling.cancel();
    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("the invocation settles")
        .expect("executor task");
    assert!(
        matches!(result.status, ToolExecutionStatus::Cancelled { .. }),
        "a frozen anchor must still settle the owned group as Cancelled, got {:?}",
        result.status
    );
    let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
        .expect("anchor pid file")
        .trim()
        .parse()
        .expect("anchor pid");
    wait_for_group_death(anchor_pid).await;
    let _ = dir;
}

/// Catastrophic supervisor-loss reproducer. The shell kills both
/// supervisors after recording the fixed-group topology, redirects its
/// pipes, and remains alive. EOF must not settle the invocation.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn supervisor_chain_loss_does_not_make_an_owned_process_terminal() {
    let (dir, task, eof, shell_pid, _, _) =
        start_supervisor_loss_fixture(CancellationSignal::new()).await;
    assert!(
        !task.is_finished(),
        "control EOF fabricated process terminality while owned work was alive"
    );
    eof.release_emergency_containment();
    let result = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .expect("emergency containment did not settle")
        .expect("Bash task panicked");
    assert!(
        matches!(
        result.status,
        ToolExecutionStatus::Failed { ref error }
            if error.contains("exited before reporting terminal child ownership")
        ),
        "unexpected result: {:?}",
        result.status
    );
    wait_for_process_death(shell_pid).await;
    let _ = dir;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancellation_during_supervisor_loss_settles_failed_after_containment() {
    let cancellation = CancellationSignal::new();
    let (dir, task, eof, shell_pid, _, _) =
        start_supervisor_loss_fixture(cancellation.clone()).await;
    cancellation.cancel();
    assert!(!task.is_finished());
    assert!(process_capable_of_executing(shell_pid));
    eof.release_emergency_containment();
    let result = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .expect("containment settles")
        .expect("executor task");
    assert!(matches!(
        result.status,
        ToolExecutionStatus::Failed { ref error }
            if error.contains("exited before reporting terminal child ownership")
    ));
    wait_for_process_death(shell_pid).await;
    let _ = dir;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn timeout_during_supervisor_loss_settles_failed_after_containment() {
    let (dir, task, eof, shell_pid, _, _) =
        start_supervisor_loss_fixture(CancellationSignal::new()).await;
    eof.force_timeout();
    assert!(!task.is_finished());
    assert!(process_capable_of_executing(shell_pid));
    eof.release_emergency_containment();
    let result = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .expect("containment settles")
        .expect("executor task");
    assert!(matches!(
        result.status,
        ToolExecutionStatus::Failed { ref error }
            if error.contains("exited before reporting terminal child ownership")
    ));
    wait_for_process_death(shell_pid).await;
    let _ = dir;
}

/// The runtime child-subreaper initialization is a pre-ownership
/// prerequisite: a failure settles `Failed` with no Bash tree spawned —
/// catastrophic fallback containment is never assumed after the runtime
/// once failed to become a subreaper, and `START` can never be sent
/// without it. The injected failure proves the exact gate: the command
/// never runs, so its marker file never appears and no process group
/// signal is ever attempted.
#[cfg(unix)]
#[tokio::test]
async fn subreaper_initialization_failure_is_a_pre_ownership_setup_failure() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let ready = root.join("ready");
    let control = BashTestControl::new().fail_subreaper_init();
    let recorded = control.recorded_signals();
    let result = run_with_control(
        format!("touch {}", ready.display()),
        control,
        CancellationSignal::new(),
        artifacts,
        tool_output,
        workspace,
        None,
    )
    .await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { ref error }
            if error.contains("fallback containment")),
        "a failed child-subreaper initialization must be an explicit pre-ownership failure, got {:?}",
        result.status
    );
    assert!(
        !ready.exists(),
        "no Bash tree may be spawned after a child-subreaper initialization failure"
    );
    assert!(
        recorded.is_empty(),
        "no process-group signal may be attempted without subreaper authority"
    );
    let _ = dir;
}

/// The mandatory emergency-anchor-unavailable regression: catastrophic
/// emergency containment starts with `process_lifecycle == Owned`, the
/// anchor unavailable, and no prior `AllChildrenReaped`. The emergency
/// containment must NOT return `TerminalProven` (anchor `ECHILD` is
/// never a terminal process-group proof), so no `ToolExecutionResult`
/// may commit while the owned group still executes. The semantic state
/// is enough — no actual pid reuse is required.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn emergency_anchor_unavailable_never_settles_an_owned_invocation() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let shell_pid_file = root.join("shell.pid");
    let inner_pid_file = root.join("inner.pid");
    let outer_pid_file = root.join("outer.pid");
    let anchor_pid_file = root.join("anchor.pid");
    let ready_file = root.join("ready");
    // The fixture kills both supervisors, then becomes a single
    // long-lived owned process (`exec sleep 30`: the shell replaces
    // itself, so the group holds exactly one process with a known pid).
    let command = format!(
        "inner=$PPID; outer=$(awk '/^PPid:/ {{print $2}}' /proc/$inner/status); \
         echo $$ > {}; echo $inner > {}; echo $outer > {}; touch {}; \
         exec >/dev/null 2>&1; kill -KILL $outer; kill -KILL $inner; exec sleep 30",
        shell_pid_file.display(),
        inner_pid_file.display(),
        outer_pid_file.display(),
        ready_file.display()
    );
    let control = BashTestControl::new()
        .observe_channel_eof()
        .anchor_pid_file(anchor_pid_file.clone());
    control
        .force_emergency_anchor_unavailable_handle()
        .store(true, Ordering::SeqCst);
    let eof = control.channel_eof().expect("EOF hook").clone();
    let mut task = tokio::spawn(run_with_control(
        command,
        control.clone(),
        CancellationSignal::new(),
        artifacts,
        tool_output,
        workspace,
        None,
    ));
    for _ in 0..1000 {
        if ready_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(ready_file.exists(), "the fixture never became ready");
    let read_pid = |path: &std::path::Path| {
        std::fs::read_to_string(path)
            .expect("pid file")
            .trim()
            .parse::<i32>()
            .expect("pid")
    };
    let shell_pid = read_pid(&shell_pid_file);
    let inner_pid = read_pid(&inner_pid_file);
    wait_until_not_executing(inner_pid).await;
    tokio::time::timeout(Duration::from_secs(8), eof.await_seen())
        .await
        .expect("control EOF was not observed");
    assert!(
        process_capable_of_executing(shell_pid),
        "the owned group must still be executing when emergency containment runs"
    );
    // Emergency containment runs with the anchor unavailable; the
    // seam'd semantic state is the deterministic proof.
    eof.release_emergency_containment();
    // The invocation must NOT settle: `AnchorUnavailable` is never a
    // terminal proof and the lifecycle stays non-terminal.
    let still_pending = tokio::time::timeout(Duration::from_secs(2), &mut task)
        .await
        .is_err();
    assert!(
        still_pending,
        "an unavailable emergency anchor must never settle the owned invocation"
    );
    assert!(
        control.recorded_signals().is_empty(),
        "no process-group signal may be issued when the anchor is unavailable"
    );
    // Test-side cleanup (the invocation itself is provably non-terminal
    // by design in this state): terminate the owned group and reap the
    // adopted processes so no fixture process survives the test. The
    // emergency path correctly never consumed them, so the test reaps
    // them directly.
    nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(inner_pid),
        nix::sys::signal::Signal::SIGKILL,
    )
    .expect("test terminates the owned group");
    nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(inner_pid), None)
        .expect("reap the adopted anchor");
    nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(shell_pid), None)
        .expect("reap the adopted shell");
    task.abort();
    let _ = dir;
}

/// The normal-terminal-before-EOF regression: the authoritative
/// `AllChildrenReaped` frame is parsed first, then EOF follows (the
/// outer exits after the terminal acknowledgement). The invocation is
/// already terminal: the late EOF and the intentionally released anchor
/// behind it never trigger emergency containment and never override the
/// natural result with a failure.
#[cfg(unix)]
#[tokio::test]
async fn terminal_frame_then_eof_never_overrides_terminality() {
    let (dir, artifacts, tool_output, workspace) = fixture();
    let control = BashTestControl::new()
        .hold_terminal_event()
        .observe_channel_eof();
    let hold = control.terminal_hold().expect("terminal hold").clone();
    let eof = control.channel_eof().expect("EOF hook").clone();
    let task = tokio::spawn(run_with_control(
        "echo hello".to_owned(),
        control.clone(),
        CancellationSignal::new(),
        artifacts,
        tool_output,
        workspace,
        None,
    ));
    // 1. The authoritative terminal frame is parsed (its state
    //    transition is test-held only).
    tokio::time::timeout(Duration::from_secs(15), hold.await_held())
        .await
        .expect("the terminal frame is parsed");
    // 2. EOF provably arrives while terminality is already admitted;
    //    the EOF branch must skip emergency containment entirely.
    tokio::time::timeout(Duration::from_secs(15), eof.await_seen())
        .await
        .expect("EOF is observed after the terminal frame");
    assert!(!task.is_finished());
    // 3. Release: the invocation settles with the shell's natural
    //    result — no failure override merely because EOF followed the
    //    terminal frame.
    hold.release();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the invocation settles")
        .expect("executor task");
    assert_eq!(
        result.status,
        ToolExecutionStatus::Success,
        "the terminal frame remains authoritative; late EOF must not override it, got {:?}",
        result.status
    );
    #[cfg(target_os = "linux")]
    assert!(
        control.recorded_signals().is_empty(),
        "no containment signal may follow an already-admitted terminal frame"
    );
    // macOS settles the natural completion through the fallback containment
    // signal (its terminal proof), which is issued before — never after —
    // the terminal frame. Exactly that pre-terminal `SIGKILL` is recorded,
    // and nothing else.
    #[cfg(target_os = "macos")]
    {
        let signals = control.recorded_signals();
        assert_eq!(
            signals.len(),
            1,
            "exactly the pre-terminal fallback containment is recorded: {signals:?}"
        );
        assert_eq!(
            signals[0].signal, "SIGKILL",
            "the fallback containment is SIGKILL"
        );
        assert!(
            signals[0].emitted,
            "the fallback containment reaches the kernel"
        );
    }
    let _ = dir;
}

/// The concurrent catastrophic isolation regression: two independent
/// Bash invocations (A and B) both lose their supervisor units while
/// live owned descendants remain. Emergency containment of A must
/// signal and reap only group A: B's process group stays alive and
/// untouched, and only B's own emergency containment terminates it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn concurrent_supervisor_loss_containment_is_isolated() {
    let (dir_a, task_a, eof_a, shell_a, inner_a, _) =
        start_supervisor_loss_fixture(CancellationSignal::new()).await;
    let (dir_b, task_b, eof_b, shell_b, inner_b, _) =
        start_supervisor_loss_fixture(CancellationSignal::new()).await;
    assert!(process_capable_of_executing(shell_a));
    assert!(process_capable_of_executing(shell_b));
    // Contain A: B must remain completely untouched.
    eof_a.release_emergency_containment();
    let result_a = tokio::time::timeout(Duration::from_secs(8), task_a)
        .await
        .expect("invocation A settles")
        .expect("executor task A");
    assert!(matches!(
        result_a.status,
        ToolExecutionStatus::Failed { ref error }
            if error.contains("exited before reporting terminal child ownership")
    ));
    wait_for_process_death(shell_a).await;
    wait_for_group_death(inner_a).await;
    assert!(
        process_capable_of_executing(shell_b),
        "containing invocation A must never signal or reap invocation B"
    );
    // Contain B: only now does B become terminal.
    eof_b.release_emergency_containment();
    let result_b = tokio::time::timeout(Duration::from_secs(8), task_b)
        .await
        .expect("invocation B settles")
        .expect("executor task B");
    assert!(matches!(
        result_b.status,
        ToolExecutionStatus::Failed { ref error }
            if error.contains("exited before reporting terminal child ownership")
    ));
    wait_for_process_death(shell_b).await;
    wait_for_group_death(inner_b).await;
    let _ = (dir_a, dir_b);
}

/// The foreign-adopted-child negative isolation regression. U is a
/// **test-created foreign/unregistered hierarchy**: kernel subreaper
/// adoption makes the runtime process its OS parent, but U is outside
/// Bash semantic ownership and is not a supported production
/// rustX-owned execution in M5. The regression proves that catastrophic
/// Bash containment for invocation group G never touches U — it
/// signals only G's pgid and reaps only G's adopted children, never a
/// broad wait. U's cleanup is intentionally owned by the test (rustX
/// does not claim to generically reap unknown adopted children), and
/// the test reaps U before returning.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn bash_catastrophic_containment_does_not_touch_foreign_adopted_child() {
    crate::runtime::process_supervision::ensure_child_subreaper()
        .expect("the runtime process is a child subreaper");
    let (dir, _artifacts, _tool_output, workspace) = fixture();
    let root = workspace.root().to_path_buf();
    let u_pid_file = root.join("u.pid");
    // U: a test-created foreign hierarchy whose parent exits
    // immediately, so U orphans and reparents to the runtime process
    // (the nearest subreaper ancestor — the test binary itself).
    let mut sh = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "sleep 30 >/dev/null 2>&1 & echo $! > {}",
            u_pid_file.display()
        ))
        .spawn()
        .expect("spawn U's parent");
    let status = sh.wait().expect("U's parent exits");
    assert!(status.success());
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let u_pid: i32 = loop {
        if let Ok(content) = std::fs::read_to_string(&u_pid_file) {
            break content.trim().parse().expect("u pid");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "U's pid file never appeared"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    // U is adopted by the runtime process (test-only /proc fixture
    // inspection; /proc is never the production ownership authority).
    let self_pid = i32::try_from(std::process::id()).expect("pid fits i32");
    loop {
        let parent = std::fs::read_to_string(format!("/proc/{u_pid}/stat"))
            .ok()
            .and_then(|stat| {
                let close = stat.rfind(')')?;
                stat[close + 1..].split_whitespace().nth(1)?.parse().ok()
            });
        if parent == Some(self_pid) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "U was never adopted by the runtime process (parent: {parent:?})"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(process_alive(u_pid), "U must be alive before containment");
    // The catastrophic Bash invocation G loses both supervisors with
    // live owned work.
    let (dir_g, task, eof, shell_pid, inner_pid, _) =
        start_supervisor_loss_fixture(CancellationSignal::new()).await;
    eof.release_emergency_containment();
    let result = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .expect("invocation G settles")
        .expect("executor task G");
    assert!(matches!(
        result.status,
        ToolExecutionStatus::Failed { ref error }
            if error.contains("exited before reporting terminal child ownership")
    ));
    wait_for_process_death(shell_pid).await;
    wait_for_group_death(inner_pid).await;
    // U is untouched: still alive and still adopted by the runtime
    // process. Bash containment is scoped; M5 deliberately does not
    // reap foreign adopted children.
    assert!(
        process_alive(u_pid),
        "Bash catastrophic containment must never signal or reap a foreign adopted child"
    );
    // Test-side cleanup of U: the test is U's cleanup owner. This is
    // not missing production behavior — rustX does not generically
    // reap unknown adopted children in M5.
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(u_pid),
        nix::sys::signal::Signal::SIGKILL,
    )
    .expect("test terminates U");
    nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(u_pid), None).expect("reap U");
    let _ = (dir, dir_g);
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_frame_is_parsed_before_buffered_eof() {
    let (mut writer, mut reader) = tokio::net::UnixStream::pair().expect("socket pair");
    writer
        .write_all(&[1, 0, 0, 0, MSG_ALL_CHILDREN_REAPED])
        .await
        .expect("terminal frame");
    drop(writer);
    assert!(matches!(
        read_supervisor_event(&mut reader).await,
        Ok(Some(SupervisorEvent::AllChildrenReaped))
    ));
    assert!(matches!(read_supervisor_event(&mut reader).await, Ok(None)));
}

/// macOS-specific regressions for the Bash EXIT-trap and containment
/// contract.
///
/// macOS has no child-subreaper, so a background descendant that outlives
/// the shell is reparented to launchd and becomes invisible to the
/// supervisor's group-scoped wait. The injected `trap 'wait' EXIT` is a
/// best-effort convenience only — the user command runs in the same shell
/// and may legally clear or replace it — so the macOS terminal proof must
/// not depend on it. These tests prove that replacing the trap cannot
/// produce a false terminal result: the owned group is actively contained
/// (the background descendant is killed) instead of being reported terminal
/// while it may still run.
#[cfg(target_os = "macos")]
mod macos_exit_trap_tests {
    use super::{fixture, process_alive, run_with};
    use std::path::Path;
    use std::time::Duration;

    /// Reads a pid file the shell wrote before settling, with a deadlock
    /// guard (the write is an explicit fixture synchronization point, never
    /// a correctness assumption).
    fn read_pid(path: &Path) -> i32 {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(contents) = std::fs::read_to_string(path)
                && let Ok(pid) = contents.trim().parse::<i32>()
            {
                return pid;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pid file {} never appeared",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Polls a pid until it is provably gone (the signal-0 probe returns
    /// `ESRCH`), with a strict deadlock guard.
    async fn wait_for_process_death(pid: i32) {
        for _ in 0..1000 {
            if !process_alive(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("process {pid} is still alive after the deadline");
    }

    /// Runs one trap-replacement fixture and proves the owned background
    /// descendant was terminated, never falsely reported terminal.
    async fn assert_background_descendant_is_contained(trap_line: &str) {
        let (dir, artifacts, tool_output, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let child_pid_file = root.join("child.pid");
        // The shell backgrounds a long-lived descendant, writes its pid,
        // then replaces the injected EXIT trap and exits. Without the trap
        // the descendant is reparented to launchd; the macOS supervisor must
        // contain it via the outer's fallback `SIGKILL` rather than report a
        // false terminal while it still runs.
        let command = format!(
            "sleep 30 >/dev/null 2>&1 & echo $! > {}; {trap_line}; exit 0",
            child_pid_file.display()
        );
        let result = run_with(&command, &artifacts, &tool_output, &workspace).await;
        assert_eq!(
            result.status,
            crate::tools::types::ToolExecutionStatus::Success,
            "the shell exited 0, so the invocation settles as Success"
        );
        let child_pid = read_pid(&child_pid_file);
        wait_for_process_death(child_pid).await;
        let _ = dir;
    }

    /// Clearing the injected EXIT trap cannot produce a false terminal
    /// result: the reparented descendant is actively contained.
    #[tokio::test]
    async fn clearing_the_exit_trap_cannot_produce_false_terminal_settlement() {
        assert_background_descendant_is_contained("trap - EXIT").await;
    }

    /// Replacing the injected EXIT trap cannot produce a false terminal
    /// result either.
    #[tokio::test]
    async fn replacing_the_exit_trap_cannot_produce_false_terminal_settlement() {
        assert_background_descendant_is_contained("trap ':' EXIT").await;
    }
}

/// The macOS ownership-boundary regression: a descendant that deliberately
/// leaves the invocation process group via `setsid(2)` is outside rustX's
/// macOS ownership domain.
///
/// macOS has no seccomp fixed-membership primitive, so a command may legally
/// move a descendant out of the invocation process group/session. rustX owns
/// the invocation process-group domain: it does not track, contain, reap, or
/// wait for an escaped descendant, and settlement of the owned group does not
/// imply that descendant terminated. This test proves the real kernel
/// boundary — not a comment — and then cleans up the escaped process itself.
#[cfg(target_os = "macos")]
mod macos_escape_boundary_tests {
    use super::{fixture, process_alive, run_with_control};
    use crate::runtime::cancellation::CancellationSignal;
    use crate::tools::native::bash::BashTestControl;
    use crate::tools::types::ToolExecutionStatus;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use std::path::Path;
    use std::time::Duration;

    /// A small deterministic helper that records its pre-escape
    /// pid/process-group/session, calls the `setsid(2)` libc primitive
    /// directly via Python's `os.setsid`, records its post-escape
    /// pid/process-group/session, and then stays alive. The post-escape file
    /// is the deterministic synchronization point for the shell, and the
    /// explicit `kill` in the test is the only thing that terminates it.
    const ESCAPE_HELPER: &str = r#"import os
import sys
import time

pre, post = sys.argv[1], sys.argv[2]

with open(pre, "w") as f:
    f.write(f"{os.getpid()} {os.getpgrp()} {os.getsid(0)}\n")
    f.flush()
    os.fsync(f.fileno())

os.setsid()

with open(post, "w") as f:
    f.write(f"{os.getpid()} {os.getpgrp()} {os.getsid(0)}\n")
    f.flush()
    os.fsync(f.fileno())

while True:
    time.sleep(3600)
"#;

    /// Kills the escaped descendant on drop, so a failed assertion can never
    /// leak the helper process. Killing an already-dead pid is a harmless
    /// `ESRCH`.
    struct EscapedProcessCleanup(i32);

    impl Drop for EscapedProcessCleanup {
        fn drop(&mut self) {
            let _ = kill(Pid::from_raw(self.0), Signal::SIGKILL);
        }
    }

    /// Polls a pid until it is provably gone (the signal-0 probe returns
    /// `ESRCH`), with a strict deadlock guard.
    async fn wait_for_process_death(pid: i32) {
        for _ in 0..1000 {
            if !process_alive(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("process {pid} is still alive after the deadline");
    }

    /// Reads a `pid pgid sid` state file, polling with a strict deadlock
    /// guard: the helper's post-escape write is the deterministic
    /// synchronization point, never a timing assumption.
    fn read_state(path: &Path) -> (i32, i32, i32) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(contents) = std::fs::read_to_string(path) {
                let fields: Vec<i32> = contents
                    .split_whitespace()
                    .map(str::parse::<i32>)
                    .collect::<Result<_, _>>()
                    .expect("state file contains integers");
                if fields.len() == 3 {
                    return (fields[0], fields[1], fields[2]);
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "state file {} never appeared",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The escaped descendant is provably outside the invocation process
    /// group, the invocation still reaches its normal owned-PGID settlement,
    /// and that settlement does not imply the escaped descendant is gone.
    #[tokio::test]
    #[allow(clippy::similar_names)] // pid/pgid/sid are intentionally parallel process identities
    async fn escaped_descendant_is_outside_the_owned_group_and_survives_settlement() {
        let (dir, artifacts, tool_output, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let helper_path = root.join("escape_helper.py");
        let pre_path = root.join("escape.pre");
        let post_path = root.join("escape.post");
        let helper_pid_path = root.join("escape.pid");
        let anchor_pid_path = root.join("anchor.pid");
        std::fs::write(&helper_path, ESCAPE_HELPER).expect("write the escape helper");

        // The helper records its pre-escape state, calls setsid(2), records
        // its post-escape state, and stays alive. The shell clears the
        // injected EXIT trap (best-effort only, and the user command may
        // legally replace it), waits for the helper's post-escape write, then
        // exits so the invocation can settle its owned group normally.
        let command = format!(
            "trap - EXIT; python3 {} {} {} >/dev/null 2>&1 & echo $! > {}; \
             while [ ! -f {} ]; do sleep 0.05; done; exit 0",
            helper_path.display(),
            pre_path.display(),
            post_path.display(),
            helper_pid_path.display(),
            post_path.display()
        );
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            run_with_control(
                command,
                BashTestControl::new().anchor_pid_file(anchor_pid_path.clone()),
                CancellationSignal::new(),
                artifacts,
                tool_output,
                workspace,
                Some(10),
            ),
        )
        .await
        .expect("the invocation settles exactly once (bounded)");
        assert_eq!(
            result.status,
            ToolExecutionStatus::Success,
            "the owned invocation group settles normally with the escaped descendant out of domain"
        );

        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_path)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        let shell_recorded_pid: i32 = std::fs::read_to_string(&helper_pid_path)
            .expect("helper pid file")
            .trim()
            .parse()
            .expect("helper pid");
        let (pre_pid, pre_pgid, _pre_sid) = read_state(&pre_path);
        let (escaped_pid, escaped_pgid, escaped_sid) = read_state(&post_path);

        // Cleanup is armed before any assertion so a failure can never leak
        // the escaped helper.
        let _cleanup = EscapedProcessCleanup(escaped_pid);

        // The helper is the shell's real descendant: the shell-recorded
        // background pid matches the helper's own pid, and before escaping it
        // was a member of the invocation process group.
        assert_eq!(escaped_pid, shell_recorded_pid);
        assert_eq!(pre_pid, escaped_pid);
        assert_eq!(
            pre_pgid, anchor_pid,
            "the helper started inside the invocation process group"
        );

        // The escape is real: after setsid(2) the helper leads its own new
        // session/process group, so it is no longer in the invocation PGID.
        assert_eq!(
            escaped_pgid, escaped_pid,
            "setsid makes the helper its own process-group leader"
        );
        assert_eq!(
            escaped_sid, escaped_pid,
            "setsid makes the helper its own session leader"
        );
        assert_ne!(
            escaped_pgid, anchor_pid,
            "the helper left the invocation process group"
        );

        // The invocation settled its owned group, but that settlement does
        // not imply the escaped descendant is gone: it is still alive.
        assert!(
            process_alive(escaped_pid),
            "the escaped descendant is outside the owned group and must still be alive after settlement"
        );

        // The test itself owns the escaped process cleanup: the supervisor
        // never claimed to contain it.
        kill(Pid::from_raw(escaped_pid), Signal::SIGKILL).expect("kill the escaped descendant");
        wait_for_process_death(escaped_pid).await;
        let _ = dir;
    }
}
