//! M5 native Bash tool tests.
//!
//! Bash is Unix-first: `/bin/bash` semantics are never pretended to be
//! portable, so every test is `#[cfg(unix)]`. Tests use controlled
//! temporary workspaces and deterministic subprocess fixtures; wall-clock
//! waits appear only as the configured TERM grace period and as deadlock
//! guards — never as the proof of a concurrency invariant.

#![cfg(unix)]
#![allow(clippy::similar_names)] // scripted fixture names are intentionally similar

mod common;

use std::time::Duration;

use common::{
    native_fixture, native_fixture_with_environment, run_tool, run_tool_with_cancellation,
};
use rustx::runtime::CancellationSignal;
use rustx::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolResultContent,
};

fn json_content(result: &ToolExecutionResult) -> serde_json::Value {
    for content in &result.content {
        if let ToolResultContent::Json { value } = content {
            return value.clone();
        }
    }
    panic!("expected a JSON result content block");
}

/// The workspace-relative path of an artifact id.
fn artifact_path(fixture: &common::NativeFixture, id: &str) -> std::path::PathBuf {
    fixture.runtime.artifacts().root().join(format!("{id}.bin"))
}

#[tokio::test]
async fn bash_uses_full_shell_semantics() {
    let fixture = native_fixture();
    let result = run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "echo $((6*7)); for i in a b; do printf '%s ' $i; done"}),
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert_eq!(json_content(&result)["exit_code"], 0);
    assert!(
        json_content(&result)["stdout"]
            .as_str()
            .expect("stdout")
            .contains("42")
    );
    assert!(
        json_content(&result)["stdout"]
            .as_str()
            .expect("stdout")
            .contains("a b")
    );
}

#[tokio::test]
async fn bash_uses_the_fixed_workspace_directory() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    std::fs::write(root.join("marker.txt"), "here").expect("write marker");
    let result = run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "pwd; test -f marker.txt && echo found"}),
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    let content = json_content(&result);
    let stdout = content["stdout"].as_str().expect("stdout");
    assert!(
        stdout.contains(root.to_str().expect("utf8 root")),
        "cwd is the explicit workspace root, got: {stdout}"
    );
    assert!(stdout.contains("found"));
}

#[tokio::test]
async fn bash_has_no_state_persistence_between_calls() {
    let fixture = native_fixture();
    let first = run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "export PERSISTED=yes; cd /; echo one"}),
    )
    .await;
    assert_eq!(first.status, ToolExecutionStatus::Success);
    let second = run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "echo ${PERSISTED:-absent} $(pwd)"}),
    )
    .await;
    assert_eq!(second.status, ToolExecutionStatus::Success);
    let content = json_content(&second);
    let stdout = content["stdout"].as_str().expect("stdout");
    assert!(
        stdout.contains("absent"),
        "no shell state survives: {stdout}"
    );
    assert!(
        stdout.contains(fixture.runtime.workspace().root().to_str().expect("root")),
        "cwd resets to the workspace root"
    );
}

#[tokio::test]
async fn bash_captures_stdout_stderr_and_combined() {
    let fixture = native_fixture();
    let result = run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "echo out; echo err >&2"}),
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    let content = json_content(&result);
    assert_eq!(content["exit_code"], 0);
    assert!(content["stdout"].as_str().expect("stdout").contains("out"));
    assert!(content["stderr"].as_str().expect("stderr").contains("err"));
    let combined = content["combined"].as_str().expect("combined");
    assert!(combined.contains("out") && combined.contains("err"));
}

