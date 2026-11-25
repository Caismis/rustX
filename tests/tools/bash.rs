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

use crate::common::{native_fixture_with_execution, run_tool, run_tool_with_cancellation};
use rustx::runtime::CancellationSignal;
use rustx::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolResultContent,
};

fn fixture_with_trace(trace: &std::path::Path) -> crate::common::NativeFixture {
    fixture_with_control(
        &rustx_process_supervisor::bash_supervisor::diagnostics::FixtureControl {
            trace: Some(trace.to_path_buf()),
            before_term_socket: None,
        },
    )
}

fn fixture_with_control(
    control: &rustx_process_supervisor::bash_supervisor::diagnostics::FixtureControl,
) -> crate::common::NativeFixture {
    use rustx::tools::native::fixture_executor;
    let mut fixture = crate::common::native_fixture();
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
    std::fs::read_dir(fixture.runtime.tool_output().root().unwrap())
        .expect("managed tool-output results root")
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with("r-"))
        })
        .count()
}

#[tokio::test]
async fn bash_uses_full_shell_semantics() {
    let fixture = native_fixture_with_execution(Vec::new());
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
    let fixture = native_fixture_with_execution(Vec::new());
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
    let fixture = native_fixture_with_execution(Vec::new());
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
    let fixture = native_fixture_with_execution(Vec::new());
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
    let fixture = native_fixture_with_execution(Vec::new());
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
    let fixture = native_fixture_with_execution(Vec::new());
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
    use rustx_process_supervisor::bash_supervisor::diagnostics::{Entry, Event, FixtureControl};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Keep the socket path within Darwin's native path bound.
    let dir = tempfile::Builder::new()
        .prefix("rx-term-")
        .tempdir_in("/tmp")
        .unwrap();
    let trace = dir.path().join("trace");
    let socket = dir.path().join("gate");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let fixture = fixture_with_control(&FixtureControl {
        trace: Some(trace.clone()),
        before_term_socket: Some(socket),
    });
    let ready_path = fixture.runtime.workspace().root().join("ready");
    let ready = SignalPipe::new(&ready_path);
    let cancellation = CancellationSignal::new();
    let command = format!("printf R > {}; while :; do :; done", ready_path.display());

    // The FIFO byte proves the real shell is running. The native gate then
    // parks after MSG_TERMINATE is parsed, before the group signal syscall.
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let execution = run_tool_with_cancellation(
            &fixture,
            "bash",
            serde_json::json!({"command": command}),
            cancellation.clone(),
        );
        let controller = async {
            ready.read().await;
            cancellation.cancel();
            let (mut gate, _) = listener.accept().await.unwrap();
            gate.read_exact(&mut [0]).await.unwrap();
            let before = std::fs::read_to_string(&trace).unwrap();
            let entries: Vec<Entry> = before
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert!(
                entries
                    .iter()
                    .any(|e| matches!(e.event, Event::TerminateReceived)),
                "{before}"
            );
            assert!(
                !entries
                    .iter()
                    .any(|e| matches!(e.event, Event::Signal { .. })),
                "{before}"
            );
            gate.write_all(b"go").await.unwrap();
        };
        let (result, ()) = tokio::join!(execution, controller);
        result
    })
    .await
    .expect("foreground cancellation must settle");
    assert!(matches!(
        result.status,
        ToolExecutionStatus::Cancelled { .. }
    ));

    let evidence = std::fs::read_to_string(&trace).unwrap();
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
    let owned = index(|e| matches!(e, Event::ShellGroup { pid, pgid } if *pid > 0 && *pgid > 0));
    let received = index(|e| matches!(e, Event::TerminateReceived));
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
    assert!(
        owned < received && received < term && term < observed && observed < reaped,
        "{evidence}"
    );
    let Event::ShellGroup {
        pgid: owned_pgid, ..
    } = entries[owned].event
    else {
        unreachable!()
    };
    let Event::Signal { pgid, .. } = entries[term].event else {
        unreachable!()
    };
    assert_eq!(pgid, owned_pgid, "{evidence}");
}

