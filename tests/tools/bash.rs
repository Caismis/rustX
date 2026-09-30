//! M5 native Bash tool tests.
//!
//! Bash is Unix-first: `/bin/bash` semantics are never pretended to be
//! portable, so every test is `#[cfg(unix)]`. Tests use controlled
//! temporary workspaces and deterministic subprocess fixtures; wall-clock
//! waits appear only as the configured TERM grace period and as deadlock
//! guards — never as the proof of a concurrency invariant. The two tests
//! that inspect `/proc` and prove Linux orphan-adoption behavior are
//! additionally Linux-only; macOS runs the shared process-group lifecycle
//! suite.

#![cfg(unix)]
#![allow(clippy::similar_names)] // scripted fixture names are intentionally similar

use std::time::Duration;

use crate::common::{
    native_fixture, native_fixture_with_environment, run_tool, run_tool_with_cancellation,
};
use rustx::runtime::CancellationSignal;
use rustx::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolResultContent,
};

fn fixture_with_trace(trace: &std::path::Path) -> crate::common::NativeFixture {
    fixture_with_control(
        &rustx::tools::native::bash_supervisor::diagnostics::FixtureControl {
            trace: Some(trace.to_path_buf()),
            before_term_socket: None,
        },
    )
}

fn fixture_with_control(
    control: &rustx::tools::native::bash_supervisor::diagnostics::FixtureControl,
) -> crate::common::NativeFixture {
    use rustx::tools::native::bash_supervisor::diagnostics::fixture_executor;
    let mut fixture = native_fixture();
    let mut registry = rustx::tools::executor::ToolRegistry::new();
    for definition in fixture.registry.definitions() {
        let executor = if definition.name == "bash" {
            fixture_executor(control.clone())
        } else {
            fixture.registry.executor(&definition.id)
        };
        registry.register(definition, executor).unwrap();
    }
    fixture.registry = registry;
    fixture
}

fn json_content(result: &ToolExecutionResult) -> serde_json::Value {
    for content in &result.content {
        if let ToolResultContent::Json { value } = content {
            return value.clone();
        }
    }
    panic!("expected a JSON result content block");
}

