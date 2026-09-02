//! Issue #138 (real process boundary): a launched **named** subagent child
//! inherits the parent runtime's frozen `ModelTimeoutPolicy` and applies it
//! inside its own ordinary Agent Loop — response-start deadline, generic
//! transient retry, bounded failure — while the parent observes exactly one
//! terminal notice and never a retry.
//!
//! This is the one conformance case that must cross the real child process
//! boundary: the frozen policy travels through the typed `SubagentChildSpec`
//! handshake into the real child composition. The named definition is
//! resolved from the invoking resource snapshot and selects both a frozen
//! Builtin and a Skill, so the child crosses the current #144 resolver and
//! #145 child-owned Skill materialization boundary before it reaches the
//! Agent Loop. The deterministic gate is reached by every child provider
//! request and is never released; the child's very small finite timeout
//! policy (300ms response-start) is the only trigger. Retry ordinals,
//! backoff, and terminal publication are the child's ordinary Issue #134
//! semantics — the four child provider requests (R0 + 3 retries) and the
//! single parent `Failed` notice are the observable proof. No race or
//! precedence correctness lives here; those are proven by the deterministic
//! in-process suites.

use std::process::Stdio;
use std::sync::Arc;

use rustx::runtime_client::types::{
    RuntimeClientProtocolEvent, RuntimeClientRequest, RuntimeClientResponse, RuntimeClientResult,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// The outer subprocess conformance guard. The inherited 300ms response-start
/// policy completes the scripted run in roughly 16s (2s + 4s + 8s of real
/// retry backoff plus four short deadlines). It is deliberately below the
/// default 30s response-start timeout: if child composition re-defaults the
/// policy, the first gated request cannot settle before this guard expires.
const LIVENESS: std::time::Duration = std::time::Duration::from_secs(27);

fn binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_rustx"))
}

/// A catalog pointing at the local fixture server.
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
          "compat": {{"chatReasoningReplay": "omit"}}
        }}
      ]
    }}
  }}
}}"#
    )
}

/// The launch configuration with the deliberately tiny frozen timeout
/// policy that every launched child must inherit.
const SESSION_JSON: &str = r#"{
  "agentId": "agent-parent",
  "model": {"model": "fixture/subagent-model"},
  "context": {"reserveTokens": 1024, "keepRecentTokens": 8192},
  "defaultTools": ["read", "subagent"],
  "modelTimeoutPolicy": {"responseStartTimeoutMs": 300, "streamIdleTimeoutMs": 300},
  "subagents": {
    "maxConcurrent": 4,
    "definitions": {
      "conformance": {
        "description": "Issue 138 named conformance child.",
        "instructionsFile": ".agents/subagents/conformance/instructions.md",
        "tools": {"builtin": ["read"]},
        "skills": ["conformance"]
      }
    },
    "main": ["conformance"],
    "workflow": []
  }
}"#;

/// One spawned `rustx` process wired to its stdio JSONL transport.
struct Process {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    next_id: u64,
}

impl Process {
    fn spawn(root: &std::path::Path, models: &str, session: &str, key: &str) -> Self {
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(workspace.join(".agents/subagents/conformance"))
            .expect("subagent resources");
        std::fs::write(
            workspace.join(".agents/subagents/conformance/instructions.md"),
            "Execute the delegated conformance task exactly as requested.\n",
        )
        .expect("subagent instructions");
        let skill = workspace.join(".agents/skills/conformance");
        std::fs::create_dir_all(&skill).expect("skill package");
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: conformance\ndescription: Issue 138 conformance skill.\n---\n\nUse the ordinary child runtime.\n",
        )
        .expect("skill manifest");
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
        command.kill_on_drop(true);
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
/// Every child request hits the gate that is never released; the child's
/// inherited response-start deadline is what ends each attempt request.
fn route(body: &str, gate: &Arc<crate::common::HeaderGate>) -> crate::common::FixtureReply {
    let has_tool_history =
        body.contains("\\\"role\\\":\\\"tool\\\"") || body.contains("\"role\":\"tool\"");
    if body.contains("count the workspace files") && !body.contains("please delegate") {
        // The child runtime's own model call: the delegated task is its
        // user message. The response headers never arrive.
        crate::common::sse_fixture("openai_chat", "subagent_child_answer.sse")
            .with_header_gate(Arc::clone(gate))
    } else if has_tool_history {
        // The parent's turn after the child's terminal notice arrived.
        crate::common::sse_fixture("openai_chat", "plain_text.sse")
    } else {
        // The parent's first turn: delegate.
        crate::common::sse_fixture("openai_chat", "issue138_subagent_tool_call.sse")
    }
}

/// The child inherits the frozen timeout policy, times out on the gated
/// provider response, retries through its ordinary generic budget (R0 + 3
/// retries = 4 provider requests), and fails bounded. The parent receives
/// exactly one Runtime-authored failure notice with a timeout diagnostic
/// and consumes it in an ordinary continuation turn. Nothing about the
/// retries — ordinals, delays, a "retrying" state — is parent-visible.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_child_inherits_the_frozen_timeout_policy_and_retries_locally() {
    tokio::time::timeout(
        LIVENESS,
        run_real_child_inherits_the_frozen_timeout_policy_and_retries_locally(),
    )
    .await
    .expect(
        "the inherited 300ms child timeout must settle within 27s; a default 30s timeout cannot",
    );
}