#[tokio::test]
async fn bash_zero_exit_is_success_and_nonzero_exit_is_a_failed_result() {
    let fixture = native_fixture();
    let zero = run_tool(&fixture, "bash", serde_json::json!({"command": "exit 0"})).await;
    assert_eq!(zero.status, ToolExecutionStatus::Success);
    assert_eq!(json_content(&zero)["exit_code"], 0);
    let nonzero = run_tool(&fixture, "bash", serde_json::json!({"command": "exit 7"})).await;
    match &nonzero.status {
        ToolExecutionStatus::Failed { error } => {
            assert!(error.contains('7'), "exit code preserved: {error}");
        }
        other => panic!("expected a failed result, got {other:?}"),
    }
    assert_eq!(json_content(&nonzero)["exit_code"], 7);
}

#[tokio::test]
async fn bash_foreground_timeout_is_timed_out() {
    let fixture = native_fixture();
    let result = run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "sleep 30", "timeout_ms": 200}),
    )
    .await;
    assert_eq!(
        result.status,
        ToolExecutionStatus::TimedOut,
        "an explicit foreground timeout maps to TimedOut"
    );
    assert_eq!(result.exit_code, None);
}

#[tokio::test]
async fn bash_foreground_cancellation_sends_term_to_the_process_group() {
    let fixture = native_fixture();
    let workspace = fixture.runtime.workspace().root().to_path_buf();
    let ready = workspace.join("trap-ready.marker");
    let marker = workspace.join("term-received.marker");
    // Deterministic readiness handshake: the shell installs the TERM trap
    // before it writes the ready marker, so observing the marker
    // deterministically means the trap is in place before cancellation.
    let command = format!(
        "trap 'touch {}' TERM; touch {}; sleep 30",
        marker.display(),
        ready.display()
    );
    let cancellation = CancellationSignal::new();
    let cancelling = cancellation.clone();
    let controller = tokio::spawn(async move {
        // Polling queries the readiness marker's existence (the state
        // itself) with a strict deadlock guard.
        for _ in 0..200 {
            if ready.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(ready.exists(), "the trap readiness marker never appeared");
        cancelling.cancel();
    });
    let result = run_tool_with_cancellation(
        &fixture,
        "bash",
        serde_json::json!({"command": command}),
        cancellation,
    )
    .await;
    controller.await.expect("controller");
    assert!(matches!(
        result.status,
        ToolExecutionStatus::Cancelled { .. }
    ));
    // TERM was delivered before the grace period expired and the KILL
    // landed, so the trap marker provably exists once the tool returns.
    assert!(marker.exists(), "TERM reached the owned process group");
}

#[tokio::test]
async fn bash_kill_escalates_when_term_is_ignored() {
    let fixture = native_fixture();
    let result = run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "trap '' TERM; sleep 30", "timeout_ms": 300}),
    )
    .await;
    assert_eq!(
        result.status,
        ToolExecutionStatus::TimedOut,
        "a TERM-ignoring child is killed via KILL and the execution still settles"
    );
}

#[tokio::test]
async fn bash_cancellation_terminates_descendants_of_the_process_group() {
    let fixture = native_fixture();
    let workspace = fixture.runtime.workspace().root().to_path_buf();
    let ready = workspace.join("descendant-ready.marker");
    let marker = workspace.join("descendant-stopped.marker");
    // The shell writes the ready marker, then waits for the descendant.
    // Cancellation is triggered only after the marker exists, so the
    // descendant is provably alive inside the owned group.
    let command = format!(
        "touch {}; sleep 30 & wait; touch {}",
        ready.display(),
        marker.display()
    );
    let cancellation = CancellationSignal::new();
    let cancelling = cancellation.clone();
    let controller = tokio::spawn(async move {
        for _ in 0..200 {
            if ready.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            ready.exists(),
            "the descendant readiness marker never appeared"
        );
        cancelling.cancel();
    });
    let result = run_tool_with_cancellation(
        &fixture,
        "bash",
        serde_json::json!({"command": command}),
        cancellation,
    )
    .await;
    controller.await.expect("controller");
    assert!(matches!(
        result.status,
        ToolExecutionStatus::Cancelled { .. }
    ));
    // The group was TERMed/KILLed and the shell reaped before the tool
    // returned, so the marker can never be written afterwards; the bounded
    // poll only guards against kernel-level delivery latency.
    for _ in 0..200 {
        if marker.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !marker.exists(),
        "the descendant of the cancelled group must be terminated"
    );
}