/// The number of result spill files in the conversation's managed
/// tool-output root (the `results/` directory; `tasks/` holds background
/// live output, a deliberately separate lifecycle).
fn spill_count(fixture: &crate::common::NativeFixture) -> usize {
    std::fs::read_dir(fixture.runtime.tool_output().root().join("results"))
        .expect("managed tool-output results root")
        .count()
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
        serde_json::json!({"command": "sleep 30", "timeout": 1}),
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
    use rustx::tools::native::bash_supervisor::diagnostics::{Entry, Event, FixtureControl};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Keep the socket below Darwin's path limit. All FIFOs use nonblocking
    // test-side handles: there are no detached accept/read tasks to leak.
    let dir = tempfile::Builder::new()
        .prefix("rx-fg-term-")
        .tempdir_in("/tmp")
        .unwrap();
    let trace = dir.path().join("trace");
    let socket = dir.path().join("gate");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let fixture = fixture_with_control(&FixtureControl {
        trace: Some(trace.clone()),
        before_term_socket: Some(socket),
    });
    let root = fixture.runtime.workspace().root();
    let ready = SignalPipe::new(&root.join("ready"));
    let entered = SignalPipe::new(&root.join("entered"));
    let release = SignalPipe::new(&root.join("release"));
    let marker = root.join("term-seen");
    // No external foreground child whose wait can defer a Bash trap, and no
    // external touch inside the handler. Entry and completion are distinct.
    let command = "trap 'printf T > entered; read release < release; printf done > term-seen; exit 0' TERM; printf R > ready; while :; do :; done";
    let cancellation = CancellationSignal::new();
    let mut stage = "waiting for installed trap";
    let mut ready_seen = false;
    let mut entered_seen = false;
    let mut before = String::new();
    let controller = async {
        let outcome = tokio::time::timeout(Duration::from_secs(15), async {
            ready.read().await;
            ready_seen = true;
            stage = "waiting for supervisor TERMINATE receipt";
            cancellation.cancel();
            let (mut gate, _) = listener.accept().await?;
            gate.read_exact(&mut [0]).await?;
            stage = "supervisor before TERM";
            before = std::fs::read_to_string(&trace)?;
            // The live shell cannot naturally exit this builtin loop. The
            // supervisor is parked before sending; no handler witness exists.
            let controlled = !marker.exists();
            gate.write_all(b"go").await?;
            stage = "waiting for handler entry";
            entered.read().await;
            entered_seen = true;
            // Release immediately, before assertions or diagnostic processing:
            // the production finite grace deadline remains authoritative.
            release.release();
            stage = "handler released; waiting for physical settlement";
            Ok::<_, std::io::Error>(controlled)
        })
        .await;
        // Also unblock cleanup on a guard failure. Dropping the listener and
        // accepted socket releases the supervisor gate; cancellation stays
        // owned by the real operation, which we continue driving to completion.
        cancellation.cancel();
        release.release();
        drop(listener);
        outcome
    };
    let execution = async {
        let result = run_tool_with_cancellation(
            &fixture,
            "bash",
            serde_json::json!({"command": command}),
            cancellation.clone(),
        )
        .await;
        // Capture at the foreground return boundary, not after joining the
        // controller: a prematurely returned result must not acquire later
        // handler or settlement evidence while the controller finishes.
        let evidence = std::fs::read_to_string(&trace).unwrap_or_default();
        let completed = std::fs::read_to_string(&marker).ok();
        (result, evidence, completed)
    };
    let ((result, evidence, completed), controlled) = tokio::join!(execution, controller);
    // Trace is bounded by its existing owner. This command emits no business
    // output; result Debug includes any captured stderr without a new channel.
    let diagnostic = format!(
        "foreground call-m5: stage={stage}, ready={ready_seen}, entered={entered_seen}, completed={completed:?}, controller={controlled:?}\nresult={result:?}\nbefore TERM:\n{before}\nsettled trace:\n{evidence}"
    );
    assert!(matches!(controlled, Ok(Ok(true))), "{diagnostic}");
    assert!(
        matches!(result.status, ToolExecutionStatus::Cancelled { .. }),
        "{diagnostic}"
    );
    assert_eq!(completed.as_deref(), Some("done"), "{diagnostic}");
    let before: Vec<Entry> = before
        .lines()
        .map(|line| serde_json::from_str(line).expect(&diagnostic))
        .collect();
    assert!(
        before
            .iter()
            .any(|e| matches!(e.event, Event::TerminateReceived))
            && !before
                .iter()
                .any(|e| matches!(e.event, Event::Signal { .. })),
        "{diagnostic}"
    );
    let entries: Vec<Entry> = evidence
        .lines()
        .map(|line| serde_json::from_str(line).expect(&diagnostic))
        .collect();
    let index = |predicate: fn(&Event) -> bool| {
        entries
            .iter()
            .position(|e| predicate(&e.event))
            .expect(&diagnostic)
    };
    let term = index(|e| {
        matches!(
            e,
            Event::Signal {
                signal: libc::SIGTERM,
                result: Some(0),
                ..
            }
        )
    });
    let Event::Signal { pgid, .. } = entries[term].event else {
        unreachable!()
    };
    assert!(
        pgid > 0 && entries[term].process == u32::try_from(pgid).unwrap(),
        "{diagnostic}"
    );
    assert!(
        before
            .iter()
            .any(|e| matches!(e.event, Event::ShellGroup { pgid: group, .. } if group == pgid)),
        "{diagnostic}"
    );
    assert!(
        entries
            .iter()
            .any(|e| matches!(e.event, Event::TerminateSent { sent: true })),
        "{diagnostic}"
    );
    let shell = index(|e| matches!(e, Event::ShellExited { status: 0 }));
    let anchor = index(|e| matches!(e, Event::AnchorTerminalObserved));
    let children = index(|e| matches!(e, Event::GroupChildrenReaped));
    let published = index(|e| matches!(e, Event::TerminalPublished));
    let observed = index(|e| matches!(e, Event::TerminalObserved));
    let reaped = index(|e| matches!(e, Event::DirectChildReaped));
    // These are same-owner program/protocol dependencies, not an invented
    // cross-process clock. In particular TERM's post-syscall trace may race
    // with the shell entering its handler; no trace/entry order is asserted.
    assert!(
        term < shell && anchor < children && children < published && observed < reaped,
        "{diagnostic}"
    );
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e.event, Event::GraceExpired | Event::ControlFailure)),
        "{diagnostic}"
    );
    for (i, entry) in entries.iter().enumerate() {
        if let Event::Signal {
            pgid: target,
            signal,
            ..
        } = entry.event
        {
            assert_eq!(target, pgid, "{diagnostic}");
            // Darwin's legitimate post-shell containment is allowed.
            if signal == libc::SIGKILL {
                assert!(i > shell, "{diagnostic}");
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let absence = index(|e| matches!(e, Event::GroupAbsenceProven));
        assert!(children < absence && absence < published, "{diagnostic}");
    }
}

#[tokio::test]
async fn bash_kill_escalates_when_term_is_ignored() {
    use rustx::tools::native::bash_supervisor::diagnostics::{Entry, Event};
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("signals");
    let fixture = fixture_with_trace(&trace);
    let result = run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "trap '' TERM; sleep 30", "timeout": 1}),
    )
    .await;
    assert_eq!(
        result.status,
        ToolExecutionStatus::TimedOut,
        "a TERM-ignoring child is killed via KILL and the execution still settles"
    );
    let evidence = std::fs::read_to_string(trace).unwrap();
    let entries: Vec<Entry> = evidence
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let term = entries
        .iter()
        .position(|e| {
            matches!(
                e.event,
                Event::Signal {
                    signal: libc::SIGTERM,
                    result: Some(0),
                    ..
                }
            )
        })
        .expect(&evidence);
    let grace = entries
        .iter()
        .position(|e| matches!(e.event, Event::GraceExpired))
        .expect(&evidence);
    let kill = entries
        .iter()
        .position(|e| {
            matches!(
                e.event,
                Event::Signal {
                    signal: libc::SIGKILL,
                    ..
                }
            )
        })
        .expect(&evidence);
    assert!(term < grace && grace < kill, "{evidence}");
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
    use rustx::tools::native::bash_supervisor::diagnostics::{Entry, Event};
    let trace_dir = tempfile::tempdir().unwrap();
    let trace = trace_dir.path().join("supervision.jsonl");
    let fixture = fixture_with_trace(&trace);
    let workspace = fixture.runtime.workspace().root().to_path_buf();
    let ready_path = workspace.join("bg-ready");
    let ready = SignalPipe::new(&ready_path);
    // Readiness is OS-backed; no user-shell trap must beat the TERM grace.
    let command = format!("printf R > {}; while :; do :; done", ready_path.display());
    let registry = fixture.runtime.background().clone();
    let executor = fixture
        .registry
        .executor(&rustx::runtime::identity::ToolId::new("tool-bash"));
    let invocation = ToolInvocation {
        id: rustx::tools::types::ToolInvocationId::Agent {
            call_id: rustx::runtime::identity::ToolCallId::new("call-bg"),
        },
        tool_id: rustx::runtime::identity::ToolId::new("tool-bash"),
        tool_name: "bash".to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({"command": command}),
    };
    let prepared = registry
        .prepare_dispatch(
            &invocation,
            &executor,
            fixture.runtime.environment().clone(),
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits");
    let rustx::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        outcome
    else {
        panic!("accepted");
    };
    // The registry state itself is the thing being polled (with a strict
    // deadlock guard): wait until the execution is running.
    let running = wait_for_lifecycle(&registry, &execution_id, BackgroundLifecycle::Running).await;
    assert_eq!(running.state, BackgroundLifecycle::Running);
    tokio::time::timeout(Duration::from_secs(15), ready.read())
        .await
        .expect("background shell readiness");
    let cancelling = registry.cancel(&execution_id).expect("cancel");
    assert_eq!(cancelling.state, BackgroundLifecycle::Cancelling);
    // The terminal settlement follows the cancellation path.
    let terminal =
        wait_for_lifecycle(&registry, &execution_id, BackgroundLifecycle::Cancelled).await;
    assert_eq!(terminal.state, BackgroundLifecycle::Cancelled);
    let evidence = std::fs::read_to_string(&trace).unwrap();
    let entries: Vec<Entry> = evidence
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let index = |predicate: fn(&Event) -> bool| {
        entries
            .iter()
            .position(|entry| predicate(&entry.event))
            .unwrap_or_else(|| panic!("missing native event: {evidence}"))
    };
    let term = index(|event| {
        matches!(
            event,
            Event::Signal {
                signal: libc::SIGTERM,
                result: Some(0),
                ..
            }
        )
    });
    let Event::Signal { pgid, .. } = entries[term].event else {
        unreachable!()
    };
    assert!(
        entries[..term].iter().any(
            |entry| matches!(entry.event, Event::ShellGroup { pgid: group, .. } if group == pgid)
        ),
        "{evidence}"
    );
    let settled = index(|event| matches!(event, Event::GroupChildrenReaped));
    let observed = index(|event| matches!(event, Event::TerminalObserved));
    let published = index(|event| matches!(event, Event::TerminalPublished));
    let reaped = index(|event| matches!(event, Event::DirectChildReaped));
    // The sender logs publication after writing the frame; receiver logging
    // may precede it. Only causal owner order is a runtime invariant.
    assert!(
        term < settled
            && settled < published
            && published < reaped
            && settled < observed
            && observed < reaped,
        "{evidence}"
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
        id: rustx::tools::types::ToolInvocationId::Agent {
            call_id: rustx::runtime::identity::ToolCallId::new("call-bg"),
        },
        tool_id: rustx::runtime::identity::ToolId::new("tool-bash"),
        tool_name: "bash".to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({"command": "echo done"}),
    };
    let prepared = registry
        .prepare_dispatch(
            &invocation,
            &executor,
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare");
    let outcome = registry
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch commits");
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
        .arg("bash::bash_parent_secrets_are_absent_and_authorized_variables_are_visible")
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

/// Small output stays fully in memory: ordinary bounded text, no spill
/// file, no artifact, no truncation.
#[tokio::test]
async fn bash_small_output_creates_no_spill() {
    let fixture = native_fixture();
    let result = run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "echo hello"}),
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert!(
        result.artifacts.is_empty(),
        "textual output never becomes a semantic artifact"
    );
    assert!(result.truncation.is_none());
    assert!(
        result.managed_output.is_none(),
        "no spill continuation metadata exists"
    );
    assert!(
        json_content(&result).get("full_output").is_none(),
        "the tool-owned JSON carries no continuation keys"
    );
    assert_eq!(spill_count(&fixture), 0, "no spill file exists");
}