#[tokio::test]
async fn bash_kill_escalates_when_term_is_ignored() {
    use rustx_process_supervisor::bash_supervisor::diagnostics::{Entry, Event};
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

struct SignalReleaseGuard(SignalPipe);

impl Drop for SignalReleaseGuard {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test]
async fn bash_cancellation_terminates_descendants_of_the_process_group() {
    use nix::sys::signal::kill;
    use nix::unistd::{Pid, getpgid};
    use rustx_process_supervisor::bash_supervisor::diagnostics::{Entry, Event, FixtureControl};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Keep the socket path within Darwin's native path bound.
    let dir = tempfile::Builder::new()
        .prefix("rx-desc-")
        .tempdir_in("/tmp")
        .unwrap();
    let trace = dir.path().join("trace");
    let socket = dir.path().join("gate");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let fixture = fixture_with_control(&FixtureControl {
        trace: Some(trace.clone()),
        before_term_socket: Some(socket),
    });
    let workspace = fixture.runtime.workspace().root().to_path_buf();
    let ready_path = workspace.join("descendant-ready.fifo");
    let hold_path = workspace.join("descendant-hold.fifo");
    let desc_pid_path = workspace.join("descendant.pid");
    let ready = SignalPipe::new(&ready_path);
    // Keep the write end open across cancellation; dropping on panic/unwind
    // releases any remaining blocked `read` without signaling raw numeric PIDs.
    let _hold_guard = SignalReleaseGuard(SignalPipe::new(&hold_path));
    let command = format!(
        "sh -c 'printf \"%s\\n\" \"$$\" > {}; printf R > {}; IFS= read -r _ < {}' & wait",
        desc_pid_path.display(),
        ready_path.display(),
        hold_path.display(),
    );
    let cancellation = CancellationSignal::new();
    let (
        result,
        (
            descendant_pid,
            shell_pid,
            owned_pgid,
            pre_terminate_received,
            pre_signal_emitted,
            pre_reaped_or_terminal,
            pre_alive,
            pre_pgid,
        ),
    ) = tokio::time::timeout(Duration::from_secs(15), async {
        let execution = run_tool_with_cancellation(
            &fixture,
            "bash",
            serde_json::json!({"command": command}),
            cancellation.clone(),
        );
        let controller = async {
            ready.read().await;
            let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_path)
                .expect("descendant pid file")
                .trim()
                .parse()
                .expect("descendant pid");
            cancellation.cancel();
            let (mut gate, _) = listener.accept().await.unwrap();
            gate.read_exact(&mut [0]).await.unwrap();
            let before = std::fs::read_to_string(&trace).unwrap();
            let before_entries: Vec<Entry> = before
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let (shell_pid, owned_pgid) = before_entries
                .iter()
                .find_map(|entry| match entry.event {
                    Event::ShellGroup { pid, pgid } if pid > 0 && pgid > 0 => Some((pid, pgid)),
                    _ => None,
                })
                .expect("ShellGroup recorded before TERM");
            let pre_terminate_received = before_entries
                .iter()
                .any(|entry| matches!(entry.event, Event::TerminateReceived));
            let pre_signal_emitted = before_entries
                .iter()
                .any(|entry| matches!(entry.event, Event::Signal { .. }));
            let pre_reaped_or_terminal = before_entries.iter().any(|entry| {
                matches!(
                    entry.event,
                    Event::GroupChildrenReaped
                        | Event::GroupAbsenceProven
                        | Event::TerminalPublished
                        | Event::TerminalObserved
                        | Event::DirectChildReaped
                )
            });
            let pre_alive = kill(Pid::from_raw(descendant_pid), None).is_ok();
            let pre_pgid = getpgid(Some(Pid::from_raw(descendant_pid)))
                .ok()
                .map(Pid::as_raw);
            // Always release the native gate so the owned supervisor settles
            // before any assertion can fail.
            gate.write_all(b"go").await.unwrap();
            (
                descendant_pid,
                shell_pid,
                owned_pgid,
                pre_terminate_received,
                pre_signal_emitted,
                pre_reaped_or_terminal,
                pre_alive,
                pre_pgid,
            )
        };
        tokio::join!(execution, controller)
    })
    .await
    .expect("descendant cancellation must settle");

    assert!(
        pre_terminate_received,
        "TerminateReceived must be logged before TERM gate"
    );
    assert!(
        !pre_signal_emitted,
        "no Signal may be sent before TERM gate release"
    );
    assert!(
        !pre_reaped_or_terminal,
        "group reap or terminal state must not be reported while descendant {descendant_pid} is live at TERM gate"
    );
    assert_ne!(
        descendant_pid, shell_pid,
        "ready handshake must come from an actual descendant process"
    );
    assert!(
        pre_alive,
        "descendant pid {descendant_pid} must be alive when cancellation reaches the gate"
    );
    assert_eq!(
        pre_pgid,
        Some(owned_pgid),
        "descendant pid {descendant_pid} must belong to owned process group {owned_pgid}"
    );
    assert!(matches!(
        result.status,
        ToolExecutionStatus::Cancelled { .. }
    ));

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
    let owned =
        index(|event| matches!(event, Event::ShellGroup { pid, pgid } if *pid > 0 && *pgid > 0));
    let received = index(|event| matches!(event, Event::TerminateReceived));
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
    let group_reaped = index(|event| matches!(event, Event::GroupChildrenReaped));
    let published = index(|event| matches!(event, Event::TerminalPublished));
    let observed = index(|event| matches!(event, Event::TerminalObserved));
    let direct_reaped = index(|event| matches!(event, Event::DirectChildReaped));
    assert!(
        owned < received
            && received < term
            && term < group_reaped
            && group_reaped < published
            && published < direct_reaped
            && group_reaped < observed
            && observed < direct_reaped,
        "{evidence}"
    );
    #[cfg(target_os = "macos")]
    {
        let absence_proven = index(|event| matches!(event, Event::GroupAbsenceProven));
        assert!(
            group_reaped < absence_proven
                && absence_proven < published
                && absence_proven < observed,
            "{evidence}"
        );
    }
    let Event::Signal { pgid, .. } = entries[term].event else {
        unreachable!()
    };
    assert_eq!(pgid, owned_pgid, "{evidence}");
}