#[tokio::test]
async fn bash_cancellation_does_not_kill_unrelated_processes() {
    use std::process::Command;
    // An unrelated process in the test's own process group.
    let unrelated = Command::new("sleep").arg("30").spawn().expect("spawn");
    let unrelated_pid = unrelated.id();
    let fixture = native_fixture();
    let cancellation = CancellationSignal::new();
    // The ready marker proves the tool's own child is running inside the
    // workspace before cancellation fires; the unrelated process in the
    // test's own process group must never observe the group signal.
    let ready = fixture
        .runtime
        .workspace()
        .root()
        .join("unrelated-ready.marker");
    let ready_for_controller = ready.clone();
    let cancelling = cancellation.clone();
    let controller = tokio::spawn(async move {
        for _ in 0..200 {
            if ready_for_controller.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            ready_for_controller.exists(),
            "the tool child readiness marker never appeared"
        );
        cancelling.cancel();
    });
    let result = run_tool_with_cancellation(
        &fixture,
        "bash",
        serde_json::json!({"command": format!("touch {}; sleep 30", ready.display())}),
        cancellation,
    )
    .await;
    controller.await.expect("controller");
    assert!(matches!(
        result.status,
        ToolExecutionStatus::Cancelled { .. }
    ));
    // The tool's TERM/KILL path (grace included) completed before the tool
    // returned and targets only the owned process group; the unrelated
    // process in the test's own group must still be running.
    let mut unrelated = unrelated;
    assert!(
        unrelated.try_wait().expect("try_wait").is_none(),
        "the unrelated process (pid {unrelated_pid}) must survive group cancellation"
    );
    let _ = unrelated.kill();
    let _ = unrelated.wait();
}

#[tokio::test]
async fn bash_background_cancellation_uses_the_same_process_group_path() {
    use rustx::tools::background::BackgroundLifecycle;
    let fixture = native_fixture();
    let workspace = fixture.runtime.workspace().root().to_path_buf();
    let ready = workspace.join("bg-trap-ready.marker");
    let marker = workspace.join("bg-term-received.marker");
    let command = format!(
        "trap 'touch {}' TERM; touch {}; sleep 30",
        marker.display(),
        ready.display()
    );
    let registry = fixture.runtime.background().clone();
    let executor = fixture
        .registry
        .executor(&rustx::runtime::identity::ToolId::new("tool-bash"));
    let invocation = ToolInvocation {
        call_id: rustx::runtime::identity::ToolCallId::new("call-bg"),
        tool_id: rustx::runtime::identity::ToolId::new("tool-bash"),
        tool_name: "bash".to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({"command": command}),
    };
    let prepared = registry
        .prepare_dispatch(&invocation, &executor)
        .expect("prepare");
    let outcome = registry.commit_dispatch(prepared, &CancellationSignal::new());
    let rustx::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        outcome
    else {
        panic!("accepted");
    };
    // The registry state itself is the thing being polled (with a strict
    // deadlock guard): wait until the execution is running.
    let running = wait_for_lifecycle(&registry, &execution_id, BackgroundLifecycle::Running).await;
    assert_eq!(running.state, BackgroundLifecycle::Running);
    // Deterministic readiness: the TERM trap is installed before the ready
    // marker is written, and cancellation happens only afterwards.
    for _ in 0..200 {
        if ready.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        ready.exists(),
        "the background trap readiness marker never appeared"
    );
    let cancelling = registry.cancel(&execution_id).expect("cancel");
    assert_eq!(cancelling.state, BackgroundLifecycle::Cancelling);
    // The terminal settlement follows the cancellation path.
    let terminal =
        wait_for_lifecycle(&registry, &execution_id, BackgroundLifecycle::Cancelled).await;
    assert_eq!(terminal.state, BackgroundLifecycle::Cancelled);
    assert!(
        marker.exists(),
        "background cancellation TERMs the owned process group"
    );
}