/// Oversized textual output stays textual: the model-facing previews stay
/// bounded, the complete combined output spills into exactly one managed
/// tool-output file addressed by its absolute path inside the ordinary
/// textual result, and the file is readable and searchable through the
/// ordinary native Read/Grep tools.
#[allow(clippy::too_many_lines)] // one scenario: spill, then prove read/search access
#[tokio::test]
async fn bash_large_output_spills_to_managed_output() {
    let fixture = native_fixture();
    let result = run_tool(
        &fixture,
        "bash",
        serde_json::json!({
            "command": "for i in $(seq 1 1500); do echo line-$i; done; echo spill-boundary-marker; for i in $(seq 1501 3000); do echo line-$i; done"
        }),
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    let content = json_content(&result);
    assert!(
        content["stdout"].as_str().expect("stdout").len()
            <= rustx::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES,
        "the model-facing preview stays bounded"
    );
    assert!(
        result.artifacts.is_empty(),
        "oversized text never becomes a semantic artifact"
    );
    let truncation = result.truncation.as_ref().expect("truncation metadata");
    assert!(truncation.truncated);
    assert!(truncation.original_bytes.is_some());
    // The spill locator is typed runtime-owned metadata: absolute and
    // under the managed root. The tool-owned JSON carries no magic keys.
    assert!(
        content.get("full_output").is_none() && content.get("note").is_none(),
        "the tool-owned JSON carries no continuation keys: {content}"
    );
    let Some(rustx::tools::ManagedOutputContinuation::Complete { locator }) =
        &result.managed_output
    else {
        panic!(
            "a complete spill is typed Complete, got {:?}",
            result.managed_output
        );
    };
    let full_output = locator.to_str().expect("utf8 spill locator").to_owned();
    let full_output = full_output.as_str();
    assert!(std::path::Path::new(full_output).is_absolute());
    assert!(
        std::path::Path::new(full_output).starts_with(fixture.runtime.tool_output().root()),
        "the spill lives in the managed tool-output root: {full_output}"
    );
    // The canonical result projection presents the locator plus the
    // Read/Grep continuation guidance to the model.
    let continuation_text = result.model_facing_projection().as_text();
    assert!(
        continuation_text.contains(&format!("Complete output: {full_output}")),
        "the model-facing text carries the exact locator: {continuation_text}"
    );
    assert!(
        continuation_text.contains("Read or Grep"),
        "the bounded text states how to reach the complete output"
    );
    assert_eq!(spill_count(&fixture), 1, "exactly one combined spill file");

    // The spill retains the complete output from byte zero; the middle
    // marker line is absent from the bounded head/tail preview but present
    // in the file.
    let spilled = std::fs::read_to_string(full_output).expect("spill text");
    assert!(spilled.starts_with("line-1\n"));
    assert!(spilled.ends_with("line-3000\n"));
    assert!(
        !content["combined"]
            .as_str()
            .expect("combined")
            .contains("spill-boundary-marker"),
        "the middle marker line is beyond the bounded head/tail preview"
    );

    // Ordinary native Read with offset/limit reads the spill file.
    let marker_line = 1501_u64;
    let read = run_tool(
        &fixture,
        "read",
        serde_json::json!({
            "path": full_output,
            "offset": marker_line,
            "limit": 1,
        }),
    )
    .await;
    assert_eq!(read.status, ToolExecutionStatus::Success);
    let read_text = read
        .content
        .iter()
        .find_map(|block| match block {
            ToolResultContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("text content");
    assert_eq!(
        read_text,
        "spill-boundary-marker\n\n[1501 more lines in file. Use offset=1502 to continue.]"
    );

    // Ordinary native Grep searches the single spill file and the managed
    // root directory.
    let grep_file = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "spill-boundary-marker", "path": full_output}),
    )
    .await;
    assert_eq!(grep_file.status, ToolExecutionStatus::Success);
    let grep_file_text = grep_file
        .content
        .iter()
        .find_map(|block| match block {
            ToolResultContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("Grep text");
    assert!(grep_file_text.contains(": spill-boundary-marker"));
    let grep_root = run_tool(
        &fixture,
        "grep",
        serde_json::json!({
            "pattern": "spill-boundary-marker",
            "path": fixture.runtime.tool_output().root().to_str().expect("utf8"),
        }),
    )
    .await;
    assert_eq!(grep_root.status, ToolExecutionStatus::Success);
    let grep_root_text = grep_root
        .content
        .iter()
        .find_map(|block| match block {
            ToolResultContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("Grep text");
    assert!(grep_root_text.contains(": spill-boundary-marker"));
}

/// Every advertised Read/Grep path holds valid UTF-8 text (Issue #86):
/// raw non-UTF-8 output decodes deterministically to U+FFFD replacement
/// characters — never raw bytes — so the spilled file is always readable
/// by Read and searchable by Grep. The decoded text is identical to the
/// one-shot lossy decoding of the same bytes.
#[tokio::test]
async fn bash_non_utf8_output_spills_as_deterministic_text() {
    let fixture = native_fixture();
    // Deterministic invalid bytes (never /dev/urandom): 20000 'x' bytes
    // cross the bound, then an invalid tail.
    let result = run_tool(
        &fixture,
        "bash",
        serde_json::json!({"command": "printf 'x%.0s' {1..20000}; printf '\\377\\376\\001\\002end\\n'"}),
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    let Some(rustx::tools::ManagedOutputContinuation::Complete { locator }) =
        &result.managed_output
    else {
        panic!(
            "the spill is typed Complete, got {:?}",
            result.managed_output
        );
    };
    let full_output = locator.to_str().expect("utf8 spill locator");
    let text = std::fs::read_to_string(full_output)
        .expect("the advertised spill path is always valid UTF-8 text");
    assert!(text.starts_with(&"x".repeat(100)));
    // \377 \376 are invalid UTF-8 and decode to exactly two U+FFFD
    // replacement characters; \001 \002 are valid control bytes and pass
    // through unchanged.
    assert!(
        text.ends_with("\u{FFFD}\u{FFFD}\u{1}\u{2}end\n"),
        "invalid bytes decode deterministically to U+FFFD: {:?}",
        &text[text.len() - 16..]
    );

    // The advertised path is genuinely inspectable: Read and Grep succeed.
    let read = run_tool(&fixture, "read", serde_json::json!({"path": full_output})).await;
    assert_eq!(
        read.status,
        ToolExecutionStatus::Success,
        "Read inspects the decoded-text spill"
    );
    let grep = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "end", "path": full_output}),
    )
    .await;
    assert_eq!(
        grep.status,
        ToolExecutionStatus::Success,
        "Grep searches the decoded-text spill"
    );
    let grep_text = grep
        .content
        .iter()
        .find_map(|block| match block {
            ToolResultContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("Grep text");
    assert!(grep_text.contains("... [truncated]"));
}

/// A shell parent that exits while a descendant stays in the owned process
/// group and keeps the output pipe open cannot escape the invocation
/// timeout: the runtime keeps supervising the owned group and terminates
/// it.
#[tokio::test]
async fn bash_shell_exit_with_descendant_holding_the_pipe_still_times_out() {
    let fixture = native_fixture();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        run_tool(
            &fixture,
            "bash",
            serde_json::json!({"command": "sleep 30 & exit 0", "timeout": 1}),
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

/// The missing Scenario-D regression: the descendant redirects its
/// stdout/stderr away from the rustX pipes, so the output capture can
/// finish while the owned process group is still alive. Shell-parent exit
/// must still not settle the invocation: the tool returns `TimedOut`, the
/// owned process group is quiescent, and the recorded descendant PID is
/// provably gone. This proof uses Linux `/proc` diagnostics and the Linux
/// child-subreaper path.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn bash_redirected_descendant_does_not_settle_and_is_terminated() {
    let fixture = native_fixture();
    let workspace = fixture.runtime.workspace().root().to_path_buf();
    let shell_pid_file = workspace.join("shell.pid");
    let descendant_pid_file = workspace.join("descendant.pid");
    let pgid_file = workspace.join("pgid.txt");
    // The fixture writes the shell's own pid AND its process-group id
    // (diagnostic observability inside the fixture; /proc is never the
    // production ownership authority). The pgid is the invocation's
    // supervisor-leader pid, which the shell can report itself.
    let command = format!(
        "echo $$ > {}; echo $(awk '{{print $5}}' /proc/$$/stat) > {}; \
         sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
        shell_pid_file.display(),
        pgid_file.display(),
        descendant_pid_file.display()
    );
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_tool(
            &fixture,
            "bash",
            serde_json::json!({"command": command, "timeout": 1}),
        ),
    )
    .await
    .expect("the invocation settles exactly once");
    assert_eq!(
        result.status,
        ToolExecutionStatus::TimedOut,
        "a redirected descendant must never let the invocation settle as Success"
    );
    let descendant_pid: i32 = std::fs::read_to_string(&descendant_pid_file)
        .expect("descendant pid file")
        .trim()
        .parse()
        .expect("descendant pid");
    let pgid: i32 = std::fs::read_to_string(&pgid_file)
        .expect("pgid file")
        .trim()
        .parse()
        .expect("pgid");
    // The owned process group is quiescent and the descendant is gone.
    wait_for_group_death(pgid).await;
    wait_for_process_death(descendant_pid).await;
}

/// A short-lived redirected descendant that exits naturally lets the owned
/// group quiesce before the invocation deadline: the shell's natural exit
/// then settles the command as ordinary `Success`.
#[tokio::test]
async fn bash_natural_descendant_completion_settles_success() {
    let fixture = native_fixture();
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_tool(
            &fixture,
            "bash",
            serde_json::json!({
                "command": "sleep 0.2 >/dev/null 2>&1 & exit 0",
                "timeout": 10
            }),
        ),
    )
    .await
    .expect("the invocation settles exactly once");
    assert_eq!(
        result.status,
        ToolExecutionStatus::Success,
        "once the owned group quiesces, the shell's natural exit settles Success"
    );
    assert_eq!(json_content(&result)["exit_code"], 0);
}

/// The mandatory descendant-replacement race regression: A (a subshell)
/// creates B (a redirected descendant), A exits, B remains owned. The
/// invocation must remain active while the supervisor still owns B —
/// settlement is gated on the supervisor's kernel child-wait terminal
/// state, never on an observational process scan — and only the invocation
/// timeout settles it. This proof uses Linux `/proc` diagnostics and the
/// Linux child-subreaper path.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn bash_descendant_replacement_keeps_the_invocation_active() {
    let fixture = native_fixture();
    let workspace = fixture.runtime.workspace().root().to_path_buf();
    let a_pid_file = workspace.join("a.pid");
    let b_pid_file = workspace.join("b.pid");
    let pgid_file = workspace.join("pgid.txt");
    let command = format!(
        "echo $$ > {}; echo $(awk '{{print $5}}' /proc/$$/stat) > {}; \
         (sleep 30 >/dev/null 2>&1 & echo $! > {}) & echo $! > {}; wait; exit 0",
        workspace.join("shell.pid").display(),
        pgid_file.display(),
        b_pid_file.display(),
        a_pid_file.display()
    );
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        run_tool(
            &fixture,
            "bash",
            serde_json::json!({"command": command, "timeout": 1}),
        ),
    )
    .await
    .expect("the invocation settles exactly once");
    assert_eq!(
        result.status,
        ToolExecutionStatus::TimedOut,
        "the invocation must stay active while the supervisor owns B"
    );
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
    let pgid: i32 = std::fs::read_to_string(&pgid_file)
        .expect("pgid file")
        .trim()
        .parse()
        .expect("pgid");
    // A is gone; the whole owned domain (B included) is terminal after the
    // timeout-driven termination.
    wait_for_process_death(a_pid).await;
    wait_for_group_death(pgid).await;
    wait_for_process_death(b_pid).await;
}

/// Polls the owned process group with the same non-destructive `killpg`
/// probe the production logic uses (the authoritative OS state), with a
/// strict deadlock guard.
#[cfg(target_os = "linux")]
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

/// Polls a specific process until it is provably gone (the signal-0 probe
/// returns `ESRCH`), with a strict deadlock guard.
#[cfg(target_os = "linux")]
async fn wait_for_process_death(pid: i32) {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    for _ in 0..1000 {
        match kill(Pid::from_raw(pid), None) {
            Ok(()) | Err(Errno::EPERM) => {}
            Err(_) => return,
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("process {pid} is still alive after the deadline");
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

/// The macOS Bash EXIT `wait` is a best-effort convenience, not an
/// ownership or terminality primitive. Replacing it (`trap ':' EXIT`) must
/// not fabricate terminality: the shell exits immediately, the background
/// job outlives it, and the macOS fallback containment (anchored `SIGKILL`
/// + `killpg(pgid, 0)` absence probe) must actually terminate that job
/// before the tool settles.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn bash_replaced_exit_trap_does_not_fabricate_terminality() {
    let fixture = native_fixture();
    let workspace = fixture.runtime.workspace().root().to_path_buf();
    let pid_file = workspace.join("bg.pid");
    let command = format!("trap ':' EXIT; sleep 30 & echo $! > {}", pid_file.display());
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        run_tool(&fixture, "bash", serde_json::json!({"command": command})),
    )
    .await
    .expect("the invocation settles exactly once");
    assert_eq!(result.status, ToolExecutionStatus::Success);
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .expect("background pid file")
        .trim()
        .parse()
        .expect("background pid");
    assert!(
        wait_for_process_absence_macos(pid),
        "the replaced EXIT trap must not let a live background job be falsely settled"
    );
}

/// The macOS Bash EXIT `wait` may also be cleared entirely (`trap - EXIT`).
/// Clearing it must not fabricate terminality either: the background job is
/// still terminated by the fallback containment before the tool settles.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn bash_cleared_exit_trap_does_not_fabricate_terminality() {
    let fixture = native_fixture();
    let workspace = fixture.runtime.workspace().root().to_path_buf();
    let pid_file = workspace.join("bg.pid");
    let command = format!("trap - EXIT; sleep 30 & echo $! > {}", pid_file.display());
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        run_tool(&fixture, "bash", serde_json::json!({"command": command})),
    )
    .await
    .expect("the invocation settles exactly once");
    assert_eq!(result.status, ToolExecutionStatus::Success);
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .expect("background pid file")
        .trim()
        .parse()
        .expect("background pid");
    assert!(
        wait_for_process_absence_macos(pid),
        "the cleared EXIT trap must not let a live background job be falsely settled"
    );
}