#[tokio::test]
async fn bash_cancellation_does_not_kill_unrelated_processes() {
    use std::process::{Command, Stdio};
    struct UnrelatedGuard(std::process::Child);
    impl Drop for UnrelatedGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    // An unrelated process in the test's own process group held by its own
    // piped stdin (released/killed when UnrelatedGuard drops).
    let mut unrelated = UnrelatedGuard(
        Command::new("/bin/sh")
            .args(["-c", "read hold"])
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn"),
    );
    let unrelated_pid = unrelated.0.id();
    let fixture = native_fixture_with_execution(Vec::new());
    let cancellation = CancellationSignal::new();
    // The SignalPipe proves the tool's own child is running inside the
    // workspace before cancellation fires, while an owned FIFO blocks it until
    // cancellation terminates the group; the unrelated process in the test's
    // own process group must never observe the group signal.
    let root = fixture.runtime.workspace().root().to_path_buf();
    let ready_path = root.join("unrelated-ready.fifo");
    let hold_path = root.join("unrelated-hold.fifo");
    let ready = SignalPipe::new(&ready_path);
    let _hold_guard = SignalReleaseGuard(SignalPipe::new(&hold_path));
    let cancelling = cancellation.clone();
    let controller = async {
        tokio::time::timeout(Duration::from_secs(15), ready.read())
            .await
            .expect("the tool child readiness pipe never signalled");
        cancelling.cancel();
    };
    let (result, ()) = tokio::join!(
        run_tool_with_cancellation(
            &fixture,
            "bash",
            serde_json::json!({
                "command": format!(
                    "printf R > {}; IFS= read -r _ < {}",
                    ready_path.display(),
                    hold_path.display()
                )
            }),
            cancellation,
        ),
        controller
    );
    assert!(matches!(
        result.status,
        ToolExecutionStatus::Cancelled { .. }
    ));
    // The tool's TERM/KILL path (grace included) completed before the tool
    // returned and targets only the owned process group; the unrelated
    // process in the test's own group must still be running.
    assert!(
        unrelated.0.try_wait().expect("try_wait").is_none(),
        "the unrelated process (pid {unrelated_pid}) must survive group cancellation"
    );
}

