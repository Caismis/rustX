//! Issue #60: native async one-shot subagents end to end through the real
//! `rustx` binary.
//!
//! The parent process is driven over the stdio/JSONL Runtime Client
//! transport exactly like Issue #42. The deterministic provider fixture
//! routes by request body: the parent's first turn answers with a `subagent`
//! tool call, the child runtime (a real second `rustx` process in
//! `--subagent-child` mode) asks the same fixture about its delegated task,
//! and the parent's continuation turns answer with plain text.
//!
//! # No sleep-based readiness
//!
//! Every wait is a protocol round trip: the driver polls the authoritative
//! snapshot until the committed ledger shows the expected content. The
//! child's own liveness is bounded by its startup and delegation envelopes,
//! not by the test.

mod common;

use std::process::Stdio;
use std::sync::Arc;

use rustx::runtime_client::types::{
    RuntimeClientProtocolEvent, RuntimeClientRequest, RuntimeClientResponse, RuntimeClientResult,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// The outer liveness guard of one process interaction.
const LIVENESS: std::time::Duration = std::time::Duration::from_secs(120);

/// The `rustx` binary under test, built by cargo alongside this test.
fn binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_rustx"))
}

/// A catalog pointing at a local fixture server.
fn models_json(base_url: &str) -> String {
    format!(
        r#"{{
  "providers": {{
    "fixture": {{
      "baseUrl": "{base_url}",
      "apiKey": "$RUSTX_SUBAGENT_TEST_KEY",
      "models": [
        {{
          "id": "subagent-model",
          "protocol": "openai_chat_completions",
          "contextWindow": 128000,
          "maxOutputTokens": 512,
          "capabilities": {{
            "inputModalities": ["text"],
            "outputModalities": ["text"],
            "toolCalls": true,
            "reasoning": false
          }},
          "compat": {{"chatReasoningReplay": "omit"}},
          "requestParams": {{"temperature": 0.11}}
        }}
      ]
    }}
  }}
}}"#
    )
}

const SESSION_JSON: &str = r#"{
  "agentId": "agent-parent",
  "model": {"model": "fixture/subagent-model"},
  "context": {"reserveTokens": 1024, "keepRecentTokens": 8192}
}"#;

/// One spawned `rustx` process wired to its stdio JSONL transport.
struct Process {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    next_id: u64,
}

impl Process {
    /// Spawns the binary with explicit startup arguments.
    fn spawn(root: &std::path::Path, models: &str, session: &str, key: &str) -> Self {
        Self::launch(root, models, session, key, false)
    }

    /// Reopens the Session this runtime root already published as active. A
    /// launch is not a resume, so recovering a durable conversation across a
    /// process death is an explicit `--continue`.
    fn reopen(root: &std::path::Path, models: &str, session: &str, key: &str) -> Self {
        Self::launch(root, models, session, key, true)
    }