#[allow(clippy::too_many_lines)]
async fn run_real_child_inherits_the_frozen_timeout_policy_and_retries_locally() {
    let gate = crate::common::HeaderGate::new();
    let gate_for_server = Arc::clone(&gate);
    let server = crate::common::FixtureServer::start_with_body(move |_attempt, _head, body| {
        route(body, &gate_for_server)
    })
    .await;
    let root = tempfile::tempdir().expect("temp root");
    let mut process = Process::spawn(
        root.path(),
        &models_json(&server.url("/v1")),
        SESSION_JSON,
        "subagent-secret",
    );

    let response = process
        .request(|id| RuntimeClientRequest::Initialize {
            id: rustx::runtime_client::RequestId::new(id),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        })
        .await;
    assert!(
        matches!(
            response.result,
            Some(RuntimeClientResult::Initialized { .. })
        ),
        "initialize must succeed: {response:?}"
    );

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

    // The child request reaching the never-released gate is the
    // deterministic frontier: from here on, only the child's inherited
    // deadline can move the run forward.
    tokio::time::timeout(LIVENESS, gate.wait_entered())
        .await
        .expect("the child must reach the gated provider response");

    // Poll the authoritative snapshot until the child settles Failed and
    // the parent consumed the notice in an ordinary continuation turn.
    let mut final_snapshot = None;
    for _ in 0..8_000 {
        let response = process
            .request(|id| RuntimeClientRequest::SnapshotGet {
                id: rustx::runtime_client::RequestId::new(id),
            })
            .await;
        let Some(RuntimeClientResult::Snapshot { snapshot, .. }) = response.result else {
            panic!("snapshot_get must succeed: {response:?}");
        };
        let failed = snapshot
            .subagents
            .iter()
            .any(|subagent| subagent.state == rustx::runtime::subagent::SubagentState::Failed);
        let notice = snapshot.messages.iter().any(|message| match message {
            rustx::message::types::MessageBlock::User(user) => {
                matches!(user.source, rustx::message::types::UserSource::Runtime)
                    && user.content.iter().any(|block| match block {
                        rustx::message::types::UserContentBlock::Text(text) => {
                            text.text.contains("failed")
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
                        text.text.contains("Hello world")
                    }
                    _ => false,
                })
            }
            _ => false,
        });
        if failed && notice && parent_consumed {
            final_snapshot = Some(snapshot);
            break;
        }
        tokio::task::yield_now().await;
    }
    let Some(snapshot) = final_snapshot else {
        drop(process.stdin);
        let _ = process.child.wait().await;
        panic!("the child timeout/retry chain must settle within the liveness guard");
    };

    // Exactly one child, Failed, carrying the bounded timeout diagnostic.
    assert_eq!(snapshot.subagents.len(), 1, "exactly one child was owned");
    let subagent = &snapshot.subagents[0];
    assert_eq!(
        subagent.state,
        rustx::runtime::subagent::SubagentState::Failed
    );
    let detail = subagent.detail.clone().expect("the terminal detail");
    assert!(
        detail.contains("Timeout") || detail.contains("timed out"),
        "the bounded diagnostic is the child's own deadline outcome: {detail}"
    );

    // Exactly one Runtime-authored failure notice exists, and it contains
    // no child publication content (the child never produced any: every
    // request died at the response-start frontier).
    let notices: Vec<_> = snapshot
        .messages
        .iter()
        .filter(|message| match message {
            rustx::message::types::MessageBlock::User(user) => {
                matches!(user.source, rustx::message::types::UserSource::Runtime)
                    && user.content.iter().any(|block| match block {
                        rustx::message::types::UserContentBlock::Text(text) => {
                            text.text.contains("failed")
                        }
                        _ => false,
                    })
            }
            _ => false,
        })
        .collect();
    assert_eq!(
        notices.len(),
        1,
        "exactly one parent-facing terminal notice"
    );
    let notice_json = serde_json::to_string(&notices[0]).expect("notice json");
    assert!(
        !notice_json.contains("CHILD-ANSWER"),
        "no child publication content crosses: {notice_json}"
    );

    // The child really exercised its ordinary generic retry: exactly four
    // provider requests carry the delegated task (R0 plus three transient
    // retries), and the parent never reissued the delegation. Six requests
    // provably completed their send before the snapshot above could settle
    // (parent delegate, 4 gated child attempts, parent continuation — the
    // "Hello world" cut requires it); the parent's post-notice turn may or
    // may not have reached the server yet, so the total is not asserted.
    // Collect one consistent locked snapshot of the observed bodies instead
    // of indexing by the connection counter, which also counts
    // accepted-but-abandoned connections (a client deadline firing
    // mid-connect) and must not panic this proof.
    let mut bodies = server.request_bodies();
    for _ in 0..8_000 {
        if bodies.len() >= 6 {
            break;
        }
        tokio::task::yield_now().await;
        bodies = server.request_bodies();
    }
    assert!(
        bodies.len() >= 6,
        "all six settled provider requests were observed: {bodies:?}"
    );
    let child_requests = bodies
        .iter()
        .filter(|body| {
            body.contains("count the workspace files") && !body.contains("please delegate")
        })
        .count();
    assert_eq!(
        child_requests, 4,
        "R0 + 3 ordinary retries inside the child: {bodies:?}"
    );
    let first_child_request = bodies
        .iter()
        .find(|body| body.contains("count the workspace files"))
        .expect("the child request body");
    assert!(
        first_child_request.contains("skills/conformance/SKILL.md"),
        "the named child request uses the child-owned materialized Skill path: {first_child_request}"
    );
    let parent_delegations = bodies
        .iter()
        .filter(|body| {
            body.contains("please delegate")
                && !body.contains("\"role\":\"tool\"")
                && !body.contains("\\\"role\\\":\\\"tool\\\"")
        })
        .count();
    assert_eq!(parent_delegations, 1, "the parent never retries the child");

    // Release the gate so the fixture server's held handlers can finish,
    // then shut the parent down cleanly.
    gate.release();
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