#[tokio::test]
async fn bash_background_cancellation_uses_the_same_process_group_path() {
    use rustx::tools::background::BackgroundLifecycle;
    use rustx_process_supervisor::bash_supervisor::diagnostics::{Entry, Event};
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
        .commit_dispatch_owned(prepared, &CancellationSignal::new())
        .await
        .expect("dispatch commits");
    let rustx::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        outcome
    else {
        panic!("accepted");
    };
    tokio::time::timeout(Duration::from_secs(15), ready.read())
        .await
        .expect("background shell readiness");
    let running = registry.snapshot(&execution_id).expect("snapshot");
    assert_eq!(running.state, BackgroundLifecycle::Running);
    let cancelling = registry.cancel(&execution_id).expect("cancel");
    assert_eq!(cancelling.state, BackgroundLifecycle::Cancelling);
    // The terminal settlement follows the cancellation path.
    let terminal = tokio::time::timeout(
        Duration::from_secs(15),
        registry.wait_until_terminal(&execution_id),
    )
    .await
    .expect("terminal settlement")
    .expect("background execution terminal");
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
    let fixture = native_fixture_with_execution(Vec::new());
    let registry = fixture.runtime.background().clone();
    let invocation = ToolInvocation {
        id: rustx::tools::types::ToolInvocationId::Agent {
            call_id: rustx::runtime::identity::ToolCallId::new("call-bg"),
        },
        tool_id: rustx::runtime::identity::ToolId::new("tool-bash"),
        tool_name: "bash".to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({"command": "echo done"}),
    };
    let cancellation = rustx::runtime::ExecutionCancellation::detached(
        CancellationSignal::new(),
        rustx::runtime::types::CancellationReason::UserRequested,
    );
    let prepared = registry
        .prepare_native_bash(
            &invocation,
            fixture.runtime.environment().clone(),
            &cancellation,
        )
        .await
        .expect("prepare");
    let outcome = registry
        .commit_dispatch_owned(prepared, &CancellationSignal::new())
        .await
        .expect("dispatch commits");
    let rustx::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        outcome
    else {
        panic!("accepted");
    };
    let terminal = tokio::time::timeout(
        Duration::from_secs(15),
        registry.wait_until_terminal(&execution_id),
    )
    .await
    .expect("terminal settlement")
    .expect("background execution terminal");
    assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
    let after_cancel = registry.cancel(&execution_id).expect("cancel");
    assert_eq!(
        after_cancel.state,
        BackgroundLifecycle::Succeeded,
        "completion committed first; the later cancel is an idempotent no-op"
    );
}

#[test]
fn bash_excludes_server_environment_and_applies_configured_overrides() {
    let test_env = std::env::var("RUSTX_SENTINEL_TEST").is_ok();
    if test_env {
        // Inner run: execute the Bash tool and print its observable stdout.
        let fixture = native_fixture_with_execution(vec![(
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
    // the Server test process, then prove the bound Client only sees
    // the admitted configuration.
    let exe = std::env::current_exe().expect("current exe");
    let output = std::process::Command::new(exe)
        .arg("--exact")
        .arg("bash::bash_excludes_server_environment_and_applies_configured_overrides")
        .arg("--nocapture")
        .env("RUSTX_SENTINEL_TEST", "1")
        .env("RUSTX_SENTINEL_SECRET", "shhh")
        .env("RUSTX_AUTHORIZED", "host-value")
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
        "Server host data must stay private and configured overrides must win: {stdout}"
    );
}

/// Small output stays fully in memory: ordinary bounded text, no spill
/// file, no artifact, no truncation.
#[tokio::test]
async fn bash_small_output_creates_no_spill() {
    let fixture = native_fixture_with_execution(Vec::new());
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
    let fixture = native_fixture_with_execution(Vec::new());
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
        result.managed_output.as_deref()
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
        !std::path::Path::new(full_output).exists(),
        "capture belongs to the Client"
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
    assert_eq!(spill_count(&fixture), 0, "no Server spill");
    assert!(
        !content["combined"]
            .as_str()
            .unwrap()
            .contains("spill-boundary-marker"),
        "the middle marker falls outside the bounded preview"
    );
    let expected: String = (1..=1500)
        .map(|i| format!("line-{i}\n"))
        .chain(std::iter::once("spill-boundary-marker\n".to_owned()))
        .chain((1501..=3000).map(|i| format!("line-{i}\n")))
        .collect();
    assert_eq!(truncation.original_bytes, Some(expected.len() as u64));
    for (offset, limit) in [(1, 1000), (1001, 1000), (2001, 1001)] {
        let read = run_tool(
            &fixture,
            "read",
            serde_json::json!({"path": full_output, "offset": offset, "limit": limit}),
        )
        .await;
        assert_eq!(read.status, ToolExecutionStatus::Success);
        let ToolResultContent::Text(text) = &read.content[0] else {
            panic!("Read text")
        };
        let expected = expected
            .split('\n')
            .skip(offset - 1)
            .take(limit)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            text.text.split("\n\n[").next().unwrap().trim_end(),
            expected.trim_end()
        );
    }
    let grep = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "spill-boundary-marker", "path": full_output}),
    )
    .await;
    assert_eq!(grep.status, ToolExecutionStatus::Success);
    let ToolResultContent::Text(text) = &grep.content[0] else {
        panic!("Grep text")
    };
    let name = locator.file_name().unwrap().to_str().unwrap();
    assert_eq!(text.text, format!("{name}:1501: spill-boundary-marker"));
}