    fn launch(
        root: &std::path::Path,
        models: &str,
        session: &str,
        key: &str,
        continue_active: bool,
    ) -> Self {
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(root.join("models.jsonc"), models).expect("models.jsonc");
        std::fs::write(root.join("rustx.jsonc"), session).expect("rustx.jsonc");
        let mut command = tokio::process::Command::new(binary());
        command
            .arg("--models")
            .arg(root.join("models.jsonc"))
            .arg("--config")
            .arg(root.join("rustx.jsonc"))
            .arg("--workspace")
            .arg(&workspace)
            .arg("--runtime-root")
            .arg(root.join("private"))
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("RUSTX_SUBAGENT_TEST_KEY", key)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if continue_active {
            command.arg("--continue");
        }
        let mut child = command.spawn().expect("spawn the rustx binary");
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    /// Sends one request and returns its correlated response, skipping any
    /// notification lines that arrive first.
    async fn request(
        &mut self,
        build: impl FnOnce(u64) -> RuntimeClientRequest,
    ) -> RuntimeClientResponse {
        let id = self.next_id;
        self.next_id += 1;
        let request = build(id);
        let line = serde_json::to_string(&request).expect("serialize the request");
        tokio::time::timeout(LIVENESS, async {
            self.stdin.write_all(line.as_bytes()).await.expect("write");
            self.stdin.write_all(b"\n").await.expect("write newline");
            self.stdin.flush().await.expect("flush");
            loop {
                let mut record = String::new();
                let read = self
                    .stdout
                    .read_line(&mut record)
                    .await
                    .expect("read a protocol record");
                assert!(read > 0, "the process closed stdout before responding");
                if serde_json::from_str::<RuntimeClientProtocolEvent>(record.trim()).is_ok() {
                    continue;
                }
                let response: RuntimeClientResponse = serde_json::from_str(record.trim())
                    .unwrap_or_else(|error| {
                        panic!("stdout must carry protocol records only: {record:?} ({error})")
                    });
                assert_eq!(response.id.get(), id, "responses correlate by request id");
                return response;
            }
        })
        .await
        .expect("the process must answer")
    }

    /// Closes the transport input and waits for the process to exit.
    async fn close_and_wait(mut self) -> (std::process::ExitStatus, String) {
        drop(self.stdin);
        let status = tokio::time::timeout(LIVENESS, self.child.wait())
            .await
            .expect("the process must exit after transport EOF")
            .expect("wait");
        let mut stderr = String::new();
        if let Some(mut handle) = self.child.stderr.take() {
            use tokio::io::AsyncReadExt;
            let _ = handle.read_to_string(&mut stderr).await;
        }
        (status, stderr)
    }
}

/// Routes one provider request by body content. The parent and the child
/// runtime share this fixture, so attempt order is NOT the routing key.
fn route(body: &str) -> common::FixtureReply {
    if body.contains("CHILD-ANSWER") {
        // The parent's post-settlement turn: the child's answer arrived as a
        // new inbound from the child agent.
        common::sse_fixture("openai_chat", "subagent_parent_final.sse")
    } else if body.contains("\\\"role\\\":\\\"tool\\\"") || body.contains("\"role\":\"tool\"") {
        // The parent's delegation-acknowledgment turn, right after the
        // `subagent` tool result committed.
        common::sse_fixture("openai_chat", "plain_text.sse")
    } else if body.contains("count the workspace files") {
        // The child runtime's own model call: the delegated task is its user
        // message.
        common::sse_fixture("openai_chat", "subagent_child_answer.sse")
    } else {
        // The parent's first turn.
        common::sse_fixture("openai_chat", "subagent_tool_call.sse")
    }
}

/// The hard-parent-death fixture gates the child's provider response. The
/// parent receives its delegation tool call normally; only the real child
/// request is held, which makes the child nonterminal while the parent is
/// killed abruptly.
fn hard_parent_death_route(body: &str, gate: &Arc<common::HeaderGate>) -> common::FixtureReply {
    let has_tool_history =
        body.contains("\\\"role\\\":\\\"tool\\\"") || body.contains("\"role\":\"tool\"");
    if body.contains("please delegate hard parent death gate") && !has_tool_history {
        common::sse_fixture("openai_chat", "subagent_hard_death_tool_call.sse")
    } else if body.contains("hard parent death gate") && !has_tool_history {
        common::sse_fixture("openai_chat", "subagent_child_answer.sse")
            .with_header_gate(Arc::clone(gate))
    } else {
        // If recovery admits its Runtime-authored interruption, the model
        // receives a harmless ordinary answer. It must never receive a new
        // delegation tool call.
        common::sse_fixture("openai_chat", "plain_text.sse")
    }
}

#[cfg(unix)]
fn direct_subagent_pids(parent_pid: u32) -> Vec<u32> {
    let output = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,args="])
        .output()
        .expect("ps must be available for the process regression");
    assert!(output.status.success(), "ps must succeed");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let process_parent_pid = fields.next()?.parse::<u32>().ok()?;
            let args = fields.collect::<Vec<_>>().join(" ");
            (process_parent_pid == parent_pid && args.contains("--subagent-child")).then_some(pid)
        })
        .collect()
}

