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
  "conversationId": "conv-subagent",
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
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(root.join("models.json"), models).expect("models.json");
        std::fs::write(root.join("session.json"), session).expect("session.json");
        let mut command = tokio::process::Command::new(binary());
        command
            .arg("--models")
            .arg(root.join("models.json"))
            .arg("--session")
            .arg(root.join("session.json"))
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