/// Continuations preserve lossy UTF-8 decoding across the physical boundary.
#[tokio::test]
async fn bash_non_utf8_output_spills_as_deterministic_text() {
    let fixture = native_fixture_with_execution(Vec::new());
    let result = run_tool(
        &fixture, "bash",
        serde_json::json!({"command": r"printf 'x%.0s' {1..20000}; printf '\377\376\001\002end\n'"}),
    ).await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    let Some(rustx::tools::ManagedOutputContinuation::Complete { locator }) =
        result.managed_output.as_deref()
    else {
        panic!(
            "expected a complete Client capture: {:?}",
            result.managed_output
        )
    };
    let read = run_tool(&fixture, "read", serde_json::json!({"path": locator})).await;
    assert_eq!(read.status, ToolExecutionStatus::Success);
    let ToolResultContent::Text(text) = &read.content[0] else {
        panic!("Read text")
    };
    assert_eq!(
        text.text,
        format!("{}\u{FFFD}\u{FFFD}\u{1}\u{2}end\n", "x".repeat(20_000))
    );
    let grep = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "end", "path": locator}),
    )
    .await;
    assert_eq!(grep.status, ToolExecutionStatus::Success);
    let ToolResultContent::Text(text) = &grep.content[0] else {
        panic!("Grep text")
    };
    assert!(text.text.contains(":1:"), "{}", text.text);
}