/// Polls a specific process with the signal-0 probe until it is provably
/// gone (`ESRCH`), with a strict deadlock guard. Test-only; `/proc`-free so
/// it works on macOS, where `kill(pid, 0)` is the same existence probe.
#[cfg(target_os = "macos")]
fn wait_for_process_absence_macos(pid: i32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    for _ in 0..1000 {
        if let Err(nix::errno::Errno::ESRCH) = kill(Pid::from_raw(pid), None) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// The `ModelSelectable` Bash definition the model receives: the canonical
/// schema stays free of runtime metadata and the compiled model-facing schema
/// carries the required `execution_mode` selector next to Bash's own
/// arguments. Bash implements no `execution_mode` handling of its own.
#[test]
fn model_selectable_bash_compiles_the_execution_mode_selector() {
    let fixture = crate::common::model_selectable_native_fixture();
    let bash = selectable_bash(&fixture);
    assert_eq!(
        bash.execution_policy,
        rustx::tools::types::ToolExecutionPolicy::ModelSelectable
    );
    assert!(
        !bash.input_schema.to_string().contains("execution_mode"),
        "the canonical Bash schema stays free of runtime metadata: {}",
        bash.input_schema
    );
    let compiled = fixture
        .registry
        .model_definitions()
        .into_iter()
        .find(|definition| definition.name == "bash")
        .expect("compiled bash definition");
    crate::common::assert_compiled_execution_mode_schema(&compiled.input_schema, "command");
}

/// The canonical `ModelSelectable` Bash definition of a fixture.
fn selectable_bash(fixture: &crate::common::NativeFixture) -> rustx::tools::types::ToolDefinition {
    fixture
        .registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "bash")
        .expect("bash is registered")
}

/// One `ModelSelectable` Bash call carrying the model's explicit choice.
fn selectable_bash_call(
    bash: &rustx::tools::types::ToolDefinition,
    mode: &str,
) -> rustx::tools::types::ToolCall {
    rustx::tools::types::ToolCall {
        id: rustx::runtime::identity::ToolCallId::new("call-bash-selectable"),
        tool_id: bash.id.clone(),
        name: "bash".to_owned(),
        arguments: serde_json::json!({"execution_mode": mode, "command": "echo selectable"}),
    }
}

/// The business arguments every `ModelSelectable` Bash call in this suite
/// leaves behind once the runtime strips its `execution_mode` selector.
fn selectable_bash_business_arguments() -> serde_json::Value {
    serde_json::json!({"command": "echo selectable"})
}

/// An explicit `"foreground"` choice resolves to attempt-owned execution and
/// reaches the native executor with the selector already stripped.
#[tokio::test]
async fn model_selectable_bash_foreground_selection_reaches_the_executor_stripped() {
    use rustx::tools::executor::{PreflightOutcome, ToolExecutionContext};

    let fixture = crate::common::model_selectable_native_fixture();
    let bash = selectable_bash(&fixture);
    let business = selectable_bash_business_arguments();
    let PreflightOutcome::Ready(prepared) = fixture
        .registry
        .preflight(&selectable_bash_call(&bash, "foreground"))
        .expect("preflight")
    else {
        panic!("an explicit foreground selection preflights as ready");
    };
    assert_eq!(prepared.invocation.mode, ToolInvocationMode::Foreground);
    assert_eq!(
        prepared.invocation.arguments, business,
        "the executor receives only business arguments"
    );
    let executor = fixture.registry.executor(&prepared.invocation.tool_id);
    let reporter = crate::common::NoopProgress;
    let context = ToolExecutionContext::new(
        fixture.runtime.conversation_id(),
        None,
        rustx::runtime::ExecutionCancellation::detached(
            CancellationSignal::new(),
            rustx::runtime::types::CancellationReason::UserRequested,
        ),
        fixture.runtime.workspace(),
        &reporter,
        fixture.runtime.artifacts(),
        fixture.runtime.tool_output(),
        fixture.runtime.environment(),
    );
    let result = executor
        .start(prepared.invocation, context)
        .completion
        .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert_eq!(json_content(&result)["exit_code"], 0);
    assert!(
        json_content(&result)["stdout"]
            .as_str()
            .expect("stdout")
            .contains("selectable")
    );
}

/// An explicit `"background"` choice resolves to conversation-owned execution
/// and dispatches through the background registry with the same stripped
/// business arguments.
#[tokio::test]
async fn model_selectable_bash_background_selection_dispatches_conversation_owned() {
    use rustx::tools::executor::PreflightOutcome;

    let fixture = crate::common::model_selectable_native_fixture();
    let bash = selectable_bash(&fixture);
    let business = selectable_bash_business_arguments();
    let PreflightOutcome::Ready(prepared) = fixture
        .registry
        .preflight(&selectable_bash_call(&bash, "background"))
        .expect("preflight")
    else {
        panic!("an explicit background selection preflights as ready");
    };
    assert_eq!(prepared.invocation.mode, ToolInvocationMode::Background);
    assert_eq!(
        prepared.invocation.arguments, business,
        "the executor receives only business arguments"
    );
    let background = fixture.runtime.background().clone();
    let executor = fixture.registry.executor(&prepared.invocation.tool_id);
    let dispatch = background
        .prepare_dispatch(
            &prepared.invocation,
            &executor,
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("prepare background dispatch");
    let rustx::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        background
            .commit_dispatch(dispatch, &CancellationSignal::new())
            .expect("background dispatch commits")
    else {
        panic!("the background dispatch is accepted");
    };
    // The registry's own state-change notification settles this, not a
    // polling loop: `wait_until_terminal` subscribes to the exact watch
    // channel every published transition bumps.
    let terminal = background
        .wait_until_terminal(&execution_id)
        .await
        .expect("the dispatched execution settles");
    assert_eq!(
        terminal.state,
        rustx::tools::background::BackgroundLifecycle::Succeeded
    );
}

/// A `ModelSelectable` Bash call that omits or misspells `execution_mode` is
/// rejected at preflight with diagnostics the model can act on directly, and
/// the retired `__rustx_execution` selector no longer works.
#[tokio::test]
async fn model_selectable_bash_rejects_unusable_execution_mode() {
    use rustx::runtime::identity::ToolCallId;
    use rustx::tools::executor::PreflightOutcome;
    use rustx::tools::types::ToolCall;

    let fixture = crate::common::model_selectable_native_fixture();
    let bash = selectable_bash(&fixture);
    let reject = |arguments: serde_json::Value| {
        let outcome = fixture
            .registry
            .preflight(&ToolCall {
                id: ToolCallId::new("call-bash-selectable"),
                tool_id: bash.id.clone(),
                name: "bash".to_owned(),
                arguments,
            })
            .expect("preflight outcome");
        let PreflightOutcome::Rejected { error, .. } = outcome else {
            panic!("expected a deterministic rejection");
        };
        error
    };

    let missing = reject(serde_json::json!({"command": "echo hi"}));
    assert!(
        missing.contains("\"execution_mode\": \"foreground\"")
            && missing.contains("\"execution_mode\": \"background\""),
        "the model is told exactly how to retry: {missing}"
    );
    let invalid = reject(serde_json::json!({"execution_mode": "detached", "command": "echo hi"}));
    assert!(
        invalid.contains("execution_mode") && invalid.contains("detached"),
        "the invalid value is named: {invalid}"
    );
    let retired =
        reject(serde_json::json!({"__rustx_execution": "background", "command": "echo hi"}));
    assert!(
        retired.contains("__rustx_"),
        "the retired selector is a forged reserved argument, not a mode: {retired}"
    );
}

/// A nonblocking FIFO fixture: dropping a failed test never leaves a blocking
/// pool thread stuck opening a pipe. Readiness is kernel readiness, not a timer.
struct SignalPipe(tokio::io::unix::AsyncFd<std::fs::File>);
impl SignalPipe {
    fn new(path: &std::path::Path) -> Self {
        use std::os::unix::fs::OpenOptionsExt;
        nix::unistd::mkfifo(
            path,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .unwrap();
        Self(tokio::io::unix::AsyncFd::new(file).unwrap())
    }
    async fn read(&self) {
        use std::io::Read;
        loop {
            let mut ready = self.0.readable().await.unwrap();
            if let Ok(result) = ready.try_io(|file| file.get_ref().read_exact(&mut [0])) {
                result.unwrap();
                return;
            }
        }
    }
    fn release(&self) {
        use std::io::Write;
        self.0.get_ref().write_all(b"release\n").unwrap();
    }
}

#[tokio::test]
async fn background_cancel_records_term_before_trap_and_physical_terminal() {
    tokio::time::timeout(Duration::from_secs(15), background_cancel_at_native_gate())
        .await
        .expect("native signal/terminal gates must settle");
}

async fn background_cancel_at_native_gate() {
    use rustx::tools::background::BackgroundLifecycle;
    use rustx::tools::native::bash_supervisor::diagnostics::{
        Entry, Event, FixtureControl, fixture_executor,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // Bounded native socket namespace, independent of macOS TMPDIR length.
    let dir = tempfile::Builder::new()
        .prefix("rx-term-")
        .tempdir_in("/tmp")
        .unwrap();
    let trace = dir.path().join("trace");
    let socket = dir.path().join("gate");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let fixture = native_fixture_with_environment(vec![
        (
            "RUSTX_TEST_SUPERVISION_TRACE".into(),
            dir.path().join("untrusted-trace").display().to_string(),
        ),
        (
            "RUSTX_TEST_BEFORE_TERM_SOCKET".into(),
            "user-visible-value".into(),
        ),
    ]);
    let root = fixture.runtime.workspace().root();
    let ready_path = root.join("ready");
    let entered_path = root.join("entered");
    let release_path = root.join("release");
    let ready = SignalPipe::new(&ready_path);
    let entered = SignalPipe::new(&entered_path);
    let release = SignalPipe::new(&release_path);
    let marker = root.join("term-marker");
    let command = format!(
        "trap 'printf T > {}; read release < {}; touch {}; exit 0' TERM; printf R > {}; while :; do :; done",
        entered_path.display(),
        release_path.display(),
        marker.display(),
        ready_path.display()
    );
    let registry = fixture.runtime.background();
    let executor = fixture_executor(FixtureControl {
        trace: Some(trace.clone()),
        before_term_socket: Some(socket),
    });
    let invocation = ToolInvocation {
        id: rustx::tools::types::ToolInvocationId::Agent {
            call_id: rustx::runtime::identity::ToolCallId::new("gated-term"),
        },
        tool_id: rustx::runtime::identity::ToolId::new("tool-bash"),
        tool_name: "bash".into(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({"command": command}),
    };
    let prepared = registry
        .prepare_dispatch(
            &invocation,
            &executor,
            fixture.runtime.environment().clone(),
        )
        .unwrap();
    let rustx::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .unwrap()
    else {
        panic!("accepted")
    };
    ready.read().await; // Trap is installed before this byte is emitted.
    assert_eq!(
        registry.cancel(&execution_id).unwrap().state,
        BackgroundLifecycle::Cancelling
    );
    let (mut gate, _) = listener.accept().await.unwrap();
    gate.read_exact(&mut [0]).await.unwrap(); // Inner owns TERMINATE, has not sent TERM.
    assert_eq!(
        registry.snapshot(&execution_id).unwrap().state,
        BackgroundLifecycle::Cancelling
    );
    let before = std::fs::read_to_string(&trace).unwrap();
    assert!(
        !before
            .lines()
            .map(|line| serde_json::from_str::<Entry>(line).unwrap())
            .any(|entry| matches!(entry.event, Event::Signal { .. })),
        "{before}"
    );
    gate.write_all(b"go").await.unwrap();
    entered.read().await; // Actual Bash trap delivery, independent of Job intent.
    assert_eq!(
        registry.snapshot(&execution_id).unwrap().state,
        BackgroundLifecycle::Cancelling
    );
    release.release();
    let terminal =
        wait_for_lifecycle(registry, &execution_id, BackgroundLifecycle::Cancelled).await;
    let evidence = std::fs::read_to_string(&trace).unwrap();
    assert!(marker.exists(), "{terminal:?}\n{evidence}");
    let entries: Vec<Entry> = evidence
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let index = |predicate: fn(&Event) -> bool| {
        entries
            .iter()
            .position(|e| predicate(&e.event))
            .unwrap_or_else(|| panic!("missing event: {evidence}"))
    };
    let term = index(|e| {
        matches!(
            e,
            Event::Signal {
                signal: libc::SIGTERM,
                result: Some(0),
                ..
            }
        )
    });
    let observed = index(|e| matches!(e, Event::TerminalObserved));
    let reaped = index(|e| matches!(e, Event::DirectChildReaped));
    assert!(term < observed && observed < reaped, "{evidence}");
    let Event::Signal { pgid, .. } = entries[term].event else {
        unreachable!()
    };
    assert!(
        entries[..term]
            .iter()
            .any(|e| matches!(e.event, Event::ShellGroup { pgid: group, .. } if group == pgid)),
        "{evidence}"
    );
    // Normal TERM completion needs no grace escalation. Darwin's post-shell
    // fallback is distinct and must occur after the sole shell reaper's report.
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e.event, Event::GraceExpired)),
        "{evidence}"
    );
    assert!(!dir.path().join("untrusted-trace").exists());
    let shell = index(|e| matches!(e, Event::ShellExited { .. }));
    for (i, entry) in entries.iter().enumerate() {
        if matches!(
            entry.event,
            Event::Signal {
                signal: libc::SIGKILL,
                ..
            }
        ) {
            assert!(i > shell, "{evidence}");
        }
    }
}

#[tokio::test]
async fn ordinary_environment_cannot_arm_supervisor_term_gate() {
    tokio::time::timeout(Duration::from_secs(15), async {
        use rustx::tools::native::bash_supervisor::diagnostics::TERM_GATE_ENV;
        let dir = tempfile::Builder::new().prefix("rx-env-").tempdir_in("/tmp").unwrap();
        let socket = dir.path().join("gate");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let fixture = native_fixture_with_environment(vec![
            (TERM_GATE_ENV.into(), socket.display().to_string()),
            ("RUSTX_TEST_FAIL_SIGNAL".into(), "1".into()),
            ("RUSTX_TEST_FORCE_ANCHOR_LOSS".into(), "1".into()),
            ("RUSTX_TEST_FAIL_CONTAINMENT".into(), "1".into()),
        ]);
        let root = fixture.runtime.workspace().root();
        let ready_path = root.join("ready");
        let entered_path = root.join("entered");
        let ready = SignalPipe::new(&ready_path);
        let entered = SignalPipe::new(&entered_path);
        let cancellation = CancellationSignal::new();
        let controller = async {
            ready.read().await;
            cancellation.cancel();
            entered.read().await;
        };
        // The readiness byte also proves the same-named value reached Bash.
        let command = format!(
            "test \"$RUSTX_TEST_BEFORE_TERM_SOCKET\" = '{}' || exit 7; trap 'printf T > {}; exit 0' TERM; printf R > {}; while :; do :; done",
            socket.display(), entered_path.display(), ready_path.display()
        );
        let (result, ()) = tokio::join!(run_tool_with_cancellation(&fixture, "bash", serde_json::json!({"command": command}), cancellation.clone()), controller);
        assert!(matches!(result.status, ToolExecutionStatus::Cancelled { .. }), "{result:?}");
        // Physical settlement closes the observation interval. Any connection
        // attempted before TERM would still be queued, even if its peer exited.
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }).await.expect("ordinary environment cancellation must settle");
}

#[tokio::test]
async fn ordinary_environment_cannot_enable_supervisor_controls() {
    use rustx::tools::native::bash_supervisor::diagnostics::TRACE_ENV;
    use std::fmt::Write;
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("trace");
    let anchor = dir.path().join("anchor");
    let keys = [
        "RUSTX_SUPERVISOR_ROLE",
        "RUSTX_SUPERVISOR_COMMAND",
        "RUSTX_TEST_FAIL_SIGNAL",
        "RUSTX_TEST_FAIL_WAIT",
        "RUSTX_TEST_FAIL_BASH_SPAWN",
        "RUSTX_TEST_FAIL_SIGTERM_HANDLER",
        "RUSTX_TEST_FORCE_ANCHOR_LOSS",
        "RUSTX_TEST_OUTER_BARRIER_DIR",
        "RUSTX_TEST_AFTER_ANCHOR_STOP_SOCKET",
        "RUSTX_TEST_FAIL_CONTAINMENT",
        "RUSTX_PHYSICAL_CONTINUATION",
        "RUSTX_COMMAND_ENVIRONMENT",
    ];
    let mut environment: Vec<_> = keys
        .iter()
        .map(|key| ((*key).into(), "user-visible-value".into()))
        .collect();
    environment.push((TRACE_ENV.into(), trace.display().to_string()));
    environment.push((
        "RUSTX_SUPERVISOR_ANCHOR_PID_FILE".into(),
        anchor.display().to_string(),
    ));
    environment.push((
        "RUSTX_TEST_BEFORE_TERM_SOCKET".into(),
        "user-visible-value".into(),
    ));
    let fixture = native_fixture_with_environment(environment);
    let mut command = String::new();
    for key in keys {
        write!(command, "test \"${key}\" = user-visible-value || exit 9; ").unwrap();
    }
    command.push_str(
        "test \"$RUSTX_TEST_BEFORE_TERM_SOCKET\" = user-visible-value || exit 10; printf isolated",
    );
    let result = run_tool(&fixture, "bash", serde_json::json!({"command": command})).await;
    assert_eq!(result.status, ToolExecutionStatus::Success, "{result:?}");
    assert_eq!(json_content(&result)["exit_code"], 0);
    assert_eq!(json_content(&result)["stdout"], "isolated");
    assert!(
        !trace.exists(),
        "ordinary command data enabled supervisor tracing"
    );
    assert!(
        !anchor.exists(),
        "ordinary command data enabled supervisor observability"
    );
}

#[tokio::test]
async fn description_changes_neither_native_bash_execution_nor_registry_approval() {
    let fixture = native_fixture();
    let command = "printf stable-output";
    let before = fixture
        .registry
        .definitions()
        .into_iter()
        .find(|tool| tool.name == "bash")
        .unwrap();
    let plain = run_tool(&fixture, "bash", serde_json::json!({"command": command})).await;
    let described = run_tool(&fixture, "bash", serde_json::json!({"command": command, "description": "Claim something entirely different"})).await;
    assert_eq!(plain.status, described.status);
    assert_eq!(plain.exit_code, described.exit_code);
    assert_eq!(plain.content, described.content);
    let after = fixture
        .registry
        .definitions()
        .into_iter()
        .find(|tool| tool.name == "bash")
        .unwrap();
    assert_eq!(before.approval_policy, after.approval_policy);
    assert_eq!(before.execution_policy, after.execution_policy);
    assert_eq!(before.id, after.id);
}

#[tokio::test]
async fn direct_image_read_without_attempt_authority_fails_before_path_or_artifact_work() {
    let fixture = native_fixture();
    let before = std::fs::read_dir(fixture.runtime.artifacts().root())
        .unwrap()
        .count();
    let result = run_tool(
        &fixture,
        "read_image",
        serde_json::json!({"path": "/definitely-not-present/image.png"}),
    )
    .await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { ref error } if error.contains("image-capable admitted invocation"))
    );
    assert!(
        result
            .content
            .iter()
            .all(|block| !matches!(block, ToolResultContent::Image(_)))
    );
    assert_eq!(
        std::fs::read_dir(fixture.runtime.artifacts().root())
            .unwrap()
            .count(),
        before
    );
}