#[tokio::test]
async fn bash_natural_exit_beats_late_cancel_in_the_registry() {
    use rustx::tools::background::BackgroundLifecycle;
    let fixture = native_fixture();
    let registry = fixture.runtime.background().clone();
    let executor = fixture
        .registry
        .executor(&rustx::runtime::identity::ToolId::new("tool-bash"));
    let invocation = ToolInvocation {
        call_id: rustx::runtime::identity::ToolCallId::new("call-bg"),
        tool_id: rustx::runtime::identity::ToolId::new("tool-bash"),
        tool_name: "bash".to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({"command": "echo done"}),
    };
    let prepared = registry
        .prepare_dispatch(&invocation, &executor)
        .expect("prepare");
    let outcome = registry.commit_dispatch(prepared, &CancellationSignal::new());
    let rustx::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        outcome
    else {
        panic!("accepted");
    };
    let terminal =
        wait_for_lifecycle(&registry, &execution_id, BackgroundLifecycle::Succeeded).await;
    assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
    let after_cancel = registry.cancel(&execution_id).expect("cancel");
    assert_eq!(
        after_cancel.state,
        BackgroundLifecycle::Succeeded,
        "completion committed first; the later cancel is an idempotent no-op"
    );
}

#[test]
fn bash_parent_secrets_are_absent_and_authorized_variables_are_visible() {
    let test_env = std::env::var("RUSTX_SENTINEL_TEST").is_ok();
    if test_env {
        // Inner run: execute the Bash tool and print its observable stdout.
        let fixture = native_fixture_with_environment(vec![(
            "RUSTX_AUTHORIZED".to_owned(),
            "visible".to_owned(),
        )]);
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let result = runtime.block_on(run_tool(
            &fixture,
            "bash",
            serde_json::json!({"command": "echo ${RUSTX_SENTINEL_SECRET:-absent}:${RUSTX_AUTHORIZED:-missing}"}),
        ));
        println!("OBSERVED={}", json_content(&result)["stdout"]);
        return;
    }
    // Outer run: re-execute this test with a sentinel secret injected into
    // the child process environment, then prove Bash cannot observe it.
    let exe = std::env::current_exe().expect("current exe");
    let output = std::process::Command::new(exe)
        .arg("--exact")
        .arg("bash_parent_secrets_are_absent_and_authorized_variables_are_visible")
        .arg("--nocapture")
        .env("RUSTX_SENTINEL_TEST", "1")
        .env("RUSTX_SENTINEL_SECRET", "shhh")
        .env("RUSTX_AUTHORIZED", "visible")
        .output()
        .expect("inner test run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "inner run failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("absent:visible"),
        "the sentinel secret must be absent and the authorized variable visible: {stdout}"
    );
    assert!(
        !stdout.contains("OBSERVED=shhh"),
        "the sentinel secret leaked into the child environment: {stdout}"
    );
}

#[tokio::test]
async fn bash_large_previews_are_bounded_with_full_artifacts_retained() {
    let fixture = native_fixture();
    let result = run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "yes x | head -c 200000"}),
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    let content = json_content(&result);
    assert!(
        content["stdout"].as_str().expect("stdout").len()
            <= rustx::tools::limits::BASH_STREAM_PREVIEW_BYTES,
        "the model-facing preview stays bounded"
    );
    let truncation = result.truncation.expect("truncation metadata");
    assert!(truncation.truncated);
    // The full raw output artifact retains every byte beyond the preview.
    let stdout_artifact = result
        .artifacts
        .iter()
        .find(|reference| reference.name.as_deref() == Some("stdout.log"))
        .expect("stdout artifact reference");
    let bytes = std::fs::read(artifact_path(
        &fixture,
        stdout_artifact.artifact_id.as_str(),
    ))
    .expect("artifact bytes");
    assert_eq!(bytes.len(), 200_000, "full output is retained verbatim");
    assert!(bytes.iter().all(|byte| *byte == b'x' || *byte == b'\n'));
}

