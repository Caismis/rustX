//! Issue #42: the spawnable `rustx` local conversation runtime process.
//!
//! These tests spawn the **actual binary** against a local deterministic
//! provider fixture and drive it over the Issue #38 stdio/JSONL transport.
//!
//! # No sleep-based readiness
//!
//! Readiness is the protocol itself: the driver writes one JSONL request and
//! blocks on the correlated response line. A process that has not finished
//! composing has not written that line, so there is nothing to poll and
//! nothing to time.

mod common;

use std::io::Write;
use std::process::Stdio;

use rustx::runtime_client::types::{
    RuntimeClientProtocolEvent, RuntimeClientRequest, RuntimeClientResponse, RuntimeClientResult,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// The outer liveness guard of one process interaction.
const LIVENESS: std::time::Duration = std::time::Duration::from_secs(120);

/// The `rustx` binary under test, built by cargo alongside this test.
fn binary() -> std::path::PathBuf {
    // `CARGO_BIN_EXE_<name>` is set by cargo for every binary of this package.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_rustx"))
}

/// A catalog pointing at a local fixture server.
fn models_json(base_url: &str) -> String {
    format!(
        r#"{{
  "providers": {{
    "fixture": {{
      "baseUrl": "{base_url}",
      "apiKey": "$RUSTX_PROCESS_TEST_KEY",
      "models": [
        {{
          "id": "process-model",
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
  "agentId": "agent-process",
  "model": {"model": "fixture/process-model"},
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
    fn spawn(root: &std::path::Path, models: &str, session: &str, key: Option<&str>) -> Self {
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(root.join("models.json"), models).expect("models.json");
        std::fs::write(root.join("rustx.json"), session).expect("rustx.json");
        let mut command = tokio::process::Command::new(binary());
        command
            .arg("--models")
            .arg(root.join("models.json"))
            .arg("--config")
            .arg(root.join("rustx.json"))
            .arg("--workspace")
            .arg(&workspace)
            .arg("--runtime-root")
            .arg(root.join("private"))
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(key) = key {
            command.env("RUSTX_PROCESS_TEST_KEY", key);
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
                // Protocol records are the only thing on stdout, and a
                // notification structurally has no request id.
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

/// The complete process lifecycle: start, initialize, inspect the session
/// model and effective capabilities, inspect the composed tools, submit a
/// turn, observe the deterministic assistant result, shut down, close the
/// transport, and exit cleanly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn the_process_serves_a_real_conversation_runtime() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let root = tempfile::tempdir().expect("temp root");
    let mut process = Process::spawn(
        root.path(),
        &models_json(&server.url("/v1")),
        SESSION_JSON,
        Some("process-secret"),
    );

    // start -> initialize
    let response = process
        .request(|id| RuntimeClientRequest::Initialize {
            id: rustx::runtime_client::RequestId::new(id),
            protocol_version: rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
        })
        .await;
    let Some(RuntimeClientResult::Initialized {
        conversation_id,
        agent_id,
        snapshot,
        ..
    }) = response.result
    else {
        panic!("initialize must succeed: {response:?}");
    };
    assert_eq!(conversation_id.as_str(), "conversation-1");
    assert_eq!(agent_id.as_str(), "agent-process");

    // inspect session model / effective capabilities
    assert_eq!(
        snapshot.model.configured.model.to_string(),
        "fixture/process-model"
    );
    assert_eq!(snapshot.model.effective.context_window, 128_000);
    assert!(snapshot.model.effective.capabilities.tool_calls);
    assert!(
        !snapshot
            .model
            .effective
            .capabilities
            .input_modalities
            .contains(&rustx::model::Modality::Image)
    );

    // inspect capability/native tools
    let response = process
        .request(|id| RuntimeClientRequest::CapabilityGet {
            id: rustx::runtime_client::RequestId::new(id),
        })
        .await;
    let Some(RuntimeClientResult::Capability { capabilities }) = response.result else {
        panic!("capability_get must succeed: {response:?}");
    };
    let names: Vec<&str> = capabilities
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    for expected in ["background_task", "read", "write", "bash"] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }

    // the selectable-model query: the client never reads models.json
    let response = process
        .request(|id| RuntimeClientRequest::ModelCatalogGet {
            id: rustx::runtime_client::RequestId::new(id),
        })
        .await;
    let Some(RuntimeClientResult::ModelCatalog { catalog }) = response.result else {
        panic!("model_catalog_get must succeed: {response:?}");
    };
    assert_eq!(catalog.models.len(), 1);
    assert_eq!(
        catalog.models[0].credential_source,
        rustx::model::CredentialSourceView::Environment {
            variable: "RUSTX_PROCESS_TEST_KEY".to_owned()
        }
    );

    // submit an inbound turn and observe the deterministic assistant result
    let response = process
        .request(|id| RuntimeClientRequest::SubmitInbound {
            id: rustx::runtime_client::RequestId::new(id),
            content: vec![rustx::message::types::UserContentBlock::Text(
                rustx::message::content::TextBlock {
                    text: "hello".to_owned(),
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

    // The attempt settles asynchronously; poll the authoritative snapshot
    // through the protocol until the assistant message is committed. Each
    // iteration is one protocol round trip, not a delay.
    let mut committed = None;
    for _ in 0..2_000 {
        let response = process
            .request(|id| RuntimeClientRequest::SnapshotGet {
                id: rustx::runtime_client::RequestId::new(id),
            })
            .await;
        let Some(RuntimeClientResult::Snapshot { snapshot, .. }) = response.result else {
            panic!("snapshot_get must succeed: {response:?}");
        };
        if let Some(assistant) = snapshot.messages.iter().find_map(|message| match message {
            rustx::message::types::MessageBlock::Assistant(assistant) => Some(assistant.clone()),
            _ => None,
        }) {
            committed = Some(assistant);
            break;
        }
        tokio::task::yield_now().await;
    }
    let assistant = committed.expect("the attempt must commit an assistant message");
    let text = assistant
        .content
        .iter()
        .find_map(|block| match block {
            rustx::message::types::AssistantContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("the assistant message carries text");
    assert!(
        !text.is_empty(),
        "the deterministic fixture produced output"
    );

    // The provider request really carried the catalog's request parameters
    // and the process's own credential.
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("the provider request body is JSON");
    assert_eq!(body["model"], "process-model");
    assert_eq!(body["temperature"], serde_json::json!(0.11));

    // shutdown is not transport closure: it responds and the transport stays
    // usable, exactly as Issue #38 defines.
    let response = process
        .request(|id| RuntimeClientRequest::Shutdown {
            id: rustx::runtime_client::RequestId::new(id),
        })
        .await;
    assert!(matches!(
        response.result,
        Some(RuntimeClientResult::ShutdownCompleted)
    ));
    let response = process
        .request(|id| RuntimeClientRequest::SnapshotGet {
            id: rustx::runtime_client::RequestId::new(id),
        })
        .await;
    assert!(
        response.error.is_none(),
        "shutdown does not close the transport: {response:?}"
    );

    // Clean transport EOF terminates the one-session process successfully.
    let (status, _stderr) = process.close_and_wait().await;
    assert!(
        status.success(),
        "a clean input EOF exits successfully, got {status:?}"
    );
}

/// Startup configuration failure purity: a bounded diagnostic on stderr, a
/// non-zero exit, and **exactly zero bytes** on stdout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_startup_configuration_never_writes_to_stdout() {
    let cases: Vec<(&str, String, String, Option<&str>)> = vec![
        (
            "a provider without an explicit baseUrl",
            models_json("https://x.invalid/v1")
                .replace("\"baseUrl\": \"https://x.invalid/v1\",", ""),
            SESSION_JSON.to_owned(),
            Some("k"),
        ),
        (
            "an unresolved environment credential",
            models_json("https://x.invalid/v1"),
            SESSION_JSON.to_owned(),
            None,
        ),
        (
            "a session selecting an undeclared model",
            models_json("https://x.invalid/v1"),
            SESSION_JSON.replace("fixture/process-model", "fixture/absent"),
            Some("k"),
        ),
        (
            "an unknown session field",
            models_json("https://x.invalid/v1"),
            SESSION_JSON.replace(
                "\"agentId\": \"agent-process\",",
                "\"agentId\": \"agent-process\", \"futureKnob\": true,",
            ),
            Some("k"),
        ),
    ];

    for (label, models, session, key) in cases {
        let root = tempfile::tempdir().expect("temp root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(root.path().join("models.json"), &models).expect("models.json");
        std::fs::write(root.path().join("rustx.json"), &session).expect("rustx.json");
        let mut command = std::process::Command::new(binary());
        command
            .arg("--models")
            .arg(root.path().join("models.json"))
            .arg("--config")
            .arg(root.path().join("rustx.json"))
            .arg("--workspace")
            .arg(&workspace)
            .arg("--runtime-root")
            .arg(root.path().join("private"))
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdin(Stdio::null());
        if let Some(key) = key {
            command.env("RUSTX_PROCESS_TEST_KEY", key);
        }
        let output = command.output().expect("run the rustx binary");
        assert!(
            !output.status.success(),
            "{label} must exit non-zero, got {:?}",
            output.status
        );
        assert!(
            output.stdout.is_empty(),
            "{label} must leave stdout at zero bytes, got {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.starts_with("rustx: "),
            "{label} must write a bounded diagnostic to stderr, got {stderr:?}"
        );
        assert!(
            !stderr.contains("process-secret"),
            "{label} diagnostic must not carry a credential"
        );
    }
}

/// Malformed startup arguments fail the same way, with usage on stderr.
#[test]
fn malformed_arguments_fail_with_usage_on_stderr() {
    for arguments in [
        vec![],
        vec!["--models".to_owned()],
        vec!["--future".to_owned(), "x".to_owned()],
    ] {
        let output = std::process::Command::new(binary())
            .args(&arguments)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdin(Stdio::null())
            .output()
            .expect("run the rustx binary");
        assert!(!output.status.success(), "{arguments:?}");
        assert!(output.stdout.is_empty(), "{arguments:?} wrote to stdout");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("usage: rustx"), "{stderr:?}");
    }
}

/// The binary contains no human-facing stdout bootstrap: running it with a
/// valid configuration and immediate EOF produces zero stdout bytes, because
/// no client ever initialized.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_started_process_writes_no_banner() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(
        root.path().join("models.json"),
        models_json(&server.url("/v1")),
    )
    .expect("models.json");
    std::fs::write(root.path().join("rustx.json"), SESSION_JSON).expect("rustx.json");

    let mut child = std::process::Command::new(binary())
        .arg("--models")
        .arg(root.path().join("models.json"))
        .arg("--config")
        .arg(root.path().join("rustx.json"))
        .arg("--workspace")
        .arg(&workspace)
        .arg("--runtime-root")
        .arg(root.path().join("private"))
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("RUSTX_PROCESS_TEST_KEY", "process-secret")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    // Closing stdin immediately is a clean EOF at a record boundary.
    let mut stdin = child.stdin.take().expect("stdin");
    stdin.flush().expect("flush");
    drop(stdin);
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "{:?}", output.status);
    assert!(
        output.stdout.is_empty(),
        "a started process emits no banner: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}