#[cfg(unix)]
fn process_state(pid: u32) -> Option<char> {
    let output = std::process::Command::new("ps")
        .args(["-o", "state=", "-p", &pid.to_string()])
        .output()
        .expect("ps must be available for the process regression");
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .chars()
        .next()
}

#[cfg(unix)]
async fn wait_for_direct_subagent(parent_pid: u32) -> Vec<u32> {
    tokio::time::timeout(LIVENESS, async {
        loop {
            let children = direct_subagent_pids(parent_pid);
            if !children.is_empty() {
                return children;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the real child process must become observable")
}

#[cfg(unix)]
async fn wait_for_no_running_processes(pids: &[u32]) {
    tokio::time::timeout(LIVENESS, async {
        loop {
            let running = pids
                .iter()
                .filter_map(|pid| process_state(*pid))
                .any(|state| state != 'Z');
            if !running {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the old child process must exit after parent death");
}

fn has_runtime_interrupted_notice(message: &rustx::message::types::MessageBlock) -> bool {
    matches!(
        message,
        rustx::message::types::MessageBlock::User(user)
            if matches!(user.source, rustx::message::types::UserSource::Runtime)
                && user.content.iter().any(|block| match block {
                    rustx::message::types::UserContentBlock::Text(text) => {
                        text.text.contains("was interrupted")
                    }
                    _ => false,
                })
    )
}

/// The complete subagent lifecycle through the real binary: the parent's
/// model delegates, the `subagent` intrinsic durably commits the child, a
/// real second `rustx` process boots headlessly, asks the provider about its
/// task, and its answer arrives in the parent's conversation as a new
/// message authored by the child agent. The parent's next turn consumes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn a_subagent_child_runs_end_to_end_through_the_real_process_stack() {
    let server = common::FixtureServer::start_with_body(|_attempt, _head, body| route(body)).await;
    let root = tempfile::tempdir().expect("temp root");
    let mut process = Process::spawn(
        root.path(),
        &models_json(&server.url("/v1")),
        SESSION_JSON,
        "subagent-secret",
    );

    // start -> initialize
    let response = process
        .request(|id| RuntimeClientRequest::Initialize {
            id: rustx::runtime_client::RequestId::new(id),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
        })
        .await;
    let Some(RuntimeClientResult::Initialized { snapshot, .. }) = response.result else {
        panic!("initialize must succeed: {response:?}");
    };
    assert!(snapshot.subagents.is_empty(), "no children at start");

    // The composed tool surface includes the subagent intrinsic.
    let response = process
        .request(|id| RuntimeClientRequest::CapabilityGet {
            id: rustx::runtime_client::RequestId::new(id),
        })
        .await;
    let Some(RuntimeClientResult::Capability { capabilities }) = response.result else {
        panic!("capability_get must succeed: {response:?}");
    };
    assert!(
        capabilities
            .tools
            .iter()
            .any(|tool| tool.name.as_str() == "subagent"),
        "the subagent intrinsic is composed: {:?}",
        capabilities
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>()
    );

    // submit the delegating turn
    let response = process
        .request(|id| RuntimeClientRequest::SubmitInbound {
            id: rustx::runtime_client::RequestId::new(id),
            content: vec![rustx::message::types::UserContentBlock::Text(
                rustx::message::content::TextBlock {
                    text: "please delegate".to_owned(),
                },
            )],
        })
        .await;
    assert!(
        matches!(
            response.result,
            Some(RuntimeClientResult::InboundAccepted { .. })
        ),
        "submit_inbound must be accepted: {response:?}"
    );

    // Poll the authoritative snapshot until the whole chain is visible:
    // the tool call committed, the child settled, its answer arrived as an
    // Agent-authored message, and the parent consumed it.
    let mut final_snapshot = None;
    for _ in 0..4_000 {
        let response = process
            .request(|id| RuntimeClientRequest::SnapshotGet {
                id: rustx::runtime_client::RequestId::new(id),
            })
            .await;
        let Some(RuntimeClientResult::Snapshot { snapshot, .. }) = response.result else {
            panic!("snapshot_get must succeed: {response:?}");
        };
        let child_answer = snapshot.messages.iter().any(|message| match message {
            rustx::message::types::MessageBlock::User(user) => {
                matches!(user.source, rustx::message::types::UserSource::Agent { .. })
                    && user.content.iter().any(|block| match block {
                        rustx::message::types::UserContentBlock::Text(text) => {
                            text.text.contains("CHILD-ANSWER")
                        }
                        _ => false,
                    })
            }
            _ => false,
        });
        let parent_consumed = snapshot.messages.iter().any(|message| match message {
            rustx::message::types::MessageBlock::Assistant(assistant) => {
                assistant.content.iter().any(|block| match block {
                    rustx::message::types::AssistantContentBlock::Text(text) => {
                        text.text.contains("The child counted three files.")
                    }
                    _ => false,
                })
            }
            _ => false,
        });
        let settled = snapshot
            .subagents
            .iter()
            .any(|subagent| subagent.state == rustx::runtime::subagent::SubagentState::Succeeded);
        if child_answer && parent_consumed && settled {
            final_snapshot = Some(snapshot);
            break;
        }
        tokio::task::yield_now().await;
    }
    let Some(snapshot) = final_snapshot else {
        let bodies = (0..server.attempt_count())
            .map(|index| server.request_body(usize::try_from(index).expect("index fits usize")))
            .collect::<Vec<_>>();
        drop(process.stdin);
        let status = tokio::time::timeout(LIVENESS, process.child.wait())
            .await
            .expect("the process must exit after transport EOF");
        let mut stderr = String::new();
        if let Some(mut handle) = process.child.stderr.take() {
            use tokio::io::AsyncReadExt;
            let _ = handle.read_to_string(&mut stderr).await;
        }
        panic!(
            "the delegation chain must settle (exit: {status:?})\nstderr:\n{stderr}\n\
             requests: {}\n{}",
            bodies.len(),
            bodies.join("\n---\n")
        );
    };

    // The registry surface is the authoritative lifecycle: one child,
    // succeeded, carrying the bounded terminal detail.
    assert_eq!(snapshot.subagents.len(), 1, "exactly one child was owned");
    let subagent = &snapshot.subagents[0];
    assert_eq!(
        subagent.state,
        rustx::runtime::subagent::SubagentState::Succeeded
    );
    assert_eq!(subagent.profile, "explore");
    let detail = subagent.detail.clone().expect("the terminal detail");
    assert!(
        detail.contains("CHILD-ANSWER"),
        "the child's answer is the terminal detail: {detail}"
    );

    // The dedicated status surface answers with the same snapshot.
    let response = process
        .request(|id| RuntimeClientRequest::SubagentStatus {
            id: rustx::runtime_client::RequestId::new(id),
            subagent_id: subagent.subagent_id.clone(),
        })
        .await;
    let Some(RuntimeClientResult::SubagentStatus { subagent: status }) = response.result else {
        panic!("subagent_status must succeed: {response:?}");
    };
    assert_eq!(
        status.state,
        rustx::runtime::subagent::SubagentState::Succeeded
    );
    assert_eq!(status.subagent_id, subagent.subagent_id);
    assert_eq!(status.child_agent_id, subagent.child_agent_id);

    // The tool result of the `subagent` call carries the accepted identity.
    let tool_message = snapshot
        .messages
        .iter()
        .find_map(|message| match message {
            rustx::message::types::MessageBlock::Tool(tool) => Some(tool.clone()),
            _ => None,
        })
        .expect("the subagent tool result is committed");
    let tool_text = serde_json::to_string(&tool_message).expect("tool message json");
    assert!(
        tool_text.contains(subagent.subagent_id.as_str()),
        "the accepted tool result carries the subagent identity: {tool_text}"
    );

    // The child really called the provider: its request carried the task and
    // the child's own requests never saw the parent's tool definitions. The
    // parent's continuation requests also carry the task inside the tool-call
    // history, so the child request is the one WITHOUT the parent's original
    // user message.
    let bodies = (0..server.attempt_count())
        .map(|index| server.request_body(usize::try_from(index).expect("index fits usize")))
        .collect::<Vec<_>>();
    let child_request = bodies
        .iter()
        .find(|body| {
            body.contains("count the workspace files") && !body.contains("please delegate")
        })
        .expect("the child runtime called the provider");
    assert!(
        !child_request.contains("\"name\":\"subagent\""),
        "the child has no subagent tool (no recursion): {child_request}"
    );
    assert!(
        child_request.contains("You are a read-only exploration subagent of the rustX runtime"),
        "the child persona is request-time AgentProfile System authority: {child_request}"
    );

    // shutdown and clean exit
    let response = process
        .request(|id| RuntimeClientRequest::Shutdown {
            id: rustx::runtime_client::RequestId::new(id),
        })
        .await;
    assert!(
        matches!(
            response.result,
            Some(RuntimeClientResult::ShutdownCompleted)
        ),
        "shutdown must complete: {response:?}"
    );
    let (status, stderr) = process.close_and_wait().await;
    assert!(
        status.success(),
        "the process must exit cleanly: {status} stderr={stderr}"
    );
}

/// A real parent process is killed with SIGKILL while its real child is
/// blocked in a deterministic provider response. The child observes control
/// EOF, drains, and exits; the next runtime boot classifies the durable
/// nonterminal ownership as Interrupted exactly once, without adoption,
/// replay, or relaunch.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn hard_parent_death_terminates_child_and_recovery_is_idempotent() {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use std::os::unix::process::ExitStatusExt;

    let gate = common::HeaderGate::new();
    let gate_for_server = Arc::clone(&gate);
    let server = common::FixtureServer::start_with_body(move |_attempt, _head, body| {
        hard_parent_death_route(body, &gate_for_server)
    })
    .await;
    let root = tempfile::tempdir().expect("temp root");
    let models = models_json(&server.url("/v1"));

    let mut parent = Process::spawn(root.path(), &models, SESSION_JSON, "subagent-secret");
    let response = parent
        .request(|id| RuntimeClientRequest::Initialize {
            id: rustx::runtime_client::RequestId::new(id),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
        })
        .await;
    assert!(
        matches!(
            response.result,
            Some(RuntimeClientResult::Initialized { .. })
        ),
        "initial parent must initialize: {response:?}"
    );
    let response = parent
        .request(|id| RuntimeClientRequest::SubmitInbound {
            id: rustx::runtime_client::RequestId::new(id),
            content: vec![rustx::message::types::UserContentBlock::Text(
                rustx::message::content::TextBlock {
                    text: "please delegate hard parent death gate".to_owned(),
                },
            )],
        })
        .await;
    assert!(
        matches!(
            response.result,
            Some(RuntimeClientResult::InboundAccepted { .. })
        ),
        "delegation inbound must be accepted: {response:?}"
    );

    // The provider gate is reached only by the child's model request, so this
    // is a deterministic nonterminal frontier rather than a timing guess.
    tokio::time::timeout(LIVENESS, gate.wait_entered())
        .await
        .expect("the child must reach the gated provider response");
    let parent_pid = parent.child.id().expect("parent pid");
    let child_pids = wait_for_direct_subagent(parent_pid).await;
    assert_eq!(child_pids.len(), 1, "one real child process is owned");

    // This is an actual abrupt parent death: no Runtime Client Shutdown and
    // no graceful transport EOF are sent before SIGKILL.
    kill(
        Pid::from_raw(i32::try_from(parent_pid).expect("pid fits")),
        Signal::SIGKILL,
    )
    .expect("SIGKILL the real parent");
    let parent_status = tokio::time::timeout(LIVENESS, parent.child.wait())
        .await
        .expect("hard-killed parent must be waitable")
        .expect("wait parent");
    assert_eq!(parent_status.signal(), Some(Signal::SIGKILL as i32));
    drop(parent.stdin);

    // The child's only route to semantic completion is the parent control
    // socket. It must leave after EOF while the provider response remains
    // gated; then release the server task's held socket response.
    wait_for_no_running_processes(&child_pids).await;
    gate.release();

    // Reopen the same durable conversation. Recovery has no child process to
    // adopt and publishes one Runtime-authored Interrupted inbound.
    let mut recovered = Process::reopen(root.path(), &models, SESSION_JSON, "subagent-secret");
    let response = recovered
        .request(|id| RuntimeClientRequest::Initialize {
            id: rustx::runtime_client::RequestId::new(id),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
        })
        .await;
    assert!(
        matches!(
            response.result,
            Some(RuntimeClientResult::Initialized { .. })
        ),
        "restarted parent must initialize: {response:?}"
    );

    let mut recovered_snapshot = None;
    for _ in 0..4_000 {
        let response = recovered
            .request(|id| RuntimeClientRequest::SnapshotGet {
                id: rustx::runtime_client::RequestId::new(id),
            })
            .await;
        let Some(RuntimeClientResult::Snapshot { snapshot, .. }) = response.result else {
            panic!("snapshot_get must succeed after recovery: {response:?}");
        };
        let interrupted = snapshot
            .messages
            .iter()
            .filter(|message| has_runtime_interrupted_notice(message))
            .count();
        if interrupted == 1 {
            recovered_snapshot = Some(snapshot);
            break;
        }
        tokio::task::yield_now().await;
    }
    let snapshot = recovered_snapshot.expect("recovery notice must become observable");
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|message| has_runtime_interrupted_notice(message))
            .count(),
        1,
        "recovery publishes exactly one Interrupted notice"
    );
    assert!(
        snapshot.subagents.is_empty(),
        "recovery does not reattach a live registry child"
    );
    assert!(
        !snapshot.messages.iter().any(|message| match message {
            rustx::message::types::MessageBlock::User(user) => user.content.iter().any(|block| {
                matches!(
                    block,
                    rustx::message::types::UserContentBlock::Text(text)
                        if text.text.contains("CHILD-ANSWER")
                )
            }),
            _ => false,
        }),
        "recovery never fabricates the gated child success"
    );
    assert!(
        direct_subagent_pids(recovered.child.id().expect("restarted parent pid")).is_empty(),
        "recovery never relaunches the old child"
    );

    let response = recovered
        .request(|id| RuntimeClientRequest::Shutdown {
            id: rustx::runtime_client::RequestId::new(id),
        })
        .await;
    assert!(matches!(
        response.result,
        Some(RuntimeClientResult::ShutdownCompleted)
    ));
    let (status, stderr) = recovered.close_and_wait().await;
    assert!(status.success(), "recovered runtime shuts down: {stderr}");

    // A second restart must observe the absorbing terminal identity and must
    // not publish a second Runtime notice or relaunch anything.
    let mut repeated = Process::reopen(root.path(), &models, SESSION_JSON, "subagent-secret");
    let response = repeated
        .request(|id| RuntimeClientRequest::Initialize {
            id: rustx::runtime_client::RequestId::new(id),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
        })
        .await;
    assert!(
        matches!(
            response.result,
            Some(RuntimeClientResult::Initialized { .. })
        ),
        "repeated restart must initialize: {response:?}"
    );
    let response = repeated
        .request(|id| RuntimeClientRequest::SnapshotGet {
            id: rustx::runtime_client::RequestId::new(id),
        })
        .await;
    let Some(RuntimeClientResult::Snapshot { snapshot, .. }) = response.result else {
        panic!("snapshot_get must succeed on repeated restart: {response:?}");
    };
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|message| has_runtime_interrupted_notice(message))
            .count(),
        1,
        "repeated restart is idempotent"
    );
    assert!(snapshot.subagents.is_empty());
    assert!(direct_subagent_pids(repeated.child.id().expect("repeated pid")).is_empty());
    let response = repeated
        .request(|id| RuntimeClientRequest::Shutdown {
            id: rustx::runtime_client::RequestId::new(id),
        })
        .await;
    assert!(matches!(
        response.result,
        Some(RuntimeClientResult::ShutdownCompleted)
    ));
    let (status, stderr) = repeated.close_and_wait().await;
    assert!(status.success(), "repeated runtime shuts down: {stderr}");
}