/// A shell parent that exits while a descendant stays in the owned process
/// group and keeps the output pipe open cannot escape the invocation
/// timeout: the runtime keeps supervising the owned group and terminates
/// it.
#[tokio::test]
async fn bash_shell_exit_with_descendant_holding_the_pipe_still_times_out() {
    let fixture = native_fixture_with_execution(Vec::new());
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
    let fixture = native_fixture_with_execution(Vec::new());
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
    let fixture = native_fixture_with_execution(Vec::new());
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
    let fixture = native_fixture_with_execution(Vec::new());
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

/// The macOS Bash EXIT `wait` is a best-effort convenience, not an
/// ownership or terminality primitive. Replacing it (`trap ':' EXIT`) must
/// not fabricate terminality: the shell exits immediately, the background
/// job outlives it, and the macOS fallback containment (anchored `SIGKILL`
/// + `killpg(pgid, 0)` absence probe) must actually terminate that job
/// before the tool settles.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn bash_replaced_exit_trap_does_not_fabricate_terminality() {
    let fixture = native_fixture_with_execution(Vec::new());
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
    let fixture = native_fixture_with_execution(Vec::new());
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
        arguments: crate::common::tool_call_envelope(
            serde_json::json!({"execution_mode": mode, "command": "echo selectable"}),
        ),
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
    let cancellation = rustx::runtime::ExecutionCancellation::detached(
        CancellationSignal::new(),
        rustx::runtime::types::CancellationReason::UserRequested,
    );
    let dispatch = background
        .prepare_native_bash(
            &prepared.invocation,
            fixture.runtime.environment().clone(),
            &cancellation,
        )
        .await
        .expect("prepare background dispatch");
    let rustx::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        background
            .commit_dispatch_owned(dispatch, &CancellationSignal::new())
            .await
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
    let reject = |envelope: serde_json::Value| {
        let outcome = fixture
            .registry
            .preflight(&ToolCall {
                id: ToolCallId::new("call-bash-selectable"),
                tool_id: bash.id.clone(),
                name: "bash".to_owned(),
                arguments: envelope,
            })
            .expect("preflight outcome");
        let PreflightOutcome::Rejected { error, .. } = outcome else {
            panic!("expected a deterministic rejection");
        };
        error
    };

    let missing = reject(serde_json::json!({"arguments": {"command": "echo hi"}}));
    assert!(
        missing.contains("\"execution_mode\": \"foreground\"")
            && missing.contains("\"execution_mode\": \"background\""),
        "the model is told exactly how to retry: {missing}"
    );
    let invalid = reject(
        serde_json::json!({"execution_mode": "detached", "arguments": {"command": "echo hi"}}),
    );
    assert!(
        invalid.contains("execution_mode") && invalid.contains("detached"),
        "the invalid value is named: {invalid}"
    );
    let retired = reject(
        serde_json::json!({"__rustx_execution": "background", "arguments": {"command": "echo hi"}}),
    );
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
    use rustx_process_supervisor::bash_supervisor::diagnostics::{Entry, Event, FixtureControl};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // Bounded native socket namespace, independent of macOS TMPDIR length.
    let dir = tempfile::Builder::new()
        .prefix("rx-term-")
        .tempdir_in("/tmp")
        .unwrap();
    let trace = dir.path().join("trace");
    let socket = dir.path().join("gate");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let fixture = native_fixture_with_execution(vec![
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
    let executor = rustx::tools::native::fixture_executor(FixtureControl {
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
            .commit_dispatch_owned(prepared, &CancellationSignal::new())
            .await
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
    let terminal = tokio::time::timeout(
        Duration::from_secs(15),
        registry.wait_until_terminal(&execution_id),
    )
    .await
    .expect("terminal settlement")
    .expect("background execution terminal");
    assert_eq!(terminal.state, BackgroundLifecycle::Cancelled);
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
        use rustx_process_supervisor::bash_supervisor::diagnostics::TERM_GATE_ENV;
        let dir = tempfile::Builder::new().prefix("rx-env-").tempdir_in("/tmp").unwrap();
        let socket = dir.path().join("gate");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let fixture = native_fixture_with_execution(vec![
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
        // The readiness byte also proves the control value was removed before Bash.
        let command = format!(
            "test -z \"${{RUSTX_TEST_BEFORE_TERM_SOCKET+x}}\" || exit 7; trap 'printf T > {}; exit 0' TERM; printf R > {}; while :; do :; done",
            entered_path.display(), ready_path.display()
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
    use rustx_process_supervisor::bash_supervisor::diagnostics::TRACE_ENV;
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
        "RUSTX_COMMAND_ENVIRONMENT_FILE",
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
    let fixture = native_fixture_with_execution(environment);
    let mut command = String::new();
    for key in keys.into_iter().chain([
        TRACE_ENV,
        "RUSTX_SUPERVISOR_ANCHOR_PID_FILE",
        "RUSTX_TEST_BEFORE_TERM_SOCKET",
    ]) {
        write!(command, "test -z \"${{{key}+x}}\" || exit 9; ").unwrap();
    }
    command.push_str("printf isolated");
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
    let fixture = native_fixture_with_execution(Vec::new());
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

    assert_eq!(before.execution_policy, after.execution_policy);
    assert_eq!(before.id, after.id);
}

#[tokio::test]
async fn direct_image_read_without_attempt_authority_fails_before_path_or_artifact_work() {
    let fixture = native_fixture_with_execution(Vec::new());
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