#[tokio::test]
async fn bash_raw_non_utf8_output_is_preserved_in_the_artifact() {
    let fixture = native_fixture();
    let result = run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "printf '\\377\\376\\001\\002'"}),
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    // The preview is lossy-converted but the stored artifact is raw.
    let stdout_artifact = result
        .artifacts
        .iter()
        .find(|reference| reference.name.as_deref() == Some("stdout.log"))
        .expect("stdout artifact reference");
    let bytes = std::fs::read(artifact_path(
        &fixture,
        stdout_artifact.artifact_id.as_str(),
    ))
    .expect("artifact bytes");
    assert_eq!(bytes, vec![0xff, 0xfe, 0x01, 0x02], "raw bytes preserved");
    assert!(result.truncation.is_none(), "small output is not truncated");
}

/// A shell parent that exits while a descendant stays in the owned process
/// group and keeps the output pipe open cannot escape the invocation
/// timeout: the drain phase still owns the complete lifecycle and
/// terminates the group.
#[tokio::test]
async fn bash_shell_exit_with_descendant_holding_the_pipe_still_times_out() {
    let fixture = native_fixture();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        run_tool(
            &fixture,
            "bash",
            serde_json::json!({"command": "sleep 30 & exit 0", "timeout_ms": 300}),
        ),
    )
    .await
    .expect("the invocation settles exactly once");
    assert_eq!(
        result.status,
        ToolExecutionStatus::TimedOut,
        "the timeout owns the complete lifecycle: a descendant holding the pipe cannot escape"
    );
}

/// Cancellation after the shell parent exited remains effective during the
/// output-drain phase: the readiness marker proves the shell reached the
/// end of its command stream before cancellation, and the descendant
/// holding the pipe is terminated with the owned group.
#[tokio::test]
async fn bash_cancellation_after_shell_parent_exit_still_terminates_the_group() {
    let fixture = native_fixture();
    let workspace = fixture.runtime.workspace().root().to_path_buf();
    let ready = workspace.join("exited-ready.marker");
    // The shell writes the marker, then exits immediately; the descendant
    // `sleep 30` remains in the owned process group and holds the output
    // pipe, so the tool is in its output-drain phase when cancellation
    // fires.
    let command = format!("touch {}; sleep 30 & exit 0", ready.display());
    let cancellation = CancellationSignal::new();
    let cancelling = cancellation.clone();
    let controller = tokio::spawn(async move {
        for _ in 0..200 {
            if ready.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            ready.exists(),
            "the shell-exit readiness marker never appeared"
        );
        cancelling.cancel();
    });
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        run_tool_with_cancellation(
            &fixture,
            "bash",
            serde_json::json!({"command": command}),
            cancellation,
        ),
    )
    .await
    .expect("the invocation settles exactly once");
    controller.await.expect("controller");
    assert!(matches!(
        result.status,
        ToolExecutionStatus::Cancelled { .. }
    ));
}

/// Polls the registry snapshot state itself (with a strict deadlock
/// guard); the registry is the authoritative state machine, so the poll
/// queries the very state under test.
async fn wait_for_lifecycle(
    registry: &rustx::tools::background::ConversationBackgroundRegistry,
    execution_id: &rustx::runtime::identity::ToolExecutionId,
    state: rustx::tools::background::BackgroundLifecycle,
) -> rustx::tools::background::BackgroundExecutionSnapshot {
    for _ in 0..400 {
        let snapshot = registry.snapshot(execution_id).expect("snapshot");
        if snapshot.state == state {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let snapshot = registry.snapshot(execution_id).expect("snapshot");
    panic!("state {state:?} never reached; last snapshot: {snapshot:?}");
}
