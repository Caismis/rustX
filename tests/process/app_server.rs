//! Real standalone process, pipe and network boundaries; shared semantic authority.
use super::app_server_readiness::websocket_startup;
use futures_util::{SinkExt, StreamExt};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use rustx::durable::{CanonicalStore, JournalStore};
use rustx::local_runtime::configuration::observation::SourceFieldName;
use rustx::{
    app_server::transport::MAX_MESSAGE_BYTES,
    local_runtime::{
        configuration::SessionConfigInput,
        session::{SessionId, SessionPersistentState},
        session_controller::SessionController,
    },
};
use std::{fmt::Write, path::PathBuf, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
};
use tokio_tungstenite::tungstenite::protocol::frame::{
    Frame,
    coding::{Data, OpCode},
};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
#[path = "../support/app_server_conformance.rs"]
mod app_server_conformance;
#[cfg(target_os = "macos")]
#[path = "../support/case_insensitive_volume.rs"]
mod case_insensitive_volume;
#[path = "../support/app_server_driver.rs"]
mod driver;

struct Fixture {
    root: tempfile::TempDir,
    sessions: [SessionId; 2],
}
impl Fixture {
    async fn new() -> Self {
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let home = root.path().join("home");
        let config = home.join("rustx");
        std::fs::create_dir_all(&config).unwrap();
        let initialized = Command::new(env!("CARGO_BIN_EXE_rustx"))
            .env("HOME", &home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_STATE_HOME")
            .arg("init")
            .args([
                "--template",
                "openai-chat",
                "--provider",
                "local",
                "--model-id",
                "test",
                "--endpoint",
                "http://127.0.0.1:9/v1",
                "--credential-env",
                "TEST_KEY",
                "--context-window",
                "128000",
                "--max-output",
                "4096",
                "--compat",
                "chat_reasoning_replay = \"omit\"",
            ])
            .output()
            .await
            .unwrap();
        assert!(
            initialized.status.success(),
            "{}",
            String::from_utf8_lossy(&initialized.stdout)
        );
        let authored = config.join("rustx.toml");
        let mut source: toml::Value =
            toml::from_str(&std::fs::read_to_string(&authored).unwrap()).unwrap();
        source["agent"].as_table_mut().unwrap().insert("tools".into(), toml::Value::try_from(serde_json::json!({"builtin": ["read", "write", "edit", "glob", "grep", "bash", "job_status"]})).unwrap());
        source["agent"].as_table_mut().unwrap().insert(
            "plugins".into(),
            toml::Value::try_from(
                serde_json::json!({"todo": {"enabled": true}, "agent_status": {"enabled": true}}),
            )
            .unwrap(),
        );
        std::fs::write(authored, toml::to_string_pretty(&source).unwrap()).unwrap();
        let controller = SessionController::open(&root.path().join("runtime")).unwrap();
        let mut sessions = Vec::new();
        for name in ["a", "b"] {
            let workspace = root.path().join(name);
            std::fs::create_dir(&workspace).unwrap();
            let session = controller
                .create_session(SessionPersistentState::from_input(
                    &SessionConfigInput::new(workspace),
                ))
                .await
                .unwrap()
                .session;
            controller
                .rename_session(&session.id, &format!("process fixture {name}"))
                .await
                .unwrap();
            sessions.push(session.id);
        }
        // Invalid project authoring at process cwd must never become Session cwd.
        std::fs::write(root.path().join("rustx.toml"), "invalid TOML at launch cwd").unwrap();
        std::fs::write(root.path().join("token"), driver::TOKEN).unwrap();
        Self {
            root,
            sessions: sessions.try_into().unwrap(),
        }
    }
    fn command(&self, listen: &str) -> Command {
        self.command_with_program(env!("CARGO_BIN_EXE_rustx"), listen)
    }
    fn command_with_program(&self, program: impl AsRef<std::ffi::OsStr>, listen: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(self.root.path())
            .env("HOME", self.root.path().join("home"))
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_STATE_HOME")
            .env("TEST_KEY", "fixture")
            .args(["app-server", "--runtime-root"])
            .arg(self.root.path().join("runtime"))
            .args(["--listen", listen])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }
    async fn ws(&self) -> (Child, String, BufReader<tokio::process::ChildStderr>) {
        let mut child = self
            .command("ws://127.0.0.1:0")
            .arg("--token-file")
            .arg(self.root.path().join("token"))
            .spawn()
            .unwrap();
        let (endpoint, stderr) = websocket_startup(&mut child).await;
        (child, endpoint, stderr)
    }
}
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_mins(1), future)
        .await
        .expect("outer liveness guard")
}
/// Reaps a detached `std` client process on scope exit.
struct ReapChild(std::process::Child);

impl Drop for ReapChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn terminate(child: &Child) {
    kill(
        Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()),
        Signal::SIGTERM,
    )
    .unwrap();
}
async fn detach_then_shutdown(child: &mut Child) {
    let mut lines = BufReader::new(child.stderr.as_mut().unwrap()).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    assert!(line.contains("stdio detached"), "{line}");
    assert!(
        child.try_wait().unwrap().is_none(),
        "EOF must not shut down the host"
    );
    terminate(child);
}

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol_version":81,"client":{"name":"boundary","version":"1"},"presentation":{"images":false,"questionnaires":false,"reviews":false}}}"#;

#[tokio::test]
async fn app_server_stdio_real_process_shared_conformance() {
    bounded(Box::pin(async {
        let f = Fixture::new().await;
        let mut child = f.command("stdio").spawn().unwrap();
        let driver = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        Box::pin(app_server_conformance::representative_scenario(
            &driver,
            f.sessions.clone(),
        ))
        .await;
        driver.close().await;
        detach_then_shutdown(&mut child).await;
        assert!(child.wait().await.unwrap().success());
    }))
    .await;
}

#[tokio::test]
async fn standalone_server_reads_cold_sessions_without_workspace_or_helpers() {
    bounded(async {
        use rustx::app_server::protocol::*;
        let f = Fixture::new().await;
        // Both recorded Session cwds become missing directories.
        for name in ["a", "b"] {
            std::fs::rename(
                f.root.path().join(name),
                f.root.path().join(format!("{name}-retired")),
            )
            .unwrap();
        }
        // A binary with no sibling helpers: the bare Server executable in
        // its own directory (hard link on the source's own filesystem).
        let (bin_dir, rustx) = isolated_server_bin();
        let bin = bin_dir.path().to_path_buf();
        // A stale handoff left behind under HOME must never be consumed: the
        // missing workspace path it names stays missing and the file stays put.
        let missing = f.root.path().join("a");
        let admit = f
            .root
            .path()
            .join("home/rustx/runtime/workspace-python/admit");
        std::fs::create_dir_all(&admit).unwrap();
        let stale = admit.join("stale.txt");
        std::fs::write(&stale, format!("{}\n", missing.display())).unwrap();
        let mut command = Command::new(&rustx);
        command
            .current_dir(f.root.path())
            .env_clear()
            .env("HOME", f.root.path().join("home"))
            .env("PATH", &bin)
            .args(["app-server", "--runtime-root"])
            .arg(f.root.path().join("runtime"))
            .args(["--listen", "stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().expect("spawn staged server");
        let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        initialize_client(&client).await;
        let MethodResult::Sessions { sessions, .. } = result(
            &client,
            Method::SessionList {
                query: None,
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("catalog")
        };
        for session in &f.sessions {
            assert!(
                sessions.iter().any(|summary| summary.id == *session),
                "cold Session {session:?} not listed"
            );
        }
        client.close().await;
        terminate(&child);
        let output = child.wait_with_output().await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Neither recorded cwd was resurrected, the stale handoff was ignored
        // (unread and undeleted), and nothing was created beside the executable.
        for name in ["a", "b"] {
            assert!(!f.root.path().join(name).exists());
        }
        assert_eq!(
            std::fs::read_to_string(&stale).unwrap(),
            format!("{}\n", missing.display())
        );
        assert_eq!(
            std::fs::read_dir(&bin)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>(),
            ["rustx"]
        );
    })
    .await;
}

#[tokio::test]
async fn standalone_server_commits_a_model_turn_without_a_client_or_helpers() {
    bounded(async {
        use rustx::app_server::protocol::*;
        let provider = crate::common::FixtureServer::start_with_body(|_, _, _| {
            crate::common::sse_fixture("openai_chat", "plain_text.sse")
        })
        .await;
        let fixture = Fixture::new().await;
        let home = fixture.root.path().join("home");
        let source = home.join("rustx/rustx.toml");
        let original = std::fs::read_to_string(&source).unwrap();
        std::fs::write(
            source,
            original.replace("http://127.0.0.1:9/v1", &provider.url("/v1")),
        )
        .unwrap();

        let (bin, rustx) = isolated_server_bin();
        let mut child = Command::new(rustx)
            .args(["app-server", "--listen", "stdio", "--runtime-root"])
            .arg(fixture.root.path().join("runtime"))
            .current_dir(fixture.root.path())
            .env_clear()
            .env("HOME", &home)
            .env("PATH", bin.path())
            .env("TEST_KEY", "fixture")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        initialize_client(&client).await;
        let session = fixture.sessions[0].clone();
        let target = attach(&client, session, 2).await;
        start_turn(&client, &target, "standalone model request").await;
        let state = settled(&client, &target).await;
        assert!(state["messages"].to_string().contains("Hello world"));
        let requests = provider.request_bodies();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("standalone model request"));
        assert_eq!(std::fs::read_dir(bin.path()).unwrap().count(), 1);
        assert_eq!(
            std::fs::read_dir(fixture.root.path().join("a"))
                .unwrap()
                .count(),
            0
        );
        result(&client, Method::SessionDetach { target }).await;
        client.close().await;
        detach_then_shutdown(&mut child).await;
        assert!(child.wait().await.unwrap().success());
    })
    .await;
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test]
async fn standalone_server_reports_mcp_overflow_without_launching_a_client_or_spilling() {
    bounded(Box::pin(async {
        use rmcp::transport::streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        };
        use rustx::{
            app_server::protocol::Method,
            durable::SqliteConversationStore,
            events::types::{AttemptOutcome, RuntimeEvent},
            message::types::MessageBlock,
            tools::{
                limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES,
                mcp::fixture::FixtureServer as McpFixture,
                types::{ManagedOutputContinuation, ToolExecutionStatus},
            },
        };
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = Fixture::new().await;
        let original_bytes = FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1;
        let mcp_calls = fixture.root.path().join("mcp-calls");
        let mcp = McpFixture {
            result_bytes: Some(original_bytes),
            echo_call_count_file: Some(mcp_calls.clone()),
            ..McpFixture::default()
        };
        let service = StreamableHttpService::<McpFixture, LocalSessionManager>::new(
            move || Ok(mcp.clone()),
            std::sync::Arc::default(),
            StreamableHttpServerConfig::default(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mcp_server = tokio::spawn(async move {
            axum::serve(listener, axum::Router::new().nest_service("/mcp", service))
                .await
                .unwrap();
        });
        let _mcp_server = tokio_util::task::AbortOnDropHandle::new(mcp_server);
        let provider = crate::common::FixtureServer::start(|attempt, _| {
            if attempt == 0 {
                crate::common::sse_fixture("openai_chat", "mcp_overflow_tool_call.sse")
            } else {
                crate::common::sse_fixture("openai_chat", "plain_text.sse")
            }
        })
        .await;

        let home = fixture.root.path().join("home");
        let catalog = home.join("rustx/rustx.toml");
        let authored = std::fs::read_to_string(&catalog)
            .unwrap()
            .replace("http://127.0.0.1:9/v1", &provider.url("/v1"));
        let mut authored: toml::Value = toml::from_str(&authored).unwrap();
        authored.as_table_mut().unwrap().insert(
            "approval_mode".to_owned(),
            toml::Value::String("full_access".to_owned()),
        );
        authored["agent"]["tools"].as_table_mut().unwrap().insert(
            "sources".to_owned(),
            toml::from_str("overflow = ['echo']").unwrap(),
        );
        std::fs::write(catalog, toml::to_string_pretty(&authored).unwrap()).unwrap();
        std::fs::create_dir_all(home.join("rustx/.agents")).unwrap();
        std::fs::write(
            home.join("rustx/.agents/mcp.toml"),
            format!("[mcp_servers.overflow]\nurl = \"http://{address}/mcp\"\n"),
        )
        .unwrap();

        let (bin, rustx) = isolated_server_bin();
        let launch_probe = bin.path().join("rustx-execution-client");
        std::fs::write(
            &launch_probe,
            "#!/bin/sh\nprintf 'launch\n' >> \"${0%/*}/client-launches\"\nexit 99\n",
        )
        .unwrap();
        std::fs::set_permissions(launch_probe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let scratch = fixture.root.path().join("scratch");
        std::fs::create_dir(&scratch).unwrap();
        let mut child = fixture
            .command_with_program(rustx, "stdio")
            .env_clear()
            .env("HOME", home)
            .env("PATH", bin.path())
            .env("TMPDIR", &scratch)
            .env("TEST_KEY", "fixture")
            .spawn()
            .unwrap();
        let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        initialize_client(&client).await;
        let session = &fixture.sessions[0];
        let target = attach(&client, session.clone(), 2).await;
        start_turn(&client, &target, "overflow without execution binding").await;
        let state = settled(&client, &target).await;
        assert_eq!(state["attempt"]["phase"]["outcome"]["type"], "completed");
        assert_eq!(
            std::fs::read_dir(&scratch).unwrap().count(),
            0,
            "no Spill before detach or shutdown"
        );
        assert!(
            !bin.path().join("client-launches").exists(),
            "zero Client starts before teardown"
        );
        result(&client, Method::SessionDetach { target }).await;
        client.close().await;
        detach_then_shutdown(&mut child).await;
        assert!(child.wait().await.unwrap().success());

        let requests = provider.request_bodies();
        assert_eq!(requests.len(), 2);
        let request: serde_json::Value = serde_json::from_str(&requests[0]).unwrap();
        let names: Vec<_> = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect();
        assert!(
            names.contains(&"echo"),
            "HTTP MCP tool not advertised: {names:?}"
        );
        assert!(
            !bin.path().join("client-launches").exists(),
            "zero Client starts"
        );
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
        let runtime = fixture.root.path().join("runtime");
        let controller = SessionController::open(&runtime).unwrap();
        let access = controller.acquire_session(session, None).await.unwrap();
        let store = SqliteConversationStore::open_existing(
            access.node.conversation_id.clone(),
            &access.database_path,
        )
        .unwrap();
        let page = store.read_events(None, 128).unwrap();
        assert!(page.events.len() < 128, "complete attempt journal");
        let commits: Vec<_> = page
            .events
            .iter()
            .filter_map(|entry| match &entry.event {
                RuntimeEvent::ToolMessageCommitted { message_id, .. } => {
                    Some((entry.sequence, message_id.clone()))
                }
                _ => None,
            })
            .collect();
        let [(commit_sequence, message_id)] = commits.as_slice() else {
            panic!("one tool commit: {commits:?}");
        };
        let terminals: Vec<_> = page
            .events
            .iter()
            .filter_map(|entry| {
                AttemptOutcome::from_terminal_event(&entry.event)
                    .map(|outcome| (entry.sequence, outcome))
            })
            .collect();
        let [(terminal_sequence, AttemptOutcome::Completed { .. })] = terminals.as_slice() else {
            panic!("one completed terminal: {terminals:?}");
        };
        assert!(commit_sequence < terminal_sequence);
        let MessageBlock::Tool(tool) = store
            .load_messages(std::slice::from_ref(message_id))
            .unwrap()
            .remove(0)
        else {
            panic!("canonical tool result");
        };
        let preview = tool.result.model_facing_projection();
        assert_eq!(
            tool.result.status,
            ToolExecutionStatus::Success,
            "{}",
            preview.as_text()
        );
        assert_eq!(std::fs::read_to_string(mcp_calls).unwrap(), "echo\n");
        assert_eq!(
            tool.result
                .truncation
                .as_ref()
                .and_then(|cut| cut.original_bytes),
            Some(original_bytes as u64)
        );
        assert!(matches!(
            tool.result.managed_output.as_deref(),
            Some(ManagedOutputContinuation::Unavailable { diagnostic })
                if diagnostic.contains("no eligible Execution Client save binding")
        ));
        assert!(!preview.as_text().contains("Read or Grep"));
        for entry in ignore::WalkBuilder::new(&runtime)
            .standard_filters(false)
            .build()
            .map(Result::unwrap)
        {
            let name = entry.file_name().to_string_lossy();
            assert!(
                !name.starts_with("r-") && !name.starts_with("e-"),
                "unexpected Spill: {entry:?}"
            );
        }
    }))
    .await;
}

/// A fresh staging directory on the *same filesystem as the staged
/// sources* so staging is a hard link — no write, so the staged inode
/// can never still be counted as written at execve. (A tmpfs copy of
/// the ~600 MB Server binary has a real transient ETXTBSY window under
/// parallel load: the kernel still counts the just-closed writer when
/// execve runs.) The returned guard owns the directory; it must outlive
/// the spawned children.
fn staged_bin_dir() -> tempfile::TempDir {
    let base = std::path::Path::new(env!("CARGO_BIN_EXE_rustx"))
        .parent()
        .unwrap();
    tempfile::tempdir_in(base).unwrap()
}

fn stage_binary(bin: &std::path::Path, name: &str, source: &std::path::Path) -> PathBuf {
    let destination = bin.join(name);
    // Hard link required: the staging directory intentionally shares the
    // source filesystem — a copy would reintroduce the exec/write race
    // `staged_bin_dir` exists to avoid.
    std::fs::hard_link(source, &destination).unwrap_or_else(|error| {
        panic!("staged directory must share the source filesystem: {error}")
    });
    destination
}

fn isolated_server_bin() -> (tempfile::TempDir, PathBuf) {
    let bin = staged_bin_dir();
    let rustx = stage_binary(
        bin.path(),
        "rustx",
        std::path::Path::new(env!("CARGO_BIN_EXE_rustx")),
    );
    (bin, rustx)
}

fn isolated_server_pair() -> (tempfile::TempDir, PathBuf) {
    let bin = staged_bin_dir();
    // The Client is only ever the artifact preparation staged beside this
    // harness — never a runtime build, a PATH lookup, or a fallback path.
    let client = std::env::current_exe()
        .unwrap()
        .with_file_name("rustx-execution-client");
    assert!(
        client.is_file(),
        "staged Execution Client is absent: {client:?}"
    );
    stage_binary(bin.path(), "rustx-execution-client", &client);
    let rustx = stage_binary(
        bin.path(),
        "rustx",
        std::path::Path::new(env!("CARGO_BIN_EXE_rustx")),
    );
    (bin, rustx)
}

fn local_exec_command(
    binary: &std::path::Path,
    f: &Fixture,
    session: &str,
    workspace: &std::path::Path,
) -> Command {
    let mut command = Command::new(binary);
    command
        .current_dir(f.root.path())
        .env_clear()
        .env("HOME", f.root.path().join("home"))
        .env("PATH", "/usr/bin:/bin")
        .args(["app-server", "--runtime-root"])
        .arg(f.root.path().join("runtime"))
        .args(["--listen", "stdio"])
        .arg("--local-execution-session")
        .arg(session)
        .arg("--local-execution-workspace")
        .arg(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

/// The remote composition: the App Server binds a loopback WebSocket
/// listener and publishes the bootstrap bundle for the operator —
/// never launches a Client itself.
fn remote_exec_command(
    binary: &std::path::Path,
    f: &Fixture,
    session: Option<&str>,
    bundle: &std::path::Path,
) -> Command {
    let mut command = Command::new(binary);
    if let Some(session) = session {
        command
            .arg("app-server")
            .arg("--execution-session")
            .arg(session);
    } else {
        command.arg("app-server");
    }
    command
        .current_dir(f.root.path())
        .env_clear()
        .env("HOME", f.root.path().join("home"))
        .env("PATH", "/usr/bin:/bin")
        .arg("--runtime-root")
        .arg(f.root.path().join("runtime"))
        .args(["--listen", "stdio"])
        .args(["--execution-ws-listen", "127.0.0.1:0"])
        .arg("--execution-bootstrap-out")
        .arg(bundle)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

/// The operator step of the remote composition: the staged real Client
/// started independently against the provisioned bundle — no
/// environment, sealed stdio, exactly the documented invocation.
fn staged_client_command(
    bundle: &std::path::Path,
    workspace: &std::path::Path,
) -> std::process::Command {
    let executable = std::env::current_exe()
        .unwrap()
        .with_file_name("rustx-execution-client");
    assert!(
        executable.is_file(),
        "staged Execution Client is absent: {executable:?}"
    );
    let mut command = std::process::Command::new(&executable);
    command
        .arg("--binding-file")
        .arg(bundle)
        .arg("--workspace")
        .arg(workspace)
        .current_dir("/")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Polls a Session-routed read until the background accept loop admits
/// the operator-spawned Client — bounded like the handshake budget.
async fn await_admission(client: &driver::Driver, session: &SessionId, path: &std::path::Path) {
    use rustx::app_server::protocol::*;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match rpc(
            client,
            10,
            Method::FsReadFile {
                session_id: session.clone(),
                path: path.display().to_string(),
                offset: 0,
                max_bytes: 16,
                expected_revision: None,
            },
        )
        .await
        {
            Response::Success(_) => return,
            Response::Failure(failure)
                if failure.error.data == Some(ErrorData::ExecutionUnavailable) =>
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "Client was not admitted within the bound"
                );
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            other @ Response::Failure(_) => panic!("admission probe: {other:?}"),
        }
    }
}

/// Grants exactly these environment roots in the fixture's authored
/// config — the immutable allowlist the App Server's file queries are
/// authorized against.
fn grant_environment_roots(f: &Fixture, roots: &[PathBuf]) {
    let authored = f.root.path().join("home/rustx/rustx.toml");
    let mut source: toml::Value =
        toml::from_str(&std::fs::read_to_string(&authored).unwrap()).unwrap();
    source.as_table_mut().unwrap().insert(
        "app_server".into(),
        toml::Value::try_from(serde_json::json!({ "environment_roots": roots })).unwrap(),
    );
    std::fs::write(&authored, toml::to_string_pretty(&source).unwrap()).unwrap();
}

#[tokio::test]
async fn local_execution_rejects_a_workspace_that_is_not_the_reserved_one() {
    bounded(async {
        let f = Fixture::new().await;
        // The sibling Client is absent, so a workspace error proves the
        // check fires before anything is spawned.
        let (_bin, rustx) = isolated_server_bin();
        // The fixture root is a normalized absolute path but is not the
        // Session's recorded workspace.
        let child = local_exec_command(&rustx, &f, f.sessions[0].as_str(), f.root.path())
            .spawn()
            .expect("spawn staged server");
        let output = child.wait_with_output().await.unwrap();
        assert!(!output.status.success(), "unexpected success");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("not the Session's reserved workspace"),
            "{stderr}"
        );
    })
    .await;
}

#[tokio::test]
async fn local_execution_fails_exactly_when_the_sibling_client_is_absent() {
    bounded(async {
        let f = Fixture::new().await;
        let (_bin, rustx) = isolated_server_bin();
        let workspace = std::fs::canonicalize(f.root.path().join("a")).unwrap();
        let child = local_exec_command(&rustx, &f, f.sessions[0].as_str(), &workspace)
            .spawn()
            .expect("spawn staged server");
        let output = child.wait_with_output().await.unwrap();
        assert!(!output.status.success(), "unexpected success");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("client executable is absent beside the server"),
            "{stderr}"
        );
    })
    .await;
}

#[tokio::test]
async fn local_execution_rejects_an_unknown_session() {
    bounded(async {
        let f = Fixture::new().await;
        let (_bin, rustx) = isolated_server_bin();
        let workspace = std::fs::canonicalize(f.root.path().join("a")).unwrap();
        let child = local_exec_command(
            &rustx,
            &f,
            "ses_00000000-0000-7000-8000-000000000001",
            &workspace,
        )
        .spawn()
        .expect("spawn staged server");
        let output = child.wait_with_output().await.unwrap();
        assert!(!output.status.success(), "unexpected success");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("local execution:"), "{stderr}");
        assert!(!stderr.contains("client executable is absent"), "{stderr}");
    })
    .await;
}

#[tokio::test]
async fn local_execution_arguments_must_be_paired() {
    bounded(async {
        let f = Fixture::new().await;
        let output = Command::new(env!("CARGO_BIN_EXE_rustx"))
            .current_dir(f.root.path())
            .env("HOME", f.root.path().join("home"))
            .args(["app-server", "--runtime-root"])
            .arg(f.root.path().join("runtime"))
            .args(["--listen", "stdio"])
            .arg("--local-execution-session")
            .arg(f.sessions[0].as_str())
            .output()
            .await
            .unwrap();
        assert!(!output.status.success(), "unpaired flag must fail");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("local-execution-workspace"), "{stderr}");
    })
    .await;
}

#[tokio::test]
async fn explicit_composition_launches_and_reaps_the_sibling_client() {
    bounded(async {
        use base64::Engine as _;
        use rustx::app_server::protocol::*;
        let f = Fixture::new().await;
        // Both catalog workspaces are configured roots for this composition.
        grant_environment_roots(
            &f,
            &[
                std::fs::canonicalize(f.root.path().join("a")).unwrap(),
                std::fs::canonicalize(f.root.path().join("b")).unwrap(),
            ],
        );

        // Same filename in both workspaces: only Session A may be served.
        let marker = b"only through the admitted client";
        std::fs::write(f.root.path().join("a/marker"), marker).unwrap();
        std::fs::write(f.root.path().join("b/marker"), b"decoy").unwrap();
        std::os::unix::fs::symlink("marker", f.root.path().join("a/link")).unwrap();

        let (_bin, rustx) = isolated_server_pair();
        let workspace = std::fs::canonicalize(f.root.path().join("a")).unwrap();
        let mut child = local_exec_command(&rustx, &f, f.sessions[0].as_str(), &workspace)
            .spawn()
            .expect("spawn staged server");
        let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        initialize_client(&client).await;
        // Readiness proves the Client connected and was admitted before
        // the transport went up — the launch is real, not deferred.

        let MethodResult::FileChunk { chunk } = result(
            &client,
            Method::FsReadFile {
                session_id: f.sessions[0].clone(),
                path: workspace.join("marker").display().to_string(),
                offset: 0,
                max_bytes: 64,
                expected_revision: None,
            },
        )
        .await
        else {
            panic!("fs/readFile through the admitted Client")
        };
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&chunk.data_base64)
                .unwrap(),
            marker
        );
        assert_eq!(chunk.next_offset, None);
        // The stamp is the source-descriptor truth of the fixture file.
        let expected = rustx_confined_fs::FileMetadata::of(
            &std::fs::File::open(f.root.path().join("a/marker")).unwrap(),
        )
        .unwrap();
        assert_eq!(chunk.metadata.revision, expected.revision);
        assert_eq!(chunk.metadata.identity, expected.identity);
        assert_eq!(chunk.metadata.size, expected.size);

        // Session B holds no endpoint: its decoy is never served.
        let Response::Failure(failure) = rpc(
            &client,
            10,
            Method::FsReadFile {
                session_id: f.sessions[1].clone(),
                path: std::fs::canonicalize(f.root.path().join("b"))
                    .unwrap()
                    .join("marker")
                    .display()
                    .to_string(),
                offset: 0,
                max_bytes: 64,
                expected_revision: None,
            },
        )
        .await
        else {
            panic!("Session B read must fail")
        };
        assert_eq!(failure.error.data, Some(ErrorData::ExecutionUnavailable));

        // An oversized range never leaves the parameter gate.
        let Response::Failure(failure) = rpc(
            &client,
            11,
            Method::FsReadFile {
                session_id: f.sessions[0].clone(),
                path: workspace.join("marker").display().to_string(),
                offset: 0,
                max_bytes: 65537,
                expected_revision: None,
            },
        )
        .await
        else {
            panic!("oversize read must fail")
        };
        assert_eq!(failure.error.data, Some(ErrorData::InvalidParams));

        // fs/getMetadata reports the leaf symlink itself — never
        // followed — with the Client's own source stamp.
        let MethodResult::FileMetadata { metadata } = result(
            &client,
            Method::FsGetMetadata {
                session_id: f.sessions[0].clone(),
                path: workspace.join("link").display().to_string(),
            },
        )
        .await
        else {
            panic!("fs/getMetadata through the admitted Client")
        };
        assert!(metadata.is_symlink && !metadata.is_file);
        let expected_link = std::fs::symlink_metadata(workspace.join("link")).unwrap();
        assert_eq!(metadata.size, expected_link.len());

        // fs/readDirectory pages the Session's root through the Client:
        // both names, sorted, the link reported as a symlink.
        let MethodResult::DirectoryPage { page } = result(
            &client,
            Method::FsReadDirectory {
                session_id: f.sessions[0].clone(),
                path: workspace.display().to_string(),
                offset: 0,
                limit: 16,
                expected_revision: None,
            },
        )
        .await
        else {
            panic!("fs/readDirectory through the admitted Client")
        };
        assert_eq!(
            page.entries
                .iter()
                .map(|e| e.file_name.as_str())
                .collect::<Vec<_>>(),
            ["link", "marker"]
        );
        assert_eq!(page.next_offset, None);
        assert!(page.entries[0].metadata.is_symlink);
        assert!(page.entries[1].metadata.is_file);

        // Session B's unbound queries — and its write — fail the same way
        // its read did. The unbound decoy is never a fallback target.
        for method in [
            Method::FsGetMetadata {
                session_id: f.sessions[1].clone(),
                path: std::fs::canonicalize(f.root.path().join("b"))
                    .unwrap()
                    .join("marker")
                    .display()
                    .to_string(),
            },
            Method::FsReadDirectory {
                session_id: f.sessions[1].clone(),
                path: std::fs::canonicalize(f.root.path().join("b"))
                    .unwrap()
                    .display()
                    .to_string(),
                offset: 0,
                limit: 16,
                expected_revision: None,
            },
            Method::FsWriteFile {
                session_id: f.sessions[1].clone(),
                path: std::fs::canonicalize(f.root.path().join("b"))
                    .unwrap()
                    .join("marker")
                    .display()
                    .to_string(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(b"planted"),
                expected_revision: None,
            },
        ] {
            let Response::Failure(failure) = rpc(&client, 12, method).await else {
                panic!("Session B query must fail")
            };
            assert_eq!(failure.error.data, Some(ErrorData::ExecutionUnavailable));
        }
        assert_eq!(
            std::fs::read(f.root.path().join("b/marker")).unwrap(),
            b"decoy"
        );
        client.close().await;
        terminate(&child);
        let output = child.wait_with_output().await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    })
    .await;
}

#[tokio::test]
async fn remote_execution_shuts_down_cleanly_without_an_admitted_client() {
    bounded(async {
        let fixture = Fixture::new().await;
        let (_bin, rustx) = isolated_server_bin();
        let bundle = fixture.root.path().join("private").join("bootstrap");
        let mut server = remote_exec_command(
            &rustx,
            &fixture,
            Some(fixture.sessions[0].as_str()),
            &bundle,
        )
        .spawn()
        .unwrap();
        let client = driver::jsonl(server.stdout.take().unwrap(), server.stdin.take().unwrap());
        initialize_client(&client).await;
        assert!(bundle.is_file());
        terminate(&server);
        let status = server.wait().await.unwrap();
        assert!(status.success(), "{status}");
        assert!(!bundle.exists());
        client.close().await;
    })
    .await;
}

/// Remote-mode product proof: `rustx app-server --execution-*` publishes a
/// `0600` WebSocket bootstrap, an operator-spawned staged Client connects
/// through a genuine RFC 6455 upgrade — never a UDS stand-in — and
/// Session-routed `fs` dispatch crosses the WebSocket channel while a
/// second unbound Session is refused exactly as on the Unix transport.
#[tokio::test]
async fn remote_execution_serves_session_reads_over_websocket() {
    bounded(async {
        use base64::Engine as _;
        use rustx::app_server::protocol::*;
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new().await;
        let workspace = std::fs::canonicalize(f.root.path().join("a")).unwrap();
        grant_environment_roots(&f, std::slice::from_ref(&workspace));
        let marker = "remote ws payload — déjà vu".as_bytes();
        std::fs::write(workspace.join("marker"), marker).unwrap();

        let (_bin, rustx) = isolated_server_bin();
        let private = f.root.path().join("private");
        std::fs::create_dir(&private).unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
        let bundle = private.join("bootstrap");
        let mut server = remote_exec_command(&rustx, &f, Some(f.sessions[0].as_str()), &bundle)
            .spawn()
            .expect("spawn staged server");
        let client = driver::jsonl(server.stdout.take().unwrap(), server.stdin.take().unwrap());
        initialize_client(&client).await;
        // Readiness implies the reservation published: the bundle is the
        // `0600` create_new file the operator hands to the Client.
        let mode = std::fs::metadata(&bundle)
            .unwrap_or_else(|_| panic!("bootstrap bundle was not published"))
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "bootstrap must be owner-only");

        let execution = staged_client_command(&bundle, &workspace).spawn().unwrap();
        let mut execution = ReapChild(execution);

        // Admission runs in the background accept loop while management
        // already serves: the first Session-routed read polls until the
        // admitted Client answers, bounded like the handshake budget.
        await_admission(&client, &f.sessions[0], &workspace.join("marker")).await;
        let MethodResult::FileChunk { chunk } = result(
            &client,
            Method::FsReadFile {
                session_id: f.sessions[0].clone(),
                path: workspace.join("marker").display().to_string(),
                offset: 0,
                max_bytes: 256,
                expected_revision: None,
            },
        )
        .await
        else {
            panic!("fs/readFile through the WS-admitted Client")
        };
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&chunk.data_base64)
                .unwrap(),
            marker
        );
        assert_eq!(chunk.next_offset, None);

        // The same routed metadata and conditional write cross the WS
        // channel — identical semantics to the Unix transport.
        let MethodResult::FileMetadata { metadata } = result(
            &client,
            Method::FsGetMetadata {
                session_id: f.sessions[0].clone(),
                path: workspace.join("marker").display().to_string(),
            },
        )
        .await
        else {
            panic!("fs/getMetadata through the WS-admitted Client")
        };
        assert!(metadata.is_file && !metadata.is_symlink);

        let MethodResult::FileMetadata { metadata: written } = result(
            &client,
            Method::FsWriteFile {
                session_id: f.sessions[0].clone(),
                path: workspace.join("written").display().to_string(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(b"ws written"),
                expected_revision: None,
            },
        )
        .await
        else {
            panic!("fs/writeFile through the WS-admitted Client")
        };
        assert_ne!(written.revision, "");
        assert_eq!(
            std::fs::read(workspace.join("written")).unwrap(),
            b"ws written"
        );

        let mut argv = rustx::app_server::command::ExecParams {
            session_id: f.sessions[0].clone(),
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                r#"printf '%s:<%s>' "$#" "$1""#.into(),
                "argv-proof".into(),
                String::new(),
            ],
            cwd: workspace.display().to_string(),
            env: std::collections::BTreeMap::new(),
            process_id: None,
            tty: false,
            stream_stdin: false,
            stream_stdout_stderr: false,
            output_bytes_cap: None,
            timeout_ms: None,
            size: None,
        };
        let MethodResult::CommandExec { result: command } =
            result(&client, Method::CommandExec(argv.clone())).await
        else {
            panic!("empty argument must reach the real Client");
        };
        assert_eq!(command.exit_code, Some(0));
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(command.stdout_base64)
                .unwrap(),
            b"1:<>"
        );
        assert_eq!(command.stderr_base64, "");
        for invalid in [
            vec![String::new()],
            vec!["/usr/bin/printf".into(), "x\0y".into()],
        ] {
            argv.command = invalid;
            let Response::Failure(failure) =
                rpc(&client, 52, Method::CommandExec(argv.clone())).await
            else {
                panic!("invalid argv must be refused");
            };
            assert_eq!(failure.error.data, Some(ErrorData::InvalidParams));
        }

        // Session B holds no endpoint — its routed read is refused
        // exactly as on the Unix transport.
        let Response::Failure(failure) = rpc(
            &client,
            11,
            Method::FsReadFile {
                session_id: f.sessions[1].clone(),
                path: std::fs::canonicalize(f.root.path().join("b"))
                    .unwrap()
                    .join("marker")
                    .display()
                    .to_string(),
                offset: 0,
                max_bytes: 64,
                expected_revision: None,
            },
        )
        .await
        else {
            panic!("Session B read must fail")
        };
        assert_eq!(failure.error.data, Some(ErrorData::ExecutionUnavailable));
        client.close().await;
        terminate(&server);
        let output = server.wait_with_output().await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Orderly server teardown sends the WebSocket Close frame — the
        // Client observes a clean EOF and exits successfully on its own.
        let bound = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let status = loop {
            if let Some(status) = execution.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                std::time::Instant::now() < bound,
                "Client did not exit on channel EOF"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(status.success(), "gracefully closed Client must exit 0");
    })
    .await;
}

async fn assert_seeded_environment_tree(
    client: &driver::Driver,
    session: &SessionId,
    workspace: &std::path::Path,
) {
    use rustx::app_server::protocol::*;
    const CONTENT: &[u8] = b"one\r\ntwo\x00\xff\n";
    let path = |relative: &str| workspace.join(relative).display().to_string();
    std::fs::write(path("tree-source"), CONTENT).unwrap();
    let copy = |destination: &str| Method::FsCopy {
        session_id: session.clone(),
        source_path: path("tree-source"),
        destination_path: path(destination),
        recursive: false,
    };
    result(
        client,
        Method::FsCreateDirectory {
            session_id: session.clone(),
            path: path("tree-copy/nested"),
            recursive: true,
        },
    )
    .await;
    result(client, copy("tree-copy/nested/文本")).await;
    assert_eq!(
        std::fs::read(path("tree-copy/nested/文本")).unwrap(),
        CONTENT
    );
    let MethodResult::DirectoryPage { page } = result(
        client,
        Method::FsReadDirectory {
            session_id: session.clone(),
            path: path("tree-copy/nested"),
            offset: 0,
            limit: 1,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("list copied tree")
    };
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].file_name, "文本");
    assert_eq!(page.entries[0].metadata.size, CONTENT.len() as u64);
    assert_eq!(page.next_offset, None);
    let control_name = "reports\t2026";
    let parent = format!("tree-copy/{control_name}");
    result(
        client,
        Method::FsCreateDirectory {
            session_id: session.clone(),
            path: path(&parent),
            recursive: false,
        },
    )
    .await;
    let MethodResult::DirectoryPage { page } = result(
        client,
        Method::FsReadDirectory {
            session_id: session.clone(),
            path: path("tree-copy"),
            offset: 0,
            limit: 16,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("list names with literal controls");
    };
    assert_eq!(
        page.entries
            .iter()
            .map(|entry| entry.file_name.as_str())
            .collect::<Vec<_>>(),
        ["nested", control_name]
    );
    let MethodResult::FileMetadata { metadata } = result(
        client,
        Method::FsGetMetadata {
            session_id: session.clone(),
            path: path(&parent),
        },
    )
    .await
    else {
        panic!("stat the directory with a tab");
    };
    assert!(metadata.is_directory);
    let MethodResult::DirectoryPage { page } = result(
        client,
        Method::FsReadDirectory {
            session_id: session.clone(),
            path: path(&parent),
            offset: 0,
            limit: 16,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("open the directory with a tab");
    };
    assert_eq!(page.entries, []);
    let Response::Failure(error) = rpc(client, 48, copy("../escape")).await else {
        panic!("copy must refuse a non-normal Workspace path")
    };
    assert_eq!(error.error.data, Some(ErrorData::EnvironmentUnauthorized));
    assert!(!workspace.with_file_name("escape").exists());
    result(
        client,
        Method::FsRemove {
            session_id: session.clone(),
            path: path("tree-copy"),
            recursive: true,
            force: false,
        },
    )
    .await;
    assert!(!workspace.join("tree-copy").exists());
    assert_eq!(std::fs::read(path("tree-source")).unwrap(), CONTENT);
}

#[tokio::test]
async fn execution_enrollment_precedes_client_and_workspace_configuration_readiness() {
    bounded(async {
        use rustx::app_server::protocol::*;
        use rustx_execution_protocol::Bootstrap;
        use rustx_execution_protocol::transport::read_record;
        use std::os::unix::fs::PermissionsExt as _;

        let f = Fixture::new().await;
        let workspace = f.root.path().join("a");
        grant_environment_roots(&f, std::slice::from_ref(&workspace));
        let project = workspace.join("rustx.toml");
        std::fs::write(&project, b"[agent]\n").unwrap();
        std::fs::write(workspace.join("marker"), b"authorized").unwrap();
        let (_bin, rustx) = isolated_server_bin();
        let private = f.root.path().join("bootstrap-private");
        std::fs::create_dir(&private).unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
        let bundle = private.join("binding");
        let mut server = remote_exec_command(&rustx, &f, None, &bundle)
            .env("TEST_KEY", "fixture")
            .arg("--execution-workspace")
            .arg(&workspace)
            .spawn()
            .unwrap();
        let client = driver::jsonl(server.stdout.take().unwrap(), server.stdin.take().unwrap());
        initialize_client(&client).await;
        let bootstrap: Bootstrap = read_record(&mut std::fs::File::open(&bundle).unwrap()).unwrap();
        let id = SessionId::parse(bootstrap.session.as_str()).unwrap();
        assert!(!f.sessions.contains(&id));
        assert_eq!(bootstrap.workspace, workspace);
        assert!(matches!(
            result(&client, Method::SessionRead { session_id: id.clone() }).await,
            MethodResult::Session { session } if session.id == id
        ));
        let browse = || Method::FsReadDirectory {
            session_id: id.clone(),
            path: workspace.display().to_string(),
            offset: 0,
            limit: 16,
            expected_revision: None,
        };
        let Response::Failure(before) = rpc(&client, 18, browse()).await else {
            panic!("browse before authenticated Client")
        };
        assert_eq!(before.error.data, Some(ErrorData::ExecutionUnavailable));

        let thread_id = app_server_conformance::thread_id(&client, id.clone()).await;
        let Response::Failure(before_attach) = rpc(
            &client,
            42,
            Method::SessionAttach {
                thread_id: thread_id.clone(),
            },
        )
        .await
        else {
            panic!("configuration before authenticated Client");
        };
        assert!(
            before_attach
                .error
                .message
                .contains("no client is admitted"),
            "{before_attach:?}"
        );
        assert_eq!(diagnostics(&client).await.loaded, 0);
        std::fs::write(project, b"[broken").unwrap();
        let _execution = ReapChild(staged_client_command(&bundle, &workspace).spawn().unwrap());
        await_admission(&client, &id, &workspace.join("marker")).await;
        assert!(matches!(
            result(&client, browse()).await,
            MethodResult::DirectoryPage { page }
                if page.entries.iter().any(|entry| entry.file_name == "marker")
        ));
        assert!(matches!(
            rpc(&client, 44, Method::SessionAttach { thread_id }).await,
            Response::Failure(_)
        ));
        assert!(matches!(
            result(&client, Method::SessionRead { session_id: id }).await,
            MethodResult::Session { .. }
        ));
        assert_eq!(diagnostics(&client).await.loaded, 0);
        client.close().await;
        terminate(&server);
        let output = server.wait_with_output().await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    })
    .await;
}

/// Identical seed content for the parity transcript — the same bytes,
/// names, and link under both transports so the transcripts may differ
/// only by transport, never by seed.
fn seed_workspace(workspace: &std::path::Path, decoy_dir: &std::path::Path) {
    std::fs::write(workspace.join("marker"), "såme payload — déjà vu ☃\n").unwrap();
    std::fs::write(
        workspace.join("s p a c e ü.txt"),
        "escaped bytes — こんにちは\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("marker", workspace.join("link")).unwrap();
    std::fs::write(decoy_dir.join("marker"), b"decoy").unwrap();
}

/// The identical seeded operation sequence both transports must serve
/// byte-for-byte identical: UTF-8 and escaped-name reads, symlink
/// metadata, the directory page, the missing-file typed failure, a
/// conditional write that commits then conflicts, tree mutations, and the
/// unbound Session's refused read and write. The transcript records only
/// deterministic outcomes — revisions and identities are proven
/// non-empty but never compared, since they are per-write identities.
async fn seeded_operations(
    client: &driver::Driver,
    session_a: &SessionId,
    session_b: &SessionId,
    workspace_a: &std::path::Path,
    decoy_marker: &std::path::Path,
) -> Vec<String> {
    use base64::Engine as _;
    use rustx::app_server::protocol::*;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut transcript = Vec::new();
    let read_at = |path: &std::path::Path| Method::FsReadFile {
        session_id: session_a.clone(),
        path: path.display().to_string(),
        offset: 0,
        max_bytes: 256,
        expected_revision: None,
    };

    // UTF-8 content read with a proven revision.
    let MethodResult::FileChunk { chunk } =
        result(client, read_at(&workspace_a.join("marker"))).await
    else {
        panic!("seeded marker read")
    };
    let bytes = b64.decode(&chunk.data_base64).unwrap();
    transcript.push(format!(
        "read:marker:{}:{}:{}:{}",
        String::from_utf8(bytes).unwrap(),
        chunk.next_offset.is_none(),
        !chunk.metadata.revision.is_empty(),
        !chunk.metadata.identity.is_empty()
    ));

    // Escaped-name UTF-8 read.
    let MethodResult::FileChunk { chunk } =
        result(client, read_at(&workspace_a.join("s p a c e ü.txt"))).await
    else {
        panic!("seeded escaped-name read")
    };
    transcript.push(format!(
        "read:escaped:{}",
        String::from_utf8(b64.decode(&chunk.data_base64).unwrap()).unwrap()
    ));

    // Symlink metadata is stamped exactly.
    let MethodResult::FileMetadata { metadata } = result(
        client,
        Method::FsGetMetadata {
            session_id: session_a.clone(),
            path: workspace_a.join("link").display().to_string(),
        },
    )
    .await
    else {
        panic!("seeded symlink metadata")
    };
    transcript.push(format!(
        "meta:link:{}:{}:{}",
        metadata.is_symlink, metadata.is_file, metadata.size
    ));

    // The directory page names both seeded names in the same order.
    let MethodResult::DirectoryPage { page } = result(
        client,
        Method::FsReadDirectory {
            session_id: session_a.clone(),
            path: workspace_a.display().to_string(),
            offset: 0,
            limit: 16,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("seeded directory page")
    };
    transcript.push(format!(
        "dir:{}:{}",
        page.entries
            .iter()
            .map(|entry| format!(
                "{}:{}:{}",
                entry.file_name, entry.metadata.is_file, entry.metadata.is_symlink
            ))
            .collect::<Vec<_>>()
            .join("|"),
        page.next_offset.is_none()
    ));

    // A missing file is the same typed failure on both transports.
    let Response::Failure(failure) = rpc(
        client,
        21,
        Method::FsReadFile {
            session_id: session_a.clone(),
            path: workspace_a.join("absent").display().to_string(),
            offset: 0,
            max_bytes: 64,
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("the missing file must fail")
    };
    assert_eq!(
        failure.error.data,
        Some(ErrorData::EnvironmentResourceNotFound)
    );
    transcript.push("absent:EnvironmentResourceNotFound".to_string());

    // A conditional write commits, a stale revision conflicts typed,
    // and the committed revision chains the next write.
    let MethodResult::FileMetadata { metadata: first } = result(
        client,
        Method::FsWriteFile {
            session_id: session_a.clone(),
            path: workspace_a.join("written").display().to_string(),
            data_base64: b64.encode("written — 1\n"),
            expected_revision: None,
        },
    )
    .await
    else {
        panic!("seeded write")
    };
    assert_ne!(first.revision, "");
    transcript.push("write:created".to_string());

    // The committed revision chains the next write.
    let MethodResult::FileMetadata { metadata: second } = result(
        client,
        Method::FsWriteFile {
            session_id: session_a.clone(),
            path: workspace_a.join("written").display().to_string(),
            data_base64: b64.encode("written — 2\n"),
            expected_revision: Some(first.revision.clone()),
        },
    )
    .await
    else {
        panic!("the chained write must commit")
    };
    assert_ne!(second.revision, "");
    assert_ne!(second.revision, first.revision);
    assert_eq!(
        std::fs::read(workspace_a.join("written")).unwrap(),
        "written — 2\n".as_bytes()
    );
    transcript.push("write:chained".to_string());

    // A genuinely stale revision — valid format, superseded value —
    // is the same typed conflict on both transports.
    let Response::Failure(failure) = rpc(
        client,
        22,
        Method::FsWriteFile {
            session_id: session_a.clone(),
            path: workspace_a.join("written").display().to_string(),
            data_base64: b64.encode("stale\n"),
            expected_revision: Some(first.revision.clone()),
        },
    )
    .await
    else {
        panic!("the stale write must fail")
    };
    transcript.push(format!("write:stale:{:?}", failure.error.data));
    assert_eq!(
        std::fs::read(workspace_a.join("written")).unwrap(),
        "written — 2\n".as_bytes(),
        "the conflicted write never landed"
    );

    // The unbound Session's read and write are refused identically.
    for (id, call) in [
        (
            23,
            Method::FsReadFile {
                session_id: session_b.clone(),
                path: decoy_marker.display().to_string(),
                offset: 0,
                max_bytes: 64,
                expected_revision: None,
            },
        ),
        (
            24,
            Method::FsWriteFile {
                session_id: session_b.clone(),
                path: decoy_marker.display().to_string(),
                data_base64: b64.encode("planted"),
                expected_revision: None,
            },
        ),
    ] {
        let Response::Failure(failure) = rpc(client, id, call).await else {
            panic!("the unbound Session call must fail")
        };
        assert_eq!(failure.error.data, Some(ErrorData::ExecutionUnavailable));
    }
    assert_eq!(std::fs::read(decoy_marker).unwrap(), b"decoy");
    transcript.push("unbound:ExecutionUnavailable".to_string());
    assert_seeded_environment_tree(client, session_a, workspace_a).await;

    transcript
}

/// Phase one of the parity proof: the seeded sequence over the local
/// Unix sibling composition — readiness itself proves the sibling was
/// launched and admitted before the transport served.
async fn seeded_transcript_over_unix() -> Vec<String> {
    let f = Fixture::new().await;
    let workspace = std::fs::canonicalize(f.root.path().join("a")).unwrap();
    grant_environment_roots(&f, std::slice::from_ref(&workspace));
    seed_workspace(&workspace, &f.root.path().join("b"));

    let (_bin, rustx) = isolated_server_pair();
    let mut server = local_exec_command(&rustx, &f, f.sessions[0].as_str(), &workspace)
        .spawn()
        .expect("spawn staged server");
    let client = driver::jsonl(server.stdout.take().unwrap(), server.stdin.take().unwrap());
    initialize_client(&client).await;
    let transcript = seeded_operations(
        &client,
        &f.sessions[0],
        &f.sessions[1],
        &workspace,
        &f.root.path().join("b/marker"),
    )
    .await;
    client.close().await;
    terminate(&server);
    let output = server.wait_with_output().await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    transcript
}

/// Phase two: the identical sequence over the remote WebSocket
/// composition — operator-spawned Client admitted through the genuine
/// RFC 6455 upgrade.
async fn seeded_transcript_over_websocket() -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    let workspace = std::fs::canonicalize(f.root.path().join("a")).unwrap();
    grant_environment_roots(&f, std::slice::from_ref(&workspace));
    seed_workspace(&workspace, &f.root.path().join("b"));

    let (_bin, rustx) = isolated_server_bin();
    let private = f.root.path().join("private");
    std::fs::create_dir(&private).unwrap();
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
    let bundle = private.join("bootstrap");
    let mut server = remote_exec_command(&rustx, &f, Some(f.sessions[0].as_str()), &bundle)
        .spawn()
        .expect("spawn staged server");
    let client = driver::jsonl(server.stdout.take().unwrap(), server.stdin.take().unwrap());
    initialize_client(&client).await;
    let execution = staged_client_command(&bundle, &workspace).spawn().unwrap();
    let mut execution = ReapChild(execution);
    await_admission(&client, &f.sessions[0], &workspace.join("marker")).await;
    let transcript = seeded_operations(
        &client,
        &f.sessions[0],
        &f.sessions[1],
        &workspace,
        &f.root.path().join("b/marker"),
    )
    .await;
    client.close().await;
    terminate(&server);
    let output = server.wait_with_output().await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bound = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(status) = execution.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < bound,
            "Client did not exit on channel EOF"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(
        status.success(),
        "gracefully closed Client must exit 0, got {status}"
    );
    transcript
}

/// Transport parity proof: the identical seeded operation sequence is
/// served byte-for-byte identical — same bytes, same ordering, same
/// typed failures — over the local Unix composition and the remote
/// WebSocket composition.
#[tokio::test]
async fn websocket_and_unix_transports_serve_identical_seeded_operations() {
    bounded(async {
        let unix = seeded_transcript_over_unix().await;
        let websocket = seeded_transcript_over_websocket().await;
        assert_eq!(unix, websocket);
    })
    .await;
}

#[cfg(target_os = "linux")]
async fn effective_tool_deadline(
    client: &driver::Driver,
    target: rustx::app_server::protocol::AttachmentTarget,
) -> u64 {
    use rustx::app_server::protocol::*;
    use rustx::local_runtime::configuration::effective_observation::EffectiveFieldName;
    let MethodResult::EffectiveConfiguration { projection } =
        result(client, Method::ConfigurationGet { target }).await
    else {
        panic!("the bound Session has an effective configuration");
    };
    if let Some(configuration) = projection.inline {
        return configuration.tool_deadline.hard_deadline_ms;
    }
    let policy = projection
        .fields
        .into_iter()
        .find(|field| field.name == EffectiveFieldName::ToolDeadline)
        .and_then(|field| field.value)
        .expect("the bounded Tool Deadline field");
    policy["hardDeadlineMs"]
        .as_str()
        .unwrap_or_else(|| panic!("exact deadline in {policy}"))
        .parse()
        .expect("decimal deadline")
}

/// Product-entrance proof of the Client's private mount view: the real
/// two-process composition serves Session A's workspace from the
/// Client's `bwrap` bind mount, never the Server's view of the same
/// absolute path. Linux-only and opt-in — `bwrap` is a hard requirement
/// of the proof, and the wrapping launcher exists only inside this
/// fixture's private bin directory; no production override exists.
#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires Linux mount namespace; run test-support/prove_execution_mount_view.sh"]
async fn explicit_composition_reads_from_client_mount_namespace() {
    bounded(async {
        use app_server_conformance::AppServerConformanceDriver as _;
        use base64::Engine as _;
        use rustx::app_server::protocol::*;
        use rustx::local_runtime::configuration::application::{ApplyUnit, UnitApplication};
        use std::os::unix::fs::PermissionsExt;
        assert!(
            std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some(),
            "opt-in proof only: run test-support/prove_execution_mount_view.sh"
        );
        assert!(
            std::fs::metadata("/usr/bin/bwrap")
                .is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0),
            "the mount-view proof requires an executable /usr/bin/bwrap"
        );

        let f = Fixture::new().await;
        let workspace = std::fs::canonicalize(f.root.path().join("a")).unwrap();
        grant_environment_roots(&f, std::slice::from_ref(&workspace));

        // The Server's own view of the workspace holds only a decoy the
        // Client's mount never sees; the marker exists only in the
        // physical directory the launcher bind-mounts over the workspace
        // path inside the Client's mount namespace.
        let decoy = workspace.join("server-only");
        std::fs::write(&decoy, b"server-side only").unwrap();
        let server_document = workspace.join("rustx.toml");
        std::fs::write(&server_document, b"[server-side-invalid").unwrap();
        let (bin_dir, rustx) = isolated_server_bin();
        let bin = bin_dir.path();
        // The staged sibling is the only trusted source for the real
        // Client — never a runtime build, PATH lookup, or fallback.
        let staged = std::env::current_exe()
            .unwrap()
            .with_file_name("rustx-execution-client");
        assert!(
            staged.is_file(),
            "staged Execution Client is absent: {staged:?}"
        );
        stage_binary(bin, "client-real", &staged);
        // The Client's own sibling supervisor, staged beside it exactly as
        // the production executable boundary resolves it.
        let supervisor = std::env::current_exe()
            .unwrap()
            .with_file_name("bash-supervisor");
        assert!(
            supervisor.is_file(),
            "staged bash-supervisor is absent: {supervisor:?}"
        );
        stage_binary(bin, "bash-supervisor", &supervisor);
        let physical = bin.join("physical");
        std::fs::create_dir(&physical).unwrap();
        let marker = b"mount-view proof marker";
        std::fs::write(physical.join("marker"), marker).unwrap();
        // The Client's writable scratch: a private tmpfs at its own path,
        // outside the workspace and the bootstrap — the host /tmp and the
        // bootstrap socket stay exactly as the Server sees them. A tmpfs is
        // required: the owner-only snapshot check needs POSIX permissions.
        let scratch = bin.join("tmp");
        std::fs::create_dir(&scratch).unwrap();
        // The test-only launcher: `$4` is the Server's explicit trusted
        // `--workspace` value; the real Client still rechecks it byte for
        // byte against the bootstrap record.
        let launcher = bin.join("rustx-execution-client");
        std::fs::write(
            &launcher,
            "#!/bin/sh\nset -eu\nbin=${0%/*}\nexport TMPDIR=\"$bin/tmp\"\nexec /usr/bin/bwrap --die-with-parent --ro-bind / / --dev /dev --bind \"$bin/physical\" \"$4\" --tmpfs \"$bin/tmp\" -- \"$bin/client-real\" \"$@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Before the exchange the Server's own view has no marker.
        assert_eq!(
            std::fs::metadata(workspace.join("marker"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );

        let mut child = local_exec_command(&rustx, &f, f.sessions[0].as_str(), &workspace)
            .env("TEST_KEY", "fixture")
            .spawn()
            .expect("spawn staged server");
        let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        initialize_client(&client).await;

        // fs/readFile serves the marker through the admitted Client's
        // mount view — bytes the Server's filesystem cannot see.
        let MethodResult::FileChunk { chunk } = result(
            &client,
            Method::FsReadFile {
                session_id: f.sessions[0].clone(),
                path: workspace.join("marker").display().to_string(),
                offset: 0,
                max_bytes: 64,
                expected_revision: None,
            },
        )
        .await
        else {
            panic!("fs/readFile through the Client's mount view")
        };
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&chunk.data_base64)
                .unwrap(),
            marker
        );
        // The stamp is the physical source's own — ro-bind preserves the
        // same dev:ino identity the Server would stamp.
        let expected = rustx_confined_fs::FileMetadata::of(
            &std::fs::File::open(physical.join("marker")).unwrap(),
        )
        .unwrap();
        assert_eq!(chunk.metadata.revision, expected.revision);
        assert_eq!(chunk.metadata.identity, expected.identity);
        assert_eq!(chunk.metadata.size, expected.size);

        let MethodResult::FileMetadata { metadata } = result(
            &client,
            Method::FsGetMetadata {
                session_id: f.sessions[0].clone(),
                path: workspace.join("marker").display().to_string(),
            },
        )
        .await
        else {
            panic!("fs/getMetadata through the Client's mount view")
        };
        assert!(metadata.is_file);
        assert_eq!(metadata.revision, expected.revision);
        assert_eq!(metadata.identity, expected.identity);

        // The listing is the Client's view: the marker, never the
        // Server-side decoy.
        let MethodResult::DirectoryPage { page } = result(
            &client,
            Method::FsReadDirectory {
                session_id: f.sessions[0].clone(),
                path: workspace.display().to_string(),
                offset: 0,
                limit: 16,
                expected_revision: None,
            },
        )
        .await
        else {
            panic!("fs/readDirectory through the Client's mount view")
        };
        assert_eq!(
            page.entries
                .iter()
                .map(|entry| entry.file_name.as_str())
                .collect::<Vec<_>>(),
            ["marker"]
        );

        // The decoy is absent in the Client's view: NotFound, and no
        // Server-local fallback can serve it either.
        let Response::Failure(failure) = rpc(
            &client,
            11,
            Method::FsReadFile {
                session_id: f.sessions[0].clone(),
                path: decoy.display().to_string(),
                offset: 0,
                max_bytes: 64,
                expected_revision: None,
            },
        )
        .await
        else {
            panic!("the Server-side decoy must stay unreachable")
        };
        assert_eq!(
            failure.error.data,
            Some(ErrorData::EnvironmentResourceNotFound)
        );

        // The physical overlay is the only writable mount: fs/writeFile
        // with the expected revision publishes new marker bytes inside
        // the Client's mount view — the Server's view never gains one.
        let rewritten = b"mount-view proof rewritten";
        let MethodResult::FileMetadata { metadata: written } = result(
            &client,
            Method::FsWriteFile {
                session_id: f.sessions[0].clone(),
                path: workspace.join("marker").display().to_string(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(rewritten),
                expected_revision: Some(expected.revision.clone()),
            },
        )
        .await
        else {
            panic!("fs/writeFile through the Client's mount view")
        };
        assert_eq!(std::fs::read(physical.join("marker")).unwrap(), rewritten);
        assert_eq!(
            std::fs::metadata(workspace.join("marker"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
        // The returned stamp is the Client's published descriptor: the
        // replace allocated a new inode, so the identity is new too —
        // it must describe the file now at the physical path exactly.
        let published = rustx_confined_fs::FileMetadata::of(
            &std::fs::File::open(physical.join("marker")).unwrap(),
        )
        .unwrap();
        assert_eq!(written.identity, published.identity);
        assert_eq!(written.revision, published.revision);
        assert_ne!(written.revision, expected.revision);
        let MethodResult::FileChunk { chunk } = result(
            &client,
            Method::FsReadFile {
                session_id: f.sessions[0].clone(),
                path: workspace.join("marker").display().to_string(),
                offset: 0,
                max_bytes: 64,
                expected_revision: Some(written.revision.clone()),
            },
        )
        .await
        else {
            panic!("read-after-write through the Client's mount view")
        };
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&chunk.data_base64)
                .unwrap(),
            rewritten
        );

        // A Session that never bound a Client cannot write at all — the
        // Server-side decoy is not a fallback target.
        let Response::Failure(failure) = rpc(
            &client,
            12,
            Method::FsWriteFile {
                session_id: SessionId::new("ses_00000000-0000-7000-8000-000000000099"),
                path: decoy.display().to_string(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(b"planted"),
                expected_revision: None,
            },
        )
        .await
        else {
            panic!("an unbound Session's write must fail")
        };
        assert_eq!(
            failure.error.data,
            Some(ErrorData::ExecutionUnavailable)
        );
        assert_eq!(std::fs::read(&decoy).unwrap(), b"server-side only");

        std::fs::write(
            physical.join("rustx.toml"),
            b"[tool_deadline_policy]\nhard_deadline_ms = 34567\n",
        ).unwrap();
        let target = attach(&client, f.sessions[0].clone(), 25).await;
        assert_eq!(effective_tool_deadline(&client, target.clone()).await, 34_567);
        std::fs::write(
            physical.join("rustx.toml"),
            b"[tool_deadline_policy]\nhard_deadline_ms = 45678\n",
        ).unwrap();
        assert!(matches!(
            result(
                &client,
                Method::ConfigurationReconcile {
                    target: rustx::local_runtime::configuration::settings::SourceTarget::Workspace {
                        directory: workspace.clone(),
                    },
                },
            )
            .await,
            MethodResult::ConfigurationReceipt { .. }
        ));
        assert!(matches!(
            result(
                &client,
                Method::SessionConfiguration { session_id: f.sessions[0].clone() },
            )
            .await,
            MethodResult::SessionConfiguration { projection: Some(projection) }
                if projection.inline.as_ref().is_some_and(|application| application.desired.input_revision.is_some())
        ));
        loop {
            let notification = client.next_notification().await;
            if let NotificationMethod::ConfigurationChanged { application } = notification.notification
                && application.scope == target.session_id.to_string()
            {
                let application = application.inline.expect("bounded Session application");
                match &application.units[&ApplyUnit::ExecutionPolicy] {
                    UnitApplication::Preparing | UnitApplication::Ready => {}
                    UnitApplication::Applied => break,
                    outcome => panic!("the Client's deadline policy did not settle: {outcome:?}"),
                }
            }
        }
        assert_eq!(effective_tool_deadline(&client, target).await, 45_678);
        assert_eq!(std::fs::read(server_document).unwrap(), b"[server-side-invalid");

        // After the exchanges the Server's view is untouched: still no
        // marker, the decoy intact.
        assert_eq!(
            std::fs::metadata(workspace.join("marker"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(std::fs::read(&decoy).unwrap(), b"server-side only");

        // A foreground `command/exec` for the bound Session runs inside the
        // Client's mount view: it reads the rewritten marker the Server can
        // never see and writes a result only the physical directory holds.
        let MethodResult::CommandExec { result: exec } = result(
            &client,
            Method::CommandExec(rustx::app_server::command::ExecParams {
                session_id: f.sessions[0].clone(),
                command: vec![
                    "/bin/bash".into(),
                    "-c".into(),
                    "cat marker; printf client > process-marker".into(),
                ],
                cwd: workspace.display().to_string(),
                env: std::collections::BTreeMap::new(),
                process_id: None,
                tty: false,
                stream_stdin: false,
                stream_stdout_stderr: false,
                output_bytes_cap: None,
                timeout_ms: None,
                size: None,
            }),
        )
        .await
        else {
            panic!("command/exec through the Client's mount view")
        };
        let decode = |text: &str| {
            String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(text).unwrap())
                .into_owned()
        };
        assert_eq!(
            exec.exit_code,
            Some(0),
            "status {:?}; stdout {:?}; stderr {:?}",
            exec.status,
            decode(&exec.stdout_base64),
            decode(&exec.stderr_base64)
        );
        assert_eq!(
            exec.status,
            rustx::app_server::command::ExecStatus::Completed
        );
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&exec.stdout_base64)
                .unwrap(),
            rewritten
        );
        assert_eq!(
            std::fs::read(physical.join("process-marker")).unwrap(),
            b"client"
        );
        assert_eq!(
            std::fs::metadata(workspace.join("process-marker"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(std::fs::read(&decoy).unwrap(), b"server-side only");

        // A Session that never bound a Client cannot run a command either.
        let Response::Failure(failure) = rpc(
            &client,
            13,
            Method::CommandExec(rustx::app_server::command::ExecParams {
                session_id: SessionId::new("ses_00000000-0000-7000-8000-000000000099"),
                command: vec!["/bin/true".into()],
                cwd: workspace.display().to_string(),
                env: std::collections::BTreeMap::new(),
                process_id: None,
                tty: false,
                stream_stdin: false,
                stream_stdout_stderr: false,
                output_bytes_cap: None,
                timeout_ms: None,
                size: None,
            }),
        )
        .await
        else {
            panic!("an unbound Session's command must fail")
        };
        assert_eq!(failure.error.data, Some(ErrorData::ExecutionUnavailable));

        client.close().await;
        terminate(&child);
        let output = child.wait_with_output().await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    })
    .await;
}

#[tokio::test]
async fn app_server_websocket_real_process_shared_conformance_and_listener_survives() {
    bounded(Box::pin(async {
        let f = Fixture::new().await;
        let (mut child, url, _stderr) = f.ws().await;
        let client = driver::websocket(&url).await;
        Box::pin(app_server_conformance::representative_scenario(
            &client,
            f.sessions.clone(),
        ))
        .await;
        client.close().await;
        let mut replacement = driver::socket(&url).await;
        replacement.send(INITIALIZE.into()).await.unwrap();
        assert_eq!(
            json_response(&mut replacement).await["result"]["protocol_version"],
            rustx::app_server::protocol::APP_SERVER_PROTOCOL_VERSION
        );
        kill(
            Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()),
            Signal::SIGTERM,
        )
        .unwrap();
        assert!(child.wait().await.unwrap().success());
    }))
    .await;
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
async fn json_response(socket: &mut Socket) -> serde_json::Value {
    loop {
        let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
            panic!("text response");
        };
        let value: serde_json::Value = serde_json::from_str(text.as_str()).unwrap();
        if value.get("id").is_some() {
            return value;
        }
    }
}

#[tokio::test]
async fn app_server_websocket_authentication_framing_and_protocol_errors() {
    bounded(async {
        let f = Fixture::new().await;
        let (mut child, url, _stderr) = f.ws().await;
        let old_offer = format!("rustx.app-server.v53, rustx-token.{}", driver::TOKEN);
        for offer in [
            None,
            Some("rustx.app-server.v81"),
            Some(old_offer.as_str()),
            Some("rustx.app-server.v81, rustx-token.wrong"),
        ] {
            let mut request = url.as_str().into_client_request().unwrap();
            if let Some(offer) = offer {
                request
                    .headers_mut()
                    .insert("sec-websocket-protocol", offer.parse().unwrap());
            }
            let error = tokio_tungstenite::connect_async(request).await.unwrap_err();
            let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
                panic!("HTTP rejection");
            };
            assert_eq!(response.status(), 401);
        }
        let mut socket = driver::socket(&url).await;
        let middle = INITIALIZE.len() / 2;
        socket
            .send(Message::Frame(Frame::message(
                INITIALIZE.as_bytes()[..middle].to_vec(),
                OpCode::Data(Data::Text),
                false,
            )))
            .await
            .unwrap();
        socket
            .send(Message::Frame(Frame::message(
                INITIALIZE.as_bytes()[middle..].to_vec(),
                OpCode::Data(Data::Continue),
                true,
            )))
            .await
            .unwrap();
        assert_eq!(json_response(&mut socket).await["id"], 1);
        // The next two responses prove distinct messages and no duplicate fragment dispatch.
        for (record, code) in [
            ("{", -32700),
            (r#"{"jsonrpc":"2.0","id":2,"method":"bogus"}"#, -32601),
            (
                r#"{"jsonrpc":"2.0","id":3,"method":"thread/start","params":{}}"#,
                -32602,
            ),
            ("[]", -32600),
        ] {
            socket.send(record.into()).await.unwrap();
            assert_eq!(json_response(&mut socket).await["error"]["code"], code);
        }
        socket
            .send(Message::Binary(INITIALIZE.as_bytes().to_vec().into()))
            .await
            .unwrap();
        assert!(
            socket
                .next()
                .await
                .is_none_or(|result| result.is_err() || matches!(result, Ok(Message::Close(_))))
        );
        let mut socket = driver::socket(&url).await;
        let _ = socket
            .send(Message::Text(" ".repeat(MAX_MESSAGE_BYTES + 1).into()))
            .await;
        assert!(
            socket
                .next()
                .await
                .is_none_or(|result| result.is_err() || matches!(result, Ok(Message::Close(_))))
        );
        let mut socket = driver::socket(&url).await;
        let exact = format!(
            "{}{}",
            " ".repeat(MAX_MESSAGE_BYTES - INITIALIZE.len()),
            INITIALIZE
        );
        socket.send(exact.into()).await.unwrap();
        assert_eq!(json_response(&mut socket).await["id"], 1);
        child.kill().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn app_server_stdio_protocol_errors_are_records_but_framing_is_terminal() {
    bounded(async {
        let f = Fixture::new().await;
        let mut child = f.command("stdio").spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
        for (record, code) in [
            ("{", -32700),
            ("[]", -32600),
            (r#"{"jsonrpc":"2.0","id":2,"method":"bogus"}"#, -32601),
        ] {
            input
                .write_all(format!("{record}\n").as_bytes())
                .await
                .unwrap();
            let reply: serde_json::Value =
                serde_json::from_str(&output.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(reply["error"]["code"], code);
        }
        input
            .write_all(
                format!(
                    "{}{}\n",
                    " ".repeat(MAX_MESSAGE_BYTES - INITIALIZE.len()),
                    INITIALIZE
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        // Initialization admits the User source before its reply is physically
        // written. Correlate the reply and the one owed empty repair level in
        // either order; neither an unrelated record nor a duplicate is allowed.
        let (mut initialized, mut level) = (false, false);
        while !initialized || !level {
            let record: serde_json::Value =
                serde_json::from_str(&output.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(record["jsonrpc"], "2.0", "{record}");
            if record.get("id").is_some() {
                assert!(!initialized, "duplicate initialize response: {record}");
                assert_eq!(record["id"], 1, "{record}");
                assert!(record.get("error").is_none(), "{record}");
                assert_eq!(record["result"]["type"], "initialized", "{record}");
                assert_eq!(
                    record["result"]["protocol_version"],
                    rustx::app_server::protocol::APP_SERVER_PROTOCOL_VERSION,
                    "{record}"
                );
                initialized = true;
            } else {
                assert!(!level, "duplicate repair level: {record}");
                assert_eq!(
                    record["method"], "configuration/sourceRepairChanged",
                    "{record}"
                );
                assert_eq!(
                    record["params"],
                    serde_json::json!({
                        "target": { "kind": "user" },
                        "receipt": null,
                        "sequence": "0",
                    }),
                    "{record}"
                );
                level = true;
            }
        }
        drop(input);
        detach_then_shutdown(&mut child).await;
        assert!(child.wait().await.unwrap().success());
        assert_eq!(
            output.next_line().await.unwrap(),
            None,
            "unexpected extra record"
        );
        for bytes in [
            vec![0xff, b'\n'],
            b"{}".to_vec(),
            vec![b' '; MAX_MESSAGE_BYTES + 1],
        ] {
            let mut child = f.command("stdio").spawn().unwrap();
            let mut input = child.stdin.take().unwrap();
            let _ = input.write_all(&bytes).await;
            drop(input);
            detach_then_shutdown(&mut child).await;
            let output = child.wait_with_output().await.unwrap();
            assert!(!output.status.success());
            assert_eq!(output.stdout, [] as [u8; 0]);
        }
    })
    .await;
}

#[tokio::test]
async fn app_server_bootstrap_fails_before_readiness_and_owner_may_kill_stdio_child() {
    bounded(async {
        let f = Fixture::new().await;
        let mut child = f.command("stdio").spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        input
            .write_all(format!("{INITIALIZE}\n").as_bytes())
            .await
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
        assert!(output.next_line().await.unwrap().is_some());
        child.kill().await.unwrap();
        assert!(!child.wait().await.unwrap().success());
        for failure in ["settings", "catalog", "root"] {
            let f = Fixture::new().await;
            let path: PathBuf = match failure {
                "settings" | "catalog" => f.root.path().join("home/rustx/rustx.toml"),
                _ => {
                    std::fs::remove_dir_all(f.root.path().join("runtime")).unwrap();
                    f.root.path().join("runtime")
                }
            };
            std::fs::write(path, "invalid").unwrap();
            for listen in ["stdio", "ws://127.0.0.1:0"] {
                let mut command = f.command(listen);
                if listen != "stdio" {
                    command.arg("--token-file").arg(f.root.path().join("token"));
                }
                let output = command.output().await.unwrap();
                assert!(!output.status.success());
                assert_eq!(output.stdout, [] as [u8; 0]);
                assert!(
                    !String::from_utf8(output.stderr)
                        .unwrap()
                        .contains("listening")
                );
            }
        }
    })
    .await;
}

#[tokio::test]
async fn app_server_stdio_broken_output_pipe_settles_with_input_still_open() {
    bounded(async {
        let f = Fixture::new().await;
        let mut child = f.command("stdio").spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        drop(child.stdout.take());
        input
            .write_all(format!("{INITIALIZE}\n").as_bytes())
            .await
            .unwrap();
        detach_then_shutdown(&mut child).await;
        assert!(child.wait().await.unwrap().success());
    })
    .await;
}

#[tokio::test]
async fn app_server_explicit_user_config_are_authoritative_for_both_transports() {
    bounded(async {
        use app_server_conformance::AppServerConformanceDriver;
        use rustx::app_server::protocol::*;
        for ws in [false, true] {
            for explicit in [false, true] {
                let f = Fixture::new().await;
                let ambient = f.root.path().join("home/rustx");
                let selected = f.root.path().join("selected");
                std::fs::create_dir(&selected).unwrap();
                let authored = std::fs::read_to_string(ambient.join("rustx.toml")).unwrap();
                std::fs::write(selected.join("rustx.toml"), &authored).unwrap();
                std::fs::write(
                    ambient.join("rustx.toml"),
                    authored.replace("context_window = 128000", "context_window = 64000"),
                )
                .unwrap();
                let mut command = f.command(if ws { "ws://127.0.0.1:0" } else { "stdio" });
                if explicit {
                    command.arg("--config").arg(selected.join("rustx.toml"));
                }
                if ws {
                    command.arg("--token-file").arg(f.root.path().join("token"));
                }
                let mut child = command.spawn().unwrap();
                let mut _startup_stderr = None;
                let client = if ws {
                    let (endpoint, stderr) = websocket_startup(&mut child).await;
                    _startup_stderr = Some(stderr);
                    driver::websocket(&endpoint).await
                } else {
                    driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap())
                };
                let init: Request = serde_json::from_str(INITIALIZE).unwrap();
                assert!(matches!(client.request(init).await, Response::Success(_)));
                // Both Sessions use the same process source binding.
                for (index, session_id) in f.sessions.iter().enumerate() {
                    let Response::Success(attached) = client
                        .request(Request {
                            jsonrpc: JsonRpcVersion::V2,
                            id: RequestId::Integer(10 + i64::try_from(index).unwrap()),
                            call: Method::SessionAttach {
                                thread_id: app_server_conformance::thread_id(
                                    &client,
                                    session_id.clone(),
                                )
                                .await,
                            },
                        })
                        .await
                    else {
                        panic!("attach");
                    };
                    let MethodResult::Attached { target, .. } = attached.result else {
                        panic!("attached");
                    };
                    let response = client
                        .request(Request {
                            jsonrpc: JsonRpcVersion::V2,
                            id: RequestId::Integer(20 + i64::try_from(index).unwrap()),
                            call: Method::ModelCatalog { target },
                        })
                        .await;
                    let json = serde_json::to_string(&response).unwrap();
                    assert!(
                        json.contains(if explicit { "128000" } else { "64000" }),
                        "{json}"
                    );
                    assert!(
                        !json.contains(if explicit { "64000" } else { "128000" }),
                        "{json}"
                    );
                }
                client.close().await;
                if ws {
                    child.kill().await.unwrap();
                } else {
                    detach_then_shutdown(&mut child).await;
                    assert!(child.wait().await.unwrap().success());
                }
            }
        }
    })
    .await;
}

#[tokio::test]
async fn app_server_explicit_user_config_fail_before_readiness() {
    bounded(async {
        let f = Fixture::new().await;
        for contents in [None, Some("secret-sentinel invalid TOML")] {
            let path = f.root.path().join("selected.toml");
            if let Some(contents) = contents {
                std::fs::write(&path, contents).unwrap();
            }
            for listen in ["stdio", "ws://127.0.0.1:0"] {
                let mut command = f.command(listen);
                command.arg("--config").arg(&path);
                if listen != "stdio" {
                    command.arg("--token-file").arg(f.root.path().join("token"));
                }
                let output = command.output().await.unwrap();
                assert!(!output.status.success());
                assert_eq!(output.stdout, [] as [u8; 0]);
                let error = String::from_utf8(output.stderr).unwrap();
                assert!(!error.contains("listening"));
                assert!(!error.contains("secret-sentinel"));
            }
        }
    })
    .await;
}

async fn rpc(
    client: &driver::Driver,
    id: i64,
    call: rustx::app_server::protocol::Method,
) -> rustx::app_server::protocol::Response {
    use app_server_conformance::AppServerConformanceDriver;
    use rustx::app_server::protocol::*;
    client
        .request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call,
        })
        .await
}
async fn initialize_client(client: &driver::Driver) {
    let request: rustx::app_server::protocol::Request = serde_json::from_str(INITIALIZE).unwrap();
    assert!(matches!(
        rpc(client, 1, request.call).await,
        rustx::app_server::protocol::Response::Success(_)
    ));
}
async fn attach(
    client: &driver::Driver,
    session: SessionId,
    id: i64,
) -> rustx::app_server::protocol::AttachmentTarget {
    use rustx::app_server::protocol::*;
    let response = rpc(
        client,
        id,
        Method::SessionAttach {
            thread_id: app_server_conformance::thread_id(client, session).await,
        },
    )
    .await;
    let Response::Success(response) = response else {
        panic!("attach failed: {response:?}")
    };
    let MethodResult::Attached { target, .. } = response.result else {
        panic!("attached")
    };
    let Response::Success(checkpoint_response) = rpc(
        client,
        id + 9000,
        Method::SessionCheckpoint {
            trace_records: vec![],
            target: target.clone(),
        },
    )
    .await
    else {
        panic!("attachment checkpoint")
    };
    let MethodResult::Checkpoint { checkpoint } = checkpoint_response.result else {
        panic!("bounded checkpoint")
    };
    let Response::Success(subscription_response) = rpc(
        client,
        id + 10000,
        Method::SessionSubscribe {
            target: target.clone(),
            after_cursor: checkpoint.cursor,
        },
    )
    .await
    else {
        panic!("subscription")
    };
    let MethodResult::Subscribed {
        target: following,
        after_cursor,
        registration_id,
    } = subscription_response.result
    else {
        panic!("subscription receipt")
    };
    assert_eq!(following, target);
    assert_eq!(after_cursor, checkpoint.cursor);
    assert_ne!(registration_id, "");
    target
}

#[tokio::test]
async fn app_server_owned_stdio_shutdown_cold_resumes_multiple_sessions() {
    bounded(async {
        use rustx::app_server::protocol::*;
        let f = Fixture::new().await;
        for _ in 0..2 {
            let mut child = f.command("stdio").spawn().unwrap();
            let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
            initialize_client(&client).await;
            let a = attach(&client, f.sessions[0].clone(), 2).await;
            attach(&client, f.sessions[1].clone(), 3).await;
            assert!(matches!(
                rpc(&client, 4, Method::SessionDetach { target: a }).await,
                Response::Success(_)
            ));
            let Response::Success(response) = rpc(&client, 5, Method::ServerDiagnostics {}).await
            else {
                panic!("diagnostics")
            };
            let MethodResult::Diagnostics { snapshot } = response.result else {
                panic!("snapshot")
            };
            assert_eq!(snapshot.loaded, 2, "focus change is not unload");
            terminate(&child);
            let output = child.wait_with_output().await.unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("Draining; new semantic admission closed")
            );
            client.close().await;
        }
    })
    .await;
}

#[tokio::test]
async fn app_server_websocket_drain_supervises_active_root_and_cold_resume() {
    Box::pin(bounded(async {
        use rustx::app_server::protocol::*;
        use tokio::io::AsyncReadExt;
        for forced in [false, true] {
            let f = Fixture::new().await;
            if forced {
                let settings = f.root.path().join("home/rustx/rustx.toml");
                let mut text = std::fs::read_to_string(&settings).unwrap();
                text.push_str("\n[app_server]\nshutdown_deadline_ms = 2000\n");
                std::fs::write(settings, text).unwrap();
            }
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let models = f.root.path().join("home/rustx/rustx.toml");
            let text = std::fs::read_to_string(&models)
                .unwrap()
                .replace("127.0.0.1:9", &listener.local_addr().unwrap().to_string());
            std::fs::write(models, text).unwrap();
            let controller = SessionController::open(&f.root.path().join("runtime")).unwrap();
            let database = controller
                .acquire_session(&f.sessions[0], None)
                .await
                .unwrap()
                .database_path;
            drop(controller);
            let (mut child, url, stderr) = f.ws().await;
            let client = driver::websocket(&url).await;
            initialize_client(&client).await;
            let a = attach(&client, f.sessions[0].clone(), 2).await;
            attach(&client, f.sessions[1].clone(), 3).await;
            let content = vec![rustx::app_server::protocol::UserInputBlock::Text(
                rustx::message::content::TextBlock {
                    text: "owned root".into(),
                },
            )];
            assert!(matches!(
                rpc(
                    &client,
                    4,
                    Method::TurnStart {
                        input_id: uuid::Uuid::new_v4().to_string(),
                        target: a,
                        content
                    }
                )
                .await,
                Response::Success(_)
            ));
            let (mut request, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            assert!(
                request.read(&mut bytes).await.unwrap() > 0,
                "provider request really started"
            );
            // Hold durable terminal publication at a real boundary. Admission
            // refusal while another owner holds the catalog is covered by the
            // deterministic protocol test; a websocket round trip here would
            // have to race SQLite's finite busy budget (or the forced deadline).
            let database = rusqlite::Connection::open(database).unwrap();
            database.execute_batch("BEGIN IMMEDIATE").unwrap();
            terminate(&child);
            let mut lines = stderr.lines();
            let line = lines.next_line().await.unwrap().unwrap();
            assert!(
                line.contains("Draining; new semantic admission closed"),
                "{line}"
            );
            if forced {
                // Startup transferred stderr to this persistent reader. Drain
                // it alongside child exit, including any buffered read-ahead.
                let mut stderr = lines.into_inner();
                let mut diagnostics = String::new();
                let (output, drained) = tokio::join!(
                    child.wait_with_output(),
                    stderr.read_to_string(&mut diagnostics)
                );
                let output = output.unwrap();
                drained.unwrap();
                assert_eq!(output.status.code(), Some(3));
                assert!(diagnostics.contains("runtime settlement unproven"));
                database.execute_batch("ROLLBACK").unwrap();
            } else {
                // Release as soon as the process proves admission is closed;
                // successful native settlement must still precede exit zero.
                database.execute_batch("ROLLBACK").unwrap();
                assert!(child.wait().await.unwrap().success());
            }
            drop(request);
            client.close().await;
            let (mut replacement, url, _stderr) = f.ws().await;
            let client = driver::websocket(&url).await;
            initialize_client(&client).await;
            attach(&client, f.sessions[0].clone(), 6).await;
            attach(&client, f.sessions[1].clone(), 7).await;
            terminate(&replacement);
            assert!(replacement.wait().await.unwrap().success());
            client.close().await;
        }
    }))
    .await;
}

/// A peer that ends the transport while a request is pending fails that
/// request promptly with the transport's own cause, never stranding it until
/// an outer guard. The pending point is the peer's receipt of the request.
#[tokio::test]
async fn driver_fails_pending_requests_with_the_transport_cause() {
    use rustx::app_server::protocol::{JsonRpcVersion, Method, Request, RequestId};
    use tokio_tungstenite::tungstenite::handshake::server::{Request as Upgrade, Response};
    Box::pin(bounded(async {
        for reset in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let (received, request_received) = tokio::sync::oneshot::channel();
            let (end, ended) = tokio::sync::oneshot::channel::<()>();
            let peer = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                #[allow(clippy::result_large_err)] // tungstenite's handshake callback type.
                let mut socket = tokio_tungstenite::accept_hdr_async(
                    stream,
                    |_: &Upgrade, mut response: Response| {
                        response.headers_mut().insert(
                            "sec-websocket-protocol",
                            "rustx.app-server.v81".parse().unwrap(),
                        );
                        Ok(response)
                    },
                )
                .await
                .unwrap();
                let request = socket.next().await.unwrap().unwrap();
                received
                    .send(request.into_text().unwrap().to_string())
                    .unwrap();
                ended.await.unwrap();
                if reset {
                    // An abortive close: the client observes a reset.
                    socket.get_ref().set_zero_linger().unwrap();
                    drop(socket);
                } else {
                    socket.close(None).await.unwrap();
                }
            });
            let client = driver::websocket(&url).await;
            let cause = {
                let pending = client.try_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(7),
                    call: Method::ServerDiagnostics {},
                });
                tokio::pin!(pending);
                tokio::select! {
                    outcome = &mut pending => panic!("answered without a peer reply: {outcome:?}"),
                    request = request_received => assert!(request.unwrap().contains("\"id\":7")),
                }
                end.send(()).unwrap();
                pending.await.unwrap_err().0
            };
            if reset {
                assert!(cause.contains("Connection reset"), "{cause}");
            } else {
                assert_eq!(cause, "end of input");
            }
            // Later requests fail at once with the same recorded cause.
            let later = client
                .try_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(8),
                    call: Method::ServerDiagnostics {},
                })
                .await
                .unwrap_err();
            assert_eq!(later.0, cause);
            peer.await.unwrap();
            client.close().await;
        }
    }))
    .await;
}

// The injection seam is absent from optimized production binaries.
#[cfg(debug_assertions)]
#[tokio::test]
async fn app_server_process_exits_with_cancelled_blocked_presentation_read() {
    Box::pin(bounded(async {
        use rustx::app_server::protocol::*;
        let f = Fixture::new().await;
        let mut child = f
            .command("ws://127.0.0.1:0")
            .arg("--token-file")
            .arg(f.root.path().join("token"))
            .env("RUSTX_TEST_PRESENTATION_READ_BLOCK", "1")
            .spawn()
            .unwrap();
        let (url, stderr) = websocket_startup(&mut child).await;
        let client = driver::websocket(&url).await;
        initialize_client(&client).await;
        let target = attach(&client, f.sessions[0].clone(), 2).await;
        assert!(matches!(
            rpc(
                &client,
                3,
                Method::TurnStart {
                    input_id: uuid::Uuid::new_v4().to_string(),
                    target,
                    content: vec![UserInputBlock::Text(rustx::message::content::TextBlock {
                        text: "presentation cancellation".into()
                    })],
                }
            )
            .await,
            Response::Success(_)
        ));
        let mut lines = stderr.lines();
        loop {
            let line = lines.next_line().await.unwrap().unwrap();
            if line == "rustx presentation read blocked at connection admission" {
                break;
            }
        }
        terminate(&child);
        // No release channel or lock rollback exists. Production cancellation
        // must end the Store wait and Tokio must really finish destruction.
        assert!(child.wait().await.unwrap().success());
        client.close().await;
    }))
    .await;
}

// APP-09 composition uses the existing external emulator, native source authoring,
// and public protocol. The fixture above owns processes/paths, never Session state.
use crate::common::provider_emulator;

async fn result(
    client: &driver::Driver,
    call: rustx::app_server::protocol::Method,
) -> rustx::app_server::protocol::MethodResult {
    match rpc(client, 100, call).await {
        rustx::app_server::protocol::Response::Success(response) => response.result,
        response @ rustx::app_server::protocol::Response::Failure(_) => {
            panic!("unexpected response: {response:?}")
        }
    }
}

async fn checkpoint_state(
    client: &driver::Driver,
    target: &rustx::app_server::protocol::AttachmentTarget,
) -> serde_json::Value {
    use rustx::app_server::protocol::*;
    let MethodResult::Checkpoint { checkpoint } = result(
        client,
        Method::SessionCheckpoint {
            trace_records: vec![],
            target: target.clone(),
        },
    )
    .await
    else {
        panic!("snapshot")
    };
    serde_json::to_value(checkpoint.snapshot).unwrap()
}

async fn model_value(
    client: &driver::Driver,
    target: &rustx::app_server::protocol::AttachmentTarget,
    page: &rustx::runtime_client::checkpoint::CheckpointPage,
) -> rustx::model::session::SessionModelView {
    use base64::Engine as _;
    use rustx::app_server::protocol::*;
    assert_eq!(page.domain, "model");
    assert_eq!(page.total, 1);
    assert_eq!(page.items.len(), 1);
    assert!(page.next_offset.is_none());
    let item_offset = page.items[0].offset;
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let MethodResult::CheckpointValueChunk { chunk } = result(
            client,
            Method::ConversationCheckpointValueRead {
                target: target.clone(),
                agent_id: None,
                cut: page.read_cut.clone(),
                domain: page.domain.clone(),
                item_offset,
                path: Vec::new(),
                offset,
                max_bytes: 65_536,
            },
        )
        .await
        else {
            panic!("same-cut original model bytes")
        };
        assert_eq!(chunk.cut, page.read_cut);
        assert_eq!(chunk.domain, page.domain);
        assert_eq!(chunk.item_offset, item_offset);
        assert_eq!(chunk.path, [] as [std::string::String; 0]);
        assert_eq!(chunk.offset, offset);
        assert_eq!(
            chunk.representation,
            rustx::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1
        );
        let part = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert!(part.len() <= 65_536);
        bytes.extend_from_slice(&part);
        assert!(bytes.len() <= 786_432, "finite original model fixture");
        let Some(next) = chunk.next_offset else {
            assert_eq!(bytes.len() as u64, chunk.readable_end);
            break;
        };
        assert!(next > offset);
        assert_eq!(next, bytes.len() as u64);
        offset = next;
    }
    rustx::observation::bounded::decode_domain_value(serde_json::from_slice(&bytes).unwrap())
        .expect("exact original native model value")
}

async fn diagnostics(client: &driver::Driver) -> rustx::app_server::host::ServerDiagnostics {
    use rustx::app_server::protocol::*;
    let MethodResult::Diagnostics { snapshot } = result(client, Method::ServerDiagnostics {}).await
    else {
        panic!("diagnostics")
    };
    snapshot
}

async fn settled(
    client: &driver::Driver,
    target: &rustx::app_server::protocol::AttachmentTarget,
) -> serde_json::Value {
    loop {
        let value = checkpoint_state(client, target).await;
        if value["attempt"]["phase"]["type"] == "settled" {
            return value;
        }
        // Readiness polling only: the returned authoritative phase is evidence.
        tokio::task::yield_now().await;
    }
}

async fn start_turn(
    client: &driver::Driver,
    target: &rustx::app_server::protocol::AttachmentTarget,
    text: &str,
) {
    use rustx::app_server::protocol::*;
    result(
        client,
        Method::TurnStart {
            input_id: uuid::Uuid::new_v4().to_string(),
            target: target.clone(),
            content: (vec![rustx::message::types::UserContentBlock::Text(
                rustx::message::content::TextBlock { text: text.into() },
            )])
            .into_iter()
            .map(|block| match block {
                rustx::message::types::UserContentBlock::Text(text) => {
                    rustx::app_server::protocol::UserInputBlock::Text(text)
                }
                _ => panic!("client fixtures must use text or issued receipts"),
            })
            .collect(),
        },
    )
    .await;
}

fn use_emulator(f: &Fixture, provider: &provider_emulator::ProviderEmulator) {
    let catalog = f.root.path().join("home/rustx/rustx.toml");
    let text = std::fs::read_to_string(&catalog)
        .unwrap()
        .replace("http://127.0.0.1:9/v1", &provider.openai_base_url())
        .replace("id = \"test\"", "id = \"integration-model\"");
    std::fs::write(catalog, text).unwrap();
    let settings = f.root.path().join("home/rustx/rustx.toml");
    let text = std::fs::read_to_string(&settings)
        .unwrap()
        .replace("local/test", "local/integration-model");
    std::fs::write(settings, text).unwrap();
}

#[tokio::test]
async fn app_server_concurrent_sessions_finish_across_external_disconnect() {
    bounded(async {
        use rustx::app_server::protocol::*;
        let Some(provider) = provider_emulator::ProviderEmulator::start("tui_multi_session").await
        else {
            return;
        };
        let f = Fixture::new().await;
        use_emulator(&f, &provider);
        let (mut child, url, _stderr) = f.ws().await;
        let pid = child.id();
        let client = driver::websocket(&url).await;
        initialize_client(&client).await;
        std::fs::write(
            f.root.path().join("b/rustx.toml"),
            "[agent.plugins.todo]\nenabled = false\n",
        )
        .unwrap();
        let mut targets = Vec::new();
        for cwd in ["a", "b"] {
            let MethodResult::SessionTransition { session, .. } = result(
                &client,
                Method::SessionCreate {
                    dynamic_tools: Vec::new(),
                    submission: None,
                    settings: SessionPersistentState::from_input(&SessionConfigInput::new(
                        f.root.path().join(cwd),
                    )),
                },
            )
            .await
            else {
                panic!("create")
            };
            result(
                &client,
                Method::SessionName {
                    session_id: session.id.clone(),
                    name: format!("concurrent process fixture {cwd}"),
                },
            )
            .await;
            targets.push(attach(&client, session.id, 2).await);
        }
        let [a, b]: [AttachmentTarget; 2] = targets.try_into().unwrap();
        start_turn(&client, &a, "tui multi-session: session A long task").await;
        provider.await_gate("session-a-holding").await;
        start_turn(&client, &b, "tui multi-session: session B quick task").await;
        let b_done = settled(&client, &b).await;
        assert_eq!(provider.requests().await.len(), 2);
        assert!(b_done["effective_plugins"]["todo"].is_null());
        // Drain through B's authoritative settlement publication. Every event
        // before that cut must route to its original Session/Conversation.
        loop {
            use app_server_conformance::AppServerConformanceDriver;
            let notification = client.next_notification().await.notification;
            // Lightweight Thread status is instance-scoped, without changing
            // transcript routing or subscribing this client to another Thread.
            if let NotificationMethod::ThreadStatusChanged(entry) = &notification {
                assert!(
                    entry.thread_id == a.conversation_id || entry.thread_id == b.conversation_id
                );
                continue;
            }
            // Session metadata invalidation is not a Conversation event and is
            // not routed by attachment target (Issue #386); nor is a
            // source-scoped repair level (Issue #464), nor a Session's
            // configuration application, which the source watch may publish
            // when the Session's Workspace becomes known (Issue #518).
            if matches!(
                notification,
                NotificationMethod::SummaryInvalidated { .. }
                    | NotificationMethod::SourceRepairChanged { .. }
                    | NotificationMethod::ConfigurationChanged { .. }
                    | NotificationMethod::ThreadStatusResyncRequired { .. }
            ) {
                continue;
            }
            let NotificationMethod::Event { target, event, .. } = notification else {
                panic!("unexpected invalidation")
            };
            assert!(target == a || target == b);
            let rendered = serde_json::to_string(&event).unwrap();
            assert!(!rendered.contains(if target == a {
                "B answered"
            } else {
                "A is working"
            }));
            let expected_attempt = b_done["attempt"]["attempt_id"]
                .as_str()
                .expect("B's authoritative settled attempt identity");
            if target == b
                && event.facts().iter().any(|fact| {
                    use rustx::runtime_client::{
                        event::RuntimeClientEvent, snapshot::RuntimeClientAttemptPhase,
                    };
                    match fact {
                        RuntimeClientEvent::AttemptSettled { attempt_id, .. } => {
                            attempt_id.as_str() == expected_attempt
                        }
                        RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                            checkpoint.snapshot.attempt.as_ref().is_some_and(|attempt| {
                                attempt
                                    .attempt_id
                                    .as_ref()
                                    .is_some_and(|id| id.as_str() == expected_attempt)
                                    && matches!(
                                        attempt.phase,
                                        RuntimeClientAttemptPhase::Settled { .. }
                                    )
                            })
                        }
                        _ => false,
                    }
                })
            {
                break;
            }
        }
        assert_eq!(
            checkpoint_state(&client, &a).await["attempt"]["phase"]["type"],
            "running"
        );
        assert_eq!(diagnostics(&client).await.loaded, 2);
        assert_eq!(child.id(), pid);
        assert_ne!(a.conversation_id, b.conversation_id);
        let b_history = b_done["messages"].to_string();
        assert!(b_history.contains("B answered while A was still working"));
        assert!(!b_history.contains("A is working"));
        client.close().await;
        // A host observer has no Session attachment and cannot continue/answer
        // work. Observe actual detach cleanup before releasing the provider.
        let observer = driver::websocket(&url).await;
        initialize_client(&observer).await;
        while diagnostics(&observer).await.external_attachments != 0 {
            tokio::task::yield_now().await;
        }
        provider.release_gate("session-a-holding").await;
        while diagnostics(&observer).await.active_roots != 0 {
            tokio::task::yield_now().await;
        }
        assert!(child.try_wait().unwrap().is_none());
        let reconnected = driver::websocket(&url).await;
        initialize_client(&reconnected).await;
        let resumed = attach(&reconnected, a.session_id.clone(), 2).await;
        assert_eq!(resumed.runtime_incarnation, a.runtime_incarnation);
        assert_eq!(resumed.conversation_id, a.conversation_id);
        assert_ne!(resumed.attachment_id, a.attachment_id);
        let a_done = settled(&reconnected, &resumed).await;
        assert_eq!(a_done["attempt"]["phase"]["outcome"]["type"], "completed");
        assert!(!a_done["effective_plugins"]["todo"].is_null());
        let history = a_done["messages"].to_string();
        assert_eq!(
            history.matches("A is working and has now finished").count(),
            1
        );
        assert!(!history.contains("B answered"));
        assert_eq!(
            a_done["pending_interactions"].as_array().unwrap(),
            &Vec::<serde_json::Value>::new()
        );
        assert!(!a_done["transcript"].to_string().contains("cancelled"));
        assert_eq!(provider.requests().await.len(), 2, "no transport retry");
        for (target, cwd) in [(&resumed, "a"), (&b, "b")] {
            let MethodResult::Settings { projection, .. } = result(
                &reconnected,
                Method::SettingsRead {
                    session_id: target.session_id.clone(),
                },
            )
            .await
            else {
                panic!("settings")
            };
            assert_eq!(projection.cwd, f.root.path().join(cwd));
        }
        reconnected.close().await;
        observer.close().await;
        terminate(&child);
        assert!(child.wait().await.unwrap().success());
        provider.finish().await;
    })
    .await;
}

#[tokio::test]
async fn app_server_current_sources_and_persisted_selection_survive_process_reconstruction() {
    bounded(async {
        use rustx::app_server::protocol::*;
        let Some(provider) =
            provider_emulator::ProviderEmulator::start("app_server_replacement").await
        else {
            return;
        };
        let f = Fixture::new().await;
        use_emulator(&f, &provider);
        let source = f.root.path().join("home/rustx/rustx.toml");
        let initial = std::fs::read_to_string(&source).unwrap();
        let (mut child, url, _stderr) = f.ws().await;
        let client = driver::websocket(&url).await;
        initialize_client(&client).await;
        let a = attach(&client, f.sessions[0].clone(), 2).await;
        let mut selected: rustx::model::session::SessionModelConfig = serde_json::from_value(
            checkpoint_state(&client, &a).await["model"]["configured"].clone(),
        )
        .unwrap();
        selected
            .request_params
            .insert("temperature".into(), serde_json::json!(0.42));
        result(
            &client,
            Method::ModelSet {
                recover_unavailable_auxiliary: None,
                target: a.clone(),
                config: Box::new(selected),
            },
        )
        .await;
        let a_initial = checkpoint_state(&client, &a).await;
        // Deliberate Session selection survives process reconstruction.
        start_turn(&client, &a, "tui multi-session: session A long task").await;
        provider.await_gate("session-a-holding").await;
        let admitted = checkpoint_state(&client, &a).await["attempt"].clone();
        // One canonical current source edit; no alternate resolver or input path.
        std::fs::write(
            &source,
            initial.replace(
                "[agent.plugins.todo]\nenabled = true",
                "[agent.plugins.todo]\nenabled = false",
            ),
        )
        .unwrap();
        assert_eq!(
            checkpoint_state(&client, &a).await["effective_plugins"],
            a_initial["effective_plugins"]
        );
        assert_eq!(
            checkpoint_state(&client, &a).await["attempt"]["model"],
            admitted["model"]
        );
        let b = attach(&client, f.sessions[1].clone(), 3).await;
        let b_initial = checkpoint_state(&client, &b).await;
        assert!(b_initial["effective_plugins"]["todo"].is_null());
        assert!(!a_initial["effective_plugins"]["todo"].is_null());
        start_turn(&client, &b, "tui multi-session: session B quick task").await;
        provider.await_gate("session-b-holding").await;
        provider.release_gate("session-a-holding").await;
        let before = settled(&client, &a).await;
        // Composition reconstruction is a process-owner operation here. The
        // internal manager suite proves targeted replacement independently.
        assert_eq!(
            checkpoint_state(&client, &b).await["effective_plugins"],
            b_initial["effective_plugins"]
        );
        assert_eq!(
            checkpoint_state(&client, &b).await["attempt"]["phase"]["type"],
            "running"
        );
        provider.release_gate("session-b-holding").await;
        settled(&client, &b).await;
        client.close().await;
        terminate(&child);
        assert!(child.wait().await.unwrap().success());
        let (mut child, url, _stderr) = f.ws().await;
        let client = driver::websocket(&url).await;
        initialize_client(&client).await;
        let cold = attach(&client, a.session_id.clone(), 4).await;
        assert_eq!(cold.conversation_id, a.conversation_id);
        let after = checkpoint_state(&client, &cold).await;
        assert!(after["effective_plugins"]["todo"].is_null());
        assert_eq!(
            after["model"]["configured"],
            a_initial["model"]["configured"]
        );
        assert_eq!(after["messages"], before["messages"]);
        assert_eq!(diagnostics(&client).await.loaded, 1);
        let MethodResult::Settings {
            projection: persisted,
            ..
        } = result(
            &client,
            Method::SettingsRead {
                session_id: a.session_id,
            },
        )
        .await
        else {
            panic!("settings")
        };
        assert_eq!(
            serde_json::to_value(
                persisted
                    .inline
                    .expect("small inline settings fixture")
                    .model
            )
            .unwrap(),
            a_initial["model"]["configured"]
        );
        assert_eq!(provider.requests().await.len(), 2);
        client.close().await;
        terminate(&child);
        assert!(child.wait().await.unwrap().success());
        provider.finish().await;
    })
    .await;
}

/// Reference higher-level host composition. Authentication has already supplied
/// the two identities. Routing selects a whole process/source/root/environment;
/// only public RPC creates/opens Sessions. This is not the internal `AppServerHost`
/// and does not implement authentication, workspace isolation or restart policy.
#[tokio::test]
async fn app_server_reference_host_two_users_and_external_crash_recovery() {
    bounded(Box::pin(async {
        use rustx::app_server::protocol::*;
        let Some(pa) = provider_emulator::ProviderEmulator::start("app_server_user_a").await else { return };
        let Some(pb) = provider_emulator::ProviderEmulator::start("app_server_user_b").await else { return };
        let users = [("a", Fixture::new().await, &pa), ("b", Fixture::new().await, &pb)];
        let mut startup_stderr = Vec::new();
        let mut processes = Vec::new();
        let mut clients = Vec::new();
        let mut targets = Vec::new();
        for (identity, user, provider) in &users {
            let skill = user.root.path().join(format!("home/rustx/.agents/skills/host-{identity}"));
            std::fs::create_dir_all(&skill).unwrap();
            std::fs::write(skill.join("SKILL.md"), format!("---\nname: host-{identity}\ndescription: host-{identity}-only resource\n---\nUse this user's supplied workspace.\n")).unwrap();
            let config = user.root.path().join("home/rustx");
            let catalog = std::fs::read_to_string(config.join("rustx.toml")).unwrap()
                .replace("http://127.0.0.1:9/v1", &provider.openai_base_url())
                .replace("id = \"test\"", &format!("id = \"user-{identity}\""))
                .replace("local/test", &format!("local/user-{identity}"));
            std::fs::write(config.join("rustx.toml"), catalog).unwrap();
            let authored = config.join("rustx.toml");
            let mut source: toml::Value = toml::from_str(&std::fs::read_to_string(&authored).unwrap()).unwrap();
            source["agent"].as_table_mut().unwrap().insert("skills".into(), "all".into());
            // Host isolation / crash recovery is not a permission-mode test.
            // Full Access admits write/bash so the fixture reaches process
            // death and cold recovery without Ask stalls under Plan.
            source.as_table_mut().unwrap().insert(
                "approval_mode".into(),
                toml::Value::String("full_access".into()),
            );
            std::fs::write(&authored, toml::to_string_pretty(&source).unwrap()).unwrap();
            let mut text = std::fs::read_to_string(&authored).unwrap();
            write!(text, "\n[environment]\nRUSTX_HOST_MARKER = \"{identity}\"\n").unwrap();
            std::fs::write(authored, text).unwrap();
            let mut child = user.command("ws://127.0.0.1:0")
                .arg("--config").arg(config.join("rustx.toml"))
                .arg("--token-file").arg(user.root.path().join("token"))
                .arg("--local-execution-session").arg(user.sessions[0].as_str())
                .arg("--local-execution-workspace").arg(user.root.path().join("a"))
                .env("TEST_KEY", format!("fake-{identity}"))
                .env("RUSTX_HOST_MARKER", identity).spawn().unwrap();
            let (endpoint, stderr) = websocket_startup(&mut child).await;
            startup_stderr.push(stderr);
            let client = driver::websocket(&endpoint).await;
            initialize_client(&client).await;
            // Exercise writable catalog management while execution stays
            // bound to the exact Session reserved before this Client started.
            let MethodResult::SessionTransition { session: other, .. } = result(&client, Method::SessionCreate { dynamic_tools: Vec::new(), submission: None,
                settings: SessionPersistentState::from_input(&SessionConfigInput::new(user.root.path().join("a")))
            }).await else { panic!("created") };
            let session = user.sessions[0].clone();
            assert_ne!(session, other.id);
            result(&client, Method::SessionName { session_id: session.clone(), name: format!("user-{identity}-private") }).await;
            let target = attach(&client, session, 2).await;
            processes.push(child); clients.push(client); targets.push(target);
        }
        assert_ne!(processes[0].id(), processes[1].id());
        // IDs are scoped to a user's root (and may have identical spellings).
        // Even that spelling on the other process reads only its own metadata.
        for (index, client) in clients.iter().enumerate() {
            let other = 1 - index;
            let list = result(client, Method::SessionList { query: Some(format!("user-{}-private", users[other].0)), offset: 0, limit: 32 }).await;
            let MethodResult::Sessions { sessions, .. } = list else { panic!("list") };
            assert_eq!(sessions, [] as [rustx::app_server::thread_status::SessionSummary; 0]);
            let Response::Failure(failure) = rpc(client, 100, Method::SessionRead { session_id: targets[other].session_id.clone() }).await else { panic!("foreign Session must be absent") };
            assert_eq!(failure.error.data, Some(ErrorData::UnknownSession { session_id: targets[other].session_id.clone() }));
        }
        // Prove absent identities fail at the other root too, in both directions.
        for (index, count) in [(0, 1), (1, 2)] {
            let mut id = targets[index].session_id.clone();
            for _ in 0..count {
                let MethodResult::SessionTransition { session, .. } = result(&clients[index], Method::SessionCreate { dynamic_tools: Vec::new(), submission: None,
                    settings: SessionPersistentState::from_input(&SessionConfigInput::new(users[index].1.root.path().join("b")))
                }).await else { panic!("private Session") };
                id = session.id;
            }
            let Response::Failure(failure) = rpc(&clients[1 - index], 12, Method::SessionRead { session_id: id.clone() }).await else { panic!("foreign Session readable") };
            assert_eq!(failure.error.data, Some(ErrorData::UnknownSession { session_id: id }));
        }
        // Prepare both process roots and catalog checks before either finite
        // model response is parked. Only the live-attempt assertions below
        // belong between provider admission and the crash/release barriers.
        for (index, (identity, _, _)) in users.iter().enumerate() {
            start_turn(&clients[index], &targets[index], &format!("host user {identity}")).await;
        }
        for (index, (identity, user, provider)) in users.iter().enumerate() {
            provider.await_gate("host-result").await;
            assert!(provider.requests().await.iter().all(|request| request["credentialHeaders"].as_array().unwrap().iter().any(|header| header == "authorization")));
            let requests = provider.requests().await;
            assert!(requests[0]["body"].to_string().contains(&format!("host-{identity}-only resource")));
            assert!(!requests[0]["body"].to_string().contains(if *identity == "a" { "host-b-only" } else { "host-a-only" }));
            assert_eq!(std::fs::read(user.root.path().join("a/host-effect")).unwrap(), b"x");
            let projected = checkpoint_state(&clients[index], &targets[index]).await;
            assert!(projected["messages"].to_string().contains(&format!("{identity}:unset")));
            assert!(!projected["messages"].to_string().contains(&format!("fake-{identity}")));
            assert!(projected["messages"].to_string().contains(user.root.path().join("a").to_str().unwrap()));
        }
        let mut selections = Vec::new();
        for (index, client) in clients.iter().enumerate() {
            let before = checkpoint_state(client, &targets[index]).await;
            assert_eq!(before["attempt"]["phase"]["type"], "running");
            let mut model: rustx::model::session::SessionModelConfig = serde_json::from_value(before["model"]["configured"].clone()).unwrap();
            model.request_params.insert("temperature".into(), serde_json::json!(0.42));
            let MethodResult::ModelSet { .. } = result(client, Method::ModelSet { recover_unavailable_auxiliary: None, target: targets[index].clone(), config: Box::new(model.clone()) }).await else { panic!("bounded live model selection receipt") };
            let selected:rustx::model::session::SessionModelView=serde_json::from_value(checkpoint_state(client,&targets[index]).await["model"].clone()).unwrap();
            assert_eq!(selected.configured, model);
            assert_eq!(selected.effective.as_ref().unwrap().request_params["temperature"], serde_json::json!(0.42));
            let during = checkpoint_state(client, &targets[index]).await;
            assert_eq!(during["model"]["configured"], serde_json::to_value(&model).unwrap());
            assert_eq!(during["attempt"]["model"], before["attempt"]["model"]);
            let MethodResult::Settings { projection } = result(client, Method::SettingsRead { session_id: targets[index].session_id.clone() }).await else { panic!("bounded persisted selection") };
            let settings=projection.inline.expect("finite fixture settings");
            assert_eq!(settings.model.as_ref(), Some(&model));
            for request in users[index].2.requests().await {
                assert_eq!(request["body"]["temperature"], before["attempt"]["model"]["primary"]["requestParams"]["temperature"]);
                assert_eq!(request["body"]["max_completion_tokens"], before["attempt"]["model"]["primary"]["maxOutputTokens"]);
            }
            selections.push(model);
        }
        let a_pid = processes[0].id();
        let a_before = checkpoint_state(&clients[0], &targets[0]).await;
        assert_eq!(a_before["attempt"]["phase"]["type"], "running", "A must remain gated before B dies: {}", a_before["attempt"]["phase"]);
        // B dies after its tool result has entered the second provider request.
        // It cannot have received that request's gated answer, and restarting
        // must neither replay the command nor start the untouched Sessions.
        processes[1].kill().await.unwrap();
        pb.await_client_disconnect().await;
        pb.release_gate("host-result").await;
        assert!(processes[0].try_wait().unwrap().is_none());
        assert_eq!(processes[0].id(), a_pid);
        // A remains the same running attempt across B's death. Release its
        // provider barrier after proving that boundary: B's later process
        // restart and cold reconstruction must not consume A's finite
        // response-start deadline while this fixture withholds its answer.
        assert_eq!(checkpoint_state(&clients[0], &targets[0]).await["attempt"], a_before["attempt"]);
        pa.release_gate("host-result").await;
        let completed = settled(&clients[0], &targets[0]).await;
        assert_eq!(completed["attempt"]["phase"]["outcome"]["type"], "completed", "A's independent attempt must succeed: {}", completed["attempt"]["phase"]);
        assert_eq!(completed["model"]["configured"], serde_json::to_value(&selections[0]).unwrap());
        assert_eq!(completed["attempt"]["model"], a_before["attempt"]["model"]);
        assert!(completed["messages"].to_string().contains("result-a"), "completed A checkpoint: {completed:#}");
        assert!(!completed["messages"].to_string().contains("result-b"));
        assert_eq!(pa.requests().await.len(), 2);
        let (mut replacement, url, _stderr) = users[1].1.ws().await;
        let recovered = driver::websocket(&url).await;
        initialize_client(&recovered).await;
        assert_eq!(diagnostics(&recovered).await.loaded, 0);
        let MethodResult::Sessions { sessions, .. } = result(&recovered, Method::SessionList { query: None, offset: 0, limit: 32 }).await else { panic!("catalog") };
        assert_eq!(sessions.len(), 5);
        assert_eq!(diagnostics(&recovered).await.loaded, 0, "listing never starts Sessions");
        let cold = attach(&recovered, targets[1].session_id.clone(), 3).await;
        let after = checkpoint_state(&recovered, &cold).await;
        assert_eq!(after["model"]["configured"], serde_json::to_value(&selections[1]).unwrap(), "the active selection was durable before the process crash");
        assert!(after["messages"].to_string().contains("b:unset"));
        assert!(!after["messages"].to_string().contains("fake-b"));
        assert!(!after["messages"].to_string().contains("result-b"));
        assert_eq!(std::fs::read(users[1].1.root.path().join("a/host-effect")).unwrap(), b"x");
        assert_eq!(pb.requests().await.len(), 2);
        assert_eq!(checkpoint_state(&clients[0], &targets[0]).await["attempt"], completed["attempt"], "B's cold recovery cannot change A's completed attempt");
        // Changing A's source is confined to A; B's admitted composition stays.
        let authored = users[0].1.root.path().join("home/rustx/rustx.toml");
        let mut source: toml::Value = toml::from_str(&std::fs::read_to_string(&authored).unwrap()).unwrap();
        source["agent"]["plugins"]["todo"]["enabled"] = false.into();
        std::fs::write(authored, toml::to_string_pretty(&source).unwrap()).unwrap();
        assert_eq!(checkpoint_state(&recovered, &cold).await["effective_plugins"], after["effective_plugins"]);
        assert_eq!(checkpoint_state(&recovered, &cold).await["model"], after["model"]);
        assert_eq!(pb.requests().await.len(), 2, "source edits in A cannot trigger B replay");
        assert_eq!(pa.requests().await.len(), 2);
        recovered.close().await;
        terminate(&replacement); assert!(replacement.wait().await.unwrap().success());
        for client in clients { client.close().await; }
        terminate(&processes[0]); assert!(processes[0].wait().await.unwrap().success());
        pa.finish().await; pb.finish().await;
    })).await;
}

#[tokio::test]
async fn app_server_archive_stdio_download_works_without_web_or_runtime_attachment() {
    use app_server_conformance::AppServerConformanceDriver;
    use rustx::app_server::protocol::*;
    use std::io::Read;
    bounded(async {
        let f = Fixture::new().await;
        let mut child = f.command("stdio").spawn().unwrap();
        let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        let initialized = client
            .request(serde_json::from_str(INITIALIZE).unwrap())
            .await;
        assert!(matches!(initialized, Response::Success(_)));
        let response = client
            .request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(2),
                call: Method::SessionExportPrepare {
                    session_id: f.sessions[0].clone(),
                },
            })
            .await;
        let Response::Success(response) = response else {
            panic!("native archive preparation failed");
        };
        let MethodResult::SessionArchive { download } = response.result else {
            panic!("archive descriptor required");
        };
        let port = download.loopback_port.expect("owned stdio stream port");
        let url = format!("http://127.0.0.1:{port}{}", download.path);
        let response = reqwest::get(&url).await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "application/zip");
        let bytes = response.bytes().await.unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut manifest = String::new();
        zip.by_name("manifest.json")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(manifest["format"], "rustx-session-archive/v11");
        assert_eq!(
            manifest["schemas"],
            serde_json::json!({
                "journal": 7, "messages": 1, "surface": 1, "requests": 5, "projections":2,
                "generations": 2, "auxiliary_audit": 4,"inherited_auxiliary_audit":4, "publication_audits": 1,
                "inherited_responses": 2, "inherited_turns": 2,
            })
        );
        assert_eq!(manifest["session"]["id"], f.sessions[0].as_str());
        let conversation = manifest["conversations"][0]["conversation_id"]
            .as_str()
            .unwrap();
        let mut auxiliary_audit = String::new();
        zip.by_name(&format!("sessions/{conversation}/auxiliary_audit.jsonl"))
            .unwrap()
            .read_to_string(&mut auxiliary_audit)
            .unwrap();
        assert_eq!(auxiliary_audit, "");
        client.close().await;
        detach_then_shutdown(&mut child).await;
        assert!(child.wait().await.unwrap().success());
    })
    .await;
}

/// The complete owned-process lifecycle through the one external protocol
/// (relocated from the retired standalone launch process): `rustx app-server
/// --listen stdio` binds only host defaults (no path flags, empty `PATH`),
/// stdout starts with the protocol, the attached Session reports its model,
/// tools and safe catalog, a real turn reaches the provider with the catalog's
/// request parameters and credential, a workspace MCP definition the Agent
/// does not select stays inert, and shutdown is an explicit owner signal.
#[tokio::test]
async fn app_server_stdio_serves_a_real_conversation_from_host_defaults() {
    use rustx::app_server::protocol::*;
    bounded(Box::pin(async {
        let server = crate::common::FixtureServer::start(|_attempt, _head| {
            crate::common::sse_fixture("openai_chat", "plain_text.sse")
        })
        .await;
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(home.join("rustx")).unwrap();
        std::fs::write(
            home.join("rustx/rustx.toml"),
            format!(
                r#"agent_id = "agent-process"

[agent]
[agent.tools]
builtin = ["read", "write", "bash", "job_status"]
[agent.model]
model = "fixture/process-model"

[providers.fixture]
protocol = "openai_chat_completions"
base_url = "{}"
api_key = "$RUSTX_PROCESS_TEST_KEY"

[models."fixture/process-model"]
provider = "fixture"
id = "process-model"
context_window = 128000
max_output_tokens = 512
request_params = '{{"temperature":0.11}}'

[models."fixture/process-model".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
[models."fixture/process-model".compat]
chat_reasoning_replay = "omit"
"#,
                server.url("/v1")
            ),
        )
        .unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(workspace.join(".agents")).unwrap();
        let sentinel = root.path().join("mcp-started");
        std::fs::write(
            workspace.join(".agents/mcp.toml"),
            toml::to_string_pretty(&serde_json::json!({"mcp_servers": {"project": {
                "command": "touch", "args": [sentinel]
            }}}))
            .unwrap(),
        )
        .unwrap();
        let session = SessionController::open(&home.join("rustx/runtime"))
            .unwrap()
            .create_session(SessionPersistentState::from_input(
                &SessionConfigInput::new(workspace.clone()),
            ))
            .await
            .unwrap()
            .session
            .id;
        let mut child = Command::new(env!("CARGO_BIN_EXE_rustx"))
            .current_dir(&workspace)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "")
            .env("RUSTX_PROCESS_TEST_KEY", "process-secret")
            .args(["app-server", "--listen", "stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        initialize_client(&client).await;
        let target = attach(&client, session, 2).await;
        let snapshot: rustx::runtime_client::RuntimeClientSnapshot =
            serde_json::from_value(checkpoint_state(&client, &target).await).unwrap();
        let model = snapshot.model.as_ref().unwrap();
        assert_eq!(model.configured.model.to_string(), "fixture/process-model");
        assert_eq!(model.effective.as_ref().unwrap().context_window, 128_000);
        let names: Vec<_> = snapshot
            .capabilities
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        for expected in ["job_status", "read", "write", "bash"] {
            assert!(names.contains(&expected), "{expected} missing from {names:?}");
        }
        let MethodResult::DomainPage { page } = result(
            &client,
            Method::ModelCatalog {
                target: target.clone(),
            },
        )
        .await
        else {
            panic!("model catalog")
        };
        let models = crate::common::native_catalog_page(page).models;

        assert_eq!(
            models[0].credential_source,
            rustx::model::CredentialSourceView::Environment {
                variable: "RUSTX_PROCESS_TEST_KEY".to_owned()
            }
        );
        start_turn(&client, &target, "hello").await;
        let settled: rustx::runtime_client::RuntimeClientSnapshot =
            serde_json::from_value(settled(&client, &target).await).unwrap();
        assert!(settled.messages.iter().any(|message| matches!(
            message,
            rustx::message::types::MessageBlock::Assistant(assistant)
                if assistant.content.iter().any(|block| matches!(
                    block,
                    rustx::message::types::AssistantContentBlock::Text(text) if !text.text.is_empty()
                ))
        )));
        let body: serde_json::Value = serde_json::from_str(&server.request_body(0)).unwrap();
        assert_eq!(body["model"], "process-model");
        assert_eq!(body["temperature"], serde_json::json!(0.11));
        assert!(!sentinel.exists(), "an unselected MCP definition stays inert");
        client.close().await;
        terminate(&child);
        let output = child.wait_with_output().await.unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("process-secret"));
    }))
    .await;
}

/// A Session whose runtime composition fails — here its Workspace is gone —
/// is metadata-valid; only composition discovers the problem. That failed
/// attach changes no catalog byte, and every other Session still attaches
/// (relocated from the retired launch client's failed-startup regressions
/// onto App Server's composition path).
#[tokio::test]
async fn app_server_failed_attach_composition_leaves_the_catalog_untouched() {
    use rustx::app_server::protocol::*;
    bounded(async {
        let f = Fixture::new().await;
        let catalog_path = f.root.path().join("runtime/sessions/catalog.json");
        // The Session's Workspace no longer exists, so it cannot compose.
        std::fs::remove_dir(f.root.path().join("a")).unwrap();
        let before = std::fs::read(&catalog_path).unwrap();
        let mut child = f.command("stdio").spawn().unwrap();
        let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        initialize_client(&client).await;
        let response = rpc(
            &client,
            2,
            Method::SessionAttach {
                thread_id: app_server_conformance::thread_id(&client, f.sessions[0].clone()).await,
            },
        )
        .await;
        assert!(matches!(response, Response::Failure(_)), "{response:?}");
        assert_eq!(
            std::fs::read(&catalog_path).unwrap(),
            before,
            "a failed composition rewrote the catalog"
        );
        attach(&client, f.sessions[1].clone(), 3).await;
        client.close().await;
        detach_then_shutdown(&mut child).await;
        assert!(child.wait().await.unwrap().success());
    })
    .await;
}

// Issue #459: a persisted selection the current catalog no longer admits is
// the Session's own fact. Attach composes it unavailable and reports it,
// turns are refused explicitly, nothing is persisted in its place, and
// `session/setModel` corrects it.
#[tokio::test]
async fn app_server_retired_persisted_model_attaches_unavailable_without_rewriting_it() {
    use rustx::app_server::protocol::*;
    use rustx::model::catalog::ModelRef;
    use rustx::model::session::SessionModelConfig;
    bounded(async {
        let f = Fixture::new().await;
        let catalog_path = f.root.path().join("runtime/sessions/catalog.json");
        let mut document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&catalog_path).unwrap()).unwrap();
        let retired = SessionModelConfig::of(ModelRef::parse("local/retired-model").unwrap());
        document["sessions"][f.sessions[0].as_str()]["state"]["model"] =
            serde_json::to_value(&retired).unwrap();
        std::fs::write(&catalog_path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        let before = std::fs::read(&catalog_path).unwrap();
        let mut child = f.command("stdio").spawn().unwrap();
        let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        initialize_client(&client).await;
        let target = attach(&client, f.sessions[0].clone(), 2).await;
        let Response::Success(success) = rpc(
            &client,
            3,
            Method::ModelGet {
                target: target.clone(),
            },
        )
        .await
        else {
            panic!("model")
        };
        let MethodResult::DomainPage { page } = success.result else {
            panic!("bounded model read")
        };
        assert_eq!(page.domain, "model");
        assert_eq!(page.items.len(), 1);
        let model = model_value(&client, &target, &page).await;
        assert_eq!(model.configured, retired);
        assert!(
            model
                .unavailable
                .as_deref()
                .is_some_and(|reason| reason.contains("local/retired-model"))
        );
        assert!(model.effective.is_none(), "nothing is substituted");
        let refused = rpc(
            &client,
            4,
            Method::TurnStart {
                input_id: uuid::Uuid::new_v4().to_string(),
                target: target.clone(),
                content: vec![UserInputBlock::Text(rustx::message::content::TextBlock {
                    text: "refused".into(),
                })],
            },
        )
        .await;
        assert!(
            matches!(
                &refused,
                Response::Failure(Failure {
                    error: RpcError {
                        data: Some(ErrorData::ModelUnavailable { .. }),
                        ..
                    },
                    ..
                })
            ),
            "{refused:?}"
        );
        assert_eq!(
            std::fs::read(&catalog_path).unwrap(),
            before,
            "loading an unavailable selection persisted a replacement"
        );
        let model_set = rpc(
            &client,
            5,
            Method::ModelSet {
                recover_unavailable_auxiliary: None,
                target: target.clone(),
                config: Box::new(SessionModelConfig::of(
                    ModelRef::parse("local/test").unwrap(),
                )),
            },
        )
        .await;
        let Response::Success(success) = model_set else {
            panic!("model set failed: {model_set:?}")
        };
        assert!(matches!(success.result, MethodResult::ModelSet { .. }));
        let MethodResult::DomainPage { page } = result(
            &client,
            Method::ModelGet {
                target: target.clone(),
            },
        )
        .await
        else {
            panic!("bounded model read after committed receipt")
        };
        assert_eq!(page.domain, "model");
        assert_eq!(page.items.len(), 1);
        let model = model_value(&client, &target, &page).await;
        assert!(model.unavailable.is_none());
        client.close().await;
        detach_then_shutdown(&mut child).await;
        assert!(child.wait().await.unwrap().success());
    })
    .await;
}

/// Network authentication and OS-owned stdio enter the same ordinary API.
#[tokio::test]
async fn app_server_ordinary_clients_have_equal_delivery_eligibility() {
    async fn granted(client: &driver::Driver) -> bool {
        use rustx::app_server::protocol::*;
        let request: Request = serde_json::from_str(INITIALIZE).unwrap();
        let Response::Success(response) = rpc(client, 1, request.call).await else {
            panic!("initialize")
        };
        let MethodResult::Initialized { capabilities, .. } = response.result else {
            panic!("initialized")
        };
        capabilities.delivery_access
    }
    bounded(async {
        let f = Fixture::new().await;
        let mut child = f.command("stdio").spawn().unwrap();
        let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        assert!(granted(&client).await);
        client.close().await;
        detach_then_shutdown(&mut child).await;
        assert!(child.wait().await.unwrap().success());
        let mut child = f
            .command("ws://127.0.0.1:0")
            .arg("--token-file")
            .arg(f.root.path().join("token"))
            .spawn()
            .unwrap();
        let (url, _stderr) = websocket_startup(&mut child).await;
        for _ in 0..2 {
            let client = driver::websocket(&url).await;
            assert!(granted(&client).await);
            client.close().await;
        }
        // A private or obsolete second credential is never an ordinary fallback.
        assert!(
            driver::try_socket(
                &url,
                &format!(
                    "rustx.app-server.v81, {}, rustx-delivery-access.obsolete",
                    driver::credential_offer(driver::TOKEN)
                )
            )
            .await
            .is_err()
        );
        kill(
            Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()),
            Signal::SIGTERM,
        )
        .unwrap();
        assert!(child.wait().await.unwrap().success());
    })
    .await;
}

async fn await_original_stdio_completion(
    store: &rustx::durable::SqliteConversationStore,
    attempt: &rustx::runtime::identity::AttemptId,
) -> rustx::runtime::identity::MessageId {
    use rustx::events::types::RuntimeEvent;
    let mut after = 0;
    let mut canonical = None;
    loop {
        let page = store.read_events(Some(after), 32).unwrap();
        for envelope in page.events {
            after = envelope.sequence;
            if envelope.attempt_id.as_ref() != Some(attempt) {
                continue;
            }
            match envelope.event {
                RuntimeEvent::AssistantMessageCommitted { message_id } => {
                    assert!(canonical.replace((message_id, envelope.sequence)).is_none());
                }
                RuntimeEvent::AttemptCompleted { attempt_id, .. } => {
                    assert_eq!(&attempt_id, attempt);
                    let (message, committed_at) = canonical.expect("same-attempt canonical C");
                    assert!(committed_at < envelope.sequence, "C precedes terminal J");
                    return message;
                }
                RuntimeEvent::AttemptCancelled { .. }
                | RuntimeEvent::AttemptFailed { .. }
                | RuntimeEvent::AttemptLimitExceeded { .. }
                | RuntimeEvent::AttemptTimedOut { .. } => {
                    panic!("stdio EOF must not replace successful native settlement");
                }
                _ => {}
            }
        }
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn t10_stdio_cli_eof_preserves_gated_attempt_and_original_canonical_commit() {
    bounded(Box::pin(async {
        use rustx::{
            durable::SqliteConversationStore,
            events::types::RuntimeEvent,
            message::types::{AssistantContentBlock, MessageBlock},
            runtime::identity::AttemptId,
        };
        let Some(provider) = provider_emulator::ProviderEmulator::start("tui_multi_session").await
        else {
            return;
        };
        let f = Fixture::new().await;
        use_emulator(&f, &provider);
        let controller = SessionController::open(&f.root.path().join("runtime")).unwrap();
        let access = controller
            .acquire_session(&f.sessions[0], None)
            .await
            .unwrap();
        let database = access.database_path.clone();
        let conversation = access.node.conversation_id.clone();
        drop(access);
        drop(controller);
        let mut child = f.command("stdio").spawn().unwrap();
        let pid = child.id();
        let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        initialize_client(&client).await;
        let a = attach(&client, f.sessions[0].clone(), 2).await;
        let b = attach(&client, f.sessions[1].clone(), 3).await;
        assert_eq!(a.conversation_id, conversation);
        start_turn(&client, &a, "tui multi-session: session A long task").await;
        provider.await_gate("session-a-holding").await;
        let held = checkpoint_state(&client, &a).await;
        assert_eq!(held["attempt"]["phase"]["type"], "running");
        let attempt: AttemptId =
            serde_json::from_value(held["attempt"]["attempt_id"].clone()).unwrap();
        start_turn(&client, &b, "tui multi-session: session B quick task").await;
        assert_eq!(
            settled(&client, &b).await["attempt"]["phase"]["outcome"]["type"],
            "completed"
        );
        let store = SqliteConversationStore::open_existing(conversation, &database).unwrap();
        let starts = store
            .read_events(None, 32)
            .unwrap()
            .events
            .into_iter()
            .filter(|event| {
                event.attempt_id.as_ref() == Some(&attempt)
                    && matches!(event.event, RuntimeEvent::AttemptStarted { .. })
            })
            .count();
        assert_eq!(starts, 1, "one original admitted A attempt");
        // Stdio EOF: the driver closes only ChildStdin and still owns stdout.
        client.close_output().await;
        let mut diagnostics = BufReader::new(child.stderr.take().unwrap()).lines();
        loop {
            let line = diagnostics.next_line().await.unwrap().unwrap();
            if line.contains("stdio detached") {
                break;
            }
        }
        assert_eq!(child.id(), pid);
        assert!(
            child.try_wait().unwrap().is_none(),
            "EOF retains the process owner"
        );
        // No RPC, replacement attachment, or process-owner shutdown follows EOF.
        provider.release_gate("session-a-holding").await;
        let closing = await_original_stdio_completion(&store, &attempt).await;
        let MessageBlock::Assistant(message) = store.load_messages(&[closing]).unwrap().remove(0)
        else {
            panic!("original canonical Assistant");
        };
        let text = message
            .content
            .iter()
            .filter_map(|block| match block {
                AssistantContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(text, "A is working and has now finished");
        assert_eq!(provider.requests().await.len(), 2, "no disconnect retry");
        assert_eq!(child.id(), pid);
        assert!(
            child.try_wait().unwrap().is_none(),
            "native completion retains the owner"
        );
        drop(store);
        client.close().await;
        terminate(&child);
        assert!(child.wait().await.unwrap().success());
        provider.finish().await;
    }))
    .await;
}

async fn startup_source_field(
    client: &driver::Driver,
    view: &rustx::local_runtime::configuration::observation::BoundedSourceSettings,
    name: rustx::local_runtime::configuration::observation::SourceFieldName,
) -> serde_json::Value {
    use base64::Engine as _;
    use rustx::app_server::protocol::{Method, MethodResult};
    if let Some(source) = &view.inline {
        let value = serde_json::to_value(source).unwrap();
        let key = serde_json::to_value(name).unwrap();
        return value[key.as_str().unwrap()].clone();
    }
    let field = view
        .fields
        .iter()
        .find(|field| field.name == name)
        .expect("the native source advertised this field");
    if let Some(value) = &field.value {
        return value.clone();
    }
    let reference = field
        .reference
        .as_ref()
        .expect("deferred field has original source evidence");
    assert_eq!(reference.revision, view.revision);
    assert_eq!(reference.field, name);
    assert!(reference.item_index.is_none());
    assert!(
        reference.byte_length <= 524_288,
        "isolated fixture's selected source byte-cache bound"
    );
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let MethodResult::SourceValueChunk { chunk } = result(
            client,
            Method::SourcesValueRead {
                target: view.target.clone(),
                reference: reference.clone(),
                offset,
                max_bytes: 65_536,
            },
        )
        .await
        else {
            panic!("original source field chunk");
        };
        assert_eq!(chunk.reference, *reference);
        assert_eq!(chunk.offset, offset);
        let part = base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap();
        assert!(part.len() <= 65_536);
        let end = offset + u64::try_from(part.len()).unwrap();
        assert_eq!(chunk.end_offset, end);
        assert!(end <= reference.byte_length);
        assert!(end > offset || end == reference.byte_length);
        assert_eq!(
            chunk.next_offset,
            (end < reference.byte_length).then_some(end)
        );
        bytes.extend_from_slice(&part);
        offset = end;
        if end == reference.byte_length {
            break;
        }
    }
    serde_json::from_slice(&bytes).unwrap()
}

/// Issue #464: a real App Server repairs its bound User source before it
/// parses process policy or opens a transport, and the first client to
/// connect reads the receipt. Startup selects no Workspace (ISO-05): its
/// working directory, which here encloses the runtime root, is neither
/// scanned nor treated as one, and the backup is private.
#[tokio::test]
async fn app_server_startup_repairs_the_user_source_before_process_policy() {
    use rustx::app_server::protocol::{
        JsonRpcVersion, Method, MethodResult, Request, RequestId, Response,
    };
    use rustx::local_runtime::configuration::repair::{ConfigRepairChange, ConfigRepairStatus};
    use rustx::local_runtime::configuration::settings::SourceTarget;
    bounded(async {
        let f = Fixture::new().await;
        let authored = f.root.path().join("home/rustx/rustx.toml");
        let valid = std::fs::read_to_string(&authored).unwrap();
        // Strictly, every one of these rejects the document before policy
        // parsing; the process policy itself is authored and must survive.
        let original = format!(
            "schema_version = 8\nlegacy_root = true\n{valid}\n[app_server]\nmax_connections = 7\nfuture_isolation = \"none\"\n"
        );
        std::fs::write(&authored, &original).unwrap();
        let cwd_source = f.root.path().join("rustx.toml");
        let cwd_text = "[agent]\nlegacy_cwd = 1\n";
        std::fs::write(&cwd_source, cwd_text).unwrap();
        let mut child = f.command("stdio").spawn().unwrap();
        let driver = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        let call = |id: i64, call: Method| {
            let driver = &driver;
            async move {
                let Response::Success(success) = app_server_conformance::AppServerConformanceDriver::request(
                    driver,
                    Request { jsonrpc: JsonRpcVersion::V2, id: RequestId::Integer(id), call },
                )
                .await
                else {
                    panic!("request {id} failed")
                };
                success.result
            }
        };
        call(1, serde_json::from_str::<Request>(INITIALIZE).unwrap().call).await;
        let MethodResult::SourceSettings { projection } =
            call(2, Method::SourcesRead { target: SourceTarget::User }).await
        else {
            panic!("source settings")
        };
        let process_bindings = startup_source_field(&driver, &projection, SourceFieldName::ProcessBindings).await;
        assert_eq!(process_bindings["max_connections"], 7);
        let receipt: rustx::local_runtime::configuration::repair::ConfigRepairReceipt =
            serde_json::from_value(startup_source_field(&driver, &projection, SourceFieldName::UserRepair).await).unwrap();
        let user = startup_source_field(&driver, &projection, SourceFieldName::User).await;
        assert_eq!(receipt.status, ConfigRepairStatus::Repaired);
        assert_eq!(receipt.target, SourceTarget::User);
        assert!(receipt.backup_available);
        assert_eq!(
            receipt.changes,
            vec![
                ConfigRepairChange::UpdatedSchemaVersion { from: 8, to: 12 },
                ConfigRepairChange::RemovedField { object_path: String::new() },
                ConfigRepairChange::RemovedField {
                    object_path: "app_server".into()
                },
            ]
        );
        assert_eq!(receipt.after_revision.as_deref(),
            Some(user["revision"].as_str().expect("exact native User revision")));
        let repaired = std::fs::read_to_string(&authored).unwrap();
        assert!(!repaired.contains("legacy_root") && !repaired.contains("future_isolation"));
        assert!(repaired.contains("max_connections = 7"));
        let backups: Vec<_> = std::fs::read_dir(f.root.path().join("runtime/configuration-backups"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original);
        let mode = |path: &std::path::Path| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(mode(&backups[0]), 0o600);
        assert_eq!(mode(backups[0].parent().unwrap()), 0o700);
        assert_eq!(std::fs::read_to_string(&cwd_source).unwrap(), cwd_text);
        driver.close().await;
        detach_then_shutdown(&mut child).await;
        assert!(child.wait().await.unwrap().success());
    })
    .await;
}

/// An explicit `--config` whose basename rustX reserves for writer metadata
/// is refused at startup with its own diagnostic, never accepted and then
/// failing every source read; the same document as `user.toml` serves.
#[tokio::test]
async fn app_server_refuses_an_explicit_config_named_like_writer_metadata() {
    bounded(async {
        let f = Fixture::new().await;
        let document = std::fs::read(f.root.path().join("home/rustx/rustx.toml")).unwrap();
        let select = |name: &str| {
            let directory = f
                .root
                .path()
                .join(format!("selected-{}", name.trim_start_matches('.')));
            std::fs::create_dir_all(&directory).unwrap();
            let path = directory.join(name);
            std::fs::write(&path, &document).unwrap();
            path
        };
        let reserved = select(".rustx-locks");
        let output = f
            .command("stdio")
            .arg("--config")
            .arg(&reserved)
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(output.stdout, [] as [u8; 0]);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("reserves for writer metadata"), "{stderr}");
        assert!(std::fs::symlink_metadata(&reserved).unwrap().is_file());
        assert_eq!(std::fs::read(&reserved).unwrap(), document);
        assert_eq!(
            std::fs::read_dir(reserved.parent().unwrap())
                .unwrap()
                .count(),
            1
        );

        let ordinary = select("user.toml");
        let mut child = f
            .command("ws://127.0.0.1:0")
            .arg("--token-file")
            .arg(f.root.path().join("token"))
            .arg("--config")
            .arg(&ordinary)
            .spawn()
            .unwrap();
        let (_endpoint, _stderr) = websocket_startup(&mut child).await;
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    })
    .await;
}

async fn assert_reserved_config_rejected_before_startup(
    root: &std::path::Path,
    name: &str,
    verify_metadata_alias: bool,
) {
    use std::os::unix::fs::MetadataExt;

    let document = include_bytes!("../../examples/local-runtime/minimal/rustx.toml");
    let selected = root.join(name);
    std::fs::write(&selected, document).unwrap();
    // An uppercase argument naming a lowercase stored file is insufficient:
    // canonicalization can recover the lowercase name before the guard runs.
    assert_eq!(
        std::fs::read_dir(root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name(),
        name
    );
    let before = std::fs::symlink_metadata(&selected).unwrap();
    assert!(before.is_file());
    if verify_metadata_alias {
        let metadata_spelling = std::fs::metadata(root.join(".rustx-locks"))
            .expect("the lowercase metadata spelling must resolve to the selected file");
        assert_eq!(
            (metadata_spelling.dev(), metadata_spelling.ino()),
            (before.dev(), before.ino()),
            "the fixture must exercise a real filesystem alias for {name}"
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_rustx"))
        .current_dir(root)
        .env_clear()
        .env("HOME", root.join("home"))
        .args(["app-server", "--listen", "stdio", "--config"])
        .arg(&selected)
        .arg("--runtime-root")
        .arg(root.join("runtime"))
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(
        !output.status.success(),
        "reserved source {name} was accepted"
    );
    assert!(output.stdout.is_empty(), "no protocol service may start");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("reserves for writer metadata"), "{stderr}");
    assert!(stderr.contains("rename the file"), "{stderr}");
    let after = std::fs::symlink_metadata(&selected).unwrap();
    assert!(after.is_file());
    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    assert_eq!(std::fs::read(&selected).unwrap(), document);
    assert!(
        !root.join("runtime").exists(),
        "no runtime or repair backup"
    );
    assert!(!root.join("home").exists(), "no default runtime storage");
    assert_eq!(
        std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>(),
        vec![selected],
        "rejection must not create writer metadata, staging files or backups"
    );
}

#[tokio::test]
async fn app_server_reserves_writer_names_before_any_startup_side_effects() {
    bounded(async {
        for name in [
            ".rustx-locks",
            ".RUSTX-LOCKS",
            ".RuStX-LoCkS",
            ".rustx-loc\u{212a}s",
            ".ruſtx-locks",
            ".rustx-stage-candidate",
            ".RUSTX-STAGE-candidate",
            ".RuStX-StAgE-candidate",
            ".rustx-ﬆage-candidate",
        ] {
            let root = tempfile::tempdir().unwrap();
            assert_reserved_config_rejected_before_startup(root.path(), name, false).await;
        }
    })
    .await;
}

/// This test executes natively on macOS. Its fixture verifies case
/// insensitivity and creates an APFS image when the temporary volume is case
/// sensitive; it never silently skips.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn app_server_refuses_actual_reserved_aliases_on_case_insensitive_macos() {
    let volume = case_insensitive_volume::CaseInsensitiveVolume::new();
    bounded(async {
        for name in [
            ".RUSTX-LOCKS",
            ".RuStX-LoCkS",
            ".rustx-loc\u{212a}s",
            ".ruſtx-locks",
        ] {
            let root = tempfile::tempdir_in(volume.path()).unwrap();
            assert_reserved_config_rejected_before_startup(root.path(), name, true).await;
        }
    })
    .await;
}

/// The public named endpoint runs beside Agent work through both physical
/// carriers. Restart retains correlation but never reconstructs ephemeral text.
#[tokio::test]
async fn auxiliary_classification_real_stdio_and_websocket_preserve_primary_and_correlation() {
    async fn authorized_ws(
        f: &Fixture,
    ) -> (
        Child,
        driver::Driver,
        BufReader<tokio::process::ChildStderr>,
    ) {
        let mut child = f
            .command("ws://127.0.0.1:0")
            .arg("--token-file")
            .arg(f.root.path().join("token"))
            .spawn()
            .unwrap();
        let (url, stderr) = websocket_startup(&mut child).await;
        let client = driver::websocket(&url).await;
        (child, client, stderr)
    }
    bounded(Box::pin(async {
        use rustx::app_server::protocol::*;
        use rustx::runtime::auxiliary::{
            delivery::ConsumerResult,
            submission::{ClassificationInput, ClassificationSubmission, CorrelationId},
        };
        use rustx::runtime::conversation_runtime::auxiliary::AuxiliarySubmissionLookup;
        for socket in [false, true] {
            eprintln!("auxiliary transport socket={socket}: setup");
            let Some(provider) =
                provider_emulator::ProviderEmulator::start("auxiliary_classification_transport")
                    .await
            else {
                return;
            };
            let f = Fixture::new().await;
            use_emulator(&f, &provider);
            let (mut child, client, _stderr) = if socket {
                let (child, client, stderr) = authorized_ws(&f).await;
                (child, client, Some(stderr))
            } else {
                let mut child = f.command("stdio").spawn().unwrap();
                let client =
                    driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
                (child, client, None)
            };
            initialize_client(&client).await;
            let target = attach(&client, f.sessions[0].clone(), 2).await;
            start_turn(&client, &target, "Primary classification control").await;
            provider.await_gate("primary").await;
            // Provider flush is not the native publication commit. Observe the
            // gated primary fragment before comparing the complete Attempt.
            let before = loop {
                let state = checkpoint_state(&client, &target).await;
                if state["attempt"]["in_flight"]["blocks"]
                    .as_array()
                    .is_some_and(|blocks| {
                        blocks
                            .iter()
                            .any(|block| block["text"] == "Primary control")
                    })
                {
                    break state;
                }
                tokio::task::yield_now().await;
            };
            eprintln!("auxiliary transport socket={socket}: primary observed");
            let mut original = None;
            for valid in [true, false] {
                let submission = ClassificationSubmission {
                    correlation: CorrelationId::new(format!("classification-{valid}")).unwrap(),
                    input: ClassificationInput::Text {
                        text: format!("classify {} input", if valid { "valid" } else { "invalid" }),
                    },
                    labels: vec!["yes".into(), "no".into()],
                };
                let MethodResult::AuxiliarySubmitted { receipt } = result(
                    &client,
                    Method::AuxiliaryClassify {
                        target: target.clone(),
                        submission: submission.clone(),
                    },
                )
                .await
                else {
                    panic!("receipt");
                };
                eprintln!("auxiliary transport socket={socket}: receipt valid={valid}");
                let output = loop {
                    if let MethodResult::AuxiliaryResult {
                        result: Some(output),
                    } = result(
                        &client,
                        Method::AuxiliaryResult {
                            target: target.clone(),
                            call_id: receipt.call_id.clone(),
                        },
                    )
                    .await
                    {
                        break output;
                    }
                    tokio::task::yield_now().await;
                };
                assert_eq!(
                    output.execution,
                    Some(rustx::model::AuxiliaryCallOutcome::Completed)
                );
                if valid {
                    assert_eq!(
                        output.value,
                        ConsumerResult::Classification {
                            label: "yes".into()
                        }
                    );
                    original = Some((submission, receipt));
                } else {
                    assert_eq!(
                        output.value,
                        ConsumerResult::Rejected {
                            reason:
                                rustx::runtime::auxiliary::delivery::ConsumerFailure::InvalidOutput
                        }
                    );
                }
                let after = checkpoint_state(&client, &target).await;
                assert_eq!(after["messages"], before["messages"]);
                assert_eq!(after["attempt"], before["attempt"]);
            }
            eprintln!("auxiliary transport socket={socket}: classification complete");
            provider.release_gate("primary").await;
            settled(&client, &target).await;
            eprintln!("auxiliary transport socket={socket}: primary settled");
            client.close().await;
            terminate(&child);
            assert!(child.wait().await.unwrap().success());
            eprintln!("auxiliary transport socket={socket}: restart");
            let (mut replacement, current, _stderr) = authorized_ws(&f).await;
            initialize_client(&current).await;
            let target = attach(&current, f.sessions[0].clone(), 3).await;
            let (submission, original) = original.unwrap();
            let MethodResult::AuxiliarySubmitted { receipt } = result(
                &current,
                Method::AuxiliaryClassify {
                    target: target.clone(),
                    submission: submission.clone(),
                },
            )
            .await
            else {
                panic!("original receipt");
            };
            eprintln!("auxiliary transport socket={socket}: duplicate receipt");
            assert_eq!(receipt, original);
            assert!(matches!(
                result(
                    &current,
                    Method::AuxiliarySubmissionLookup {
                        target: target.clone(),
                        correlation: submission.correlation,
                        scope: Some(original.scope)
                    }
                )
                .await,
                MethodResult::AuxiliarySubmissionLookup {
                    lookup: AuxiliarySubmissionLookup::Known { .. }
                }
            ));
            assert!(matches!(
                result(
                    &current,
                    Method::AuxiliaryResult {
                        target,
                        call_id: original.call_id
                    }
                )
                .await,
                MethodResult::AuxiliaryResult { result: None }
            ));
            assert_eq!(provider.requests().await.len(), 3);
            current.close().await;
            terminate(&replacement);
            assert!(replacement.wait().await.unwrap().success());
            provider.finish().await;
        }
    }))
    .await;
}

async fn python_preparation_event(
    listener: &tokio::net::UnixListener,
) -> (tokio::net::UnixStream, serde_json::Value) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    (reader.into_inner(), serde_json::from_str(&line).unwrap())
}

#[tokio::test]
async fn workspace_python_survives_stdio_eof_and_joins_on_sigterm() {
    bounded(async {
        use std::os::unix::fs::PermissionsExt;
        use tokio::io::AsyncReadExt;
        let fixtures = [Fixture::new().await, Fixture::new().await];
        let gate_dir = tempfile::tempdir_in("/tmp").unwrap();
        let socket = gate_dir.path().join("gate");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let bin = staged_bin_dir();
        let server = stage_binary(
            bin.path(),
            "rustx",
            std::path::Path::new(env!("CARGO_BIN_EXE_rustx")),
        );
        let staged_client = std::path::Path::new(env!("CARGO_BIN_EXE_rustx"))
            .with_file_name("rustx-execution-client");
        let uv = bin.path().join("uv");
        std::fs::write(
            &uv,
            r"#!/usr/bin/python3
import json, os, socket, subprocess, sys
s = socket.socket(socket.AF_UNIX)
s.connect(os.environ['UV_TEST_GATE'])
if sys.argv[1] == '--version':
    s.sendall((json.dumps(['version', os.getcwd(), os.getpid()]) + '\n').encode())
    if s.recv(1) != b'1':
        sys.exit(2)
    print('uv 0.10.2')
else:
    subprocess.run(['/usr/bin/python3', '-m', 'venv', '--without-pip', sys.argv[-1]], check=True)
    s.sendall((json.dumps(['created', os.getcwd(), sys.argv[sys.argv.index('--directory') + 1]]) + '\n').encode())
",
        )
        .unwrap();
        std::fs::set_permissions(&uv, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Only the real Execution Client receives this fixture's uv. Each
        // trusted App Server launch enrolls one existing, exact Session.
        let quote = |value: &std::path::Path| {
            format!("'{}'", value.display().to_string().replace('\'', "'\\''"))
        };
        let wrapper = bin.path().join("rustx-execution-client");
        let scratch = gate_dir.path().join("scratch");
        let client_pids = gate_dir.path().join("client-pids");
        std::fs::create_dir(&scratch).unwrap();
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexport PATH={}:\"/usr/bin:/bin\" UV_TEST_GATE={} TMPDIR={}\nprintf '%s\\n' \"$$\" >> {}\nexec {} \"$@\"\n",
                quote(bin.path()),
                quote(&socket),
                quote(&scratch),
                quote(&client_pids),
                quote(&staged_client),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let workspaces = [
            fixtures[0].root.path().join("a"),
            fixtures[1].root.path().join("b"),
        ];
        let mut running = Vec::new();
        for (index, (fixture, workspace)) in fixtures.iter().zip(&workspaces).enumerate() {
            let session = &fixture.sessions[index];
            let mut child = fixture
                .command_with_program(&server, "stdio")
                .args([
                    "--local-execution-session",
                    session.as_str(),
                    "--local-execution-workspace",
                ])
                .arg(workspace)
                .spawn()
                .unwrap();
            let client = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
            initialize_client(&client).await;
            attach(&client, session.clone(), 2).await;
            running.push((child, client));
        }
        let mut versions = std::collections::BTreeMap::new();
        for _ in 0..2 {
            let (stream, event) = python_preparation_event(&listener).await;
            assert_eq!(event[0], "version");
            versions.insert(
                event[1].as_str().unwrap().to_owned(),
                (stream, event[2].as_i64().unwrap()),
            );
        }
        assert_eq!(versions.len(), 2);
        let mut diagnostic_pipes = Vec::new();
        for (child, client) in &mut running {
            client.close_output().await;
            let mut diagnostics = BufReader::new(child.stderr.take().unwrap()).lines();
            while !diagnostics
                .next_line()
                .await
                .unwrap()
                .expect("stdio detached")
                .contains("stdio detached")
            {}
            diagnostic_pipes.push(diagnostics);
        }
        let [a, b] = workspaces
            .each_ref()
            .map(|workspace| workspace.display().to_string());
        let (mut first, _) = versions.remove(&a).unwrap();
        let (mut pending, pending_pid) = versions.remove(&b).unwrap();
        first
            .write_all(b"1")
            .await
            .expect("EOF did not cancel preparation");
        let (_stream, event) = python_preparation_event(&listener).await;
        assert_eq!(event[0], "created");
        assert_eq!(event[1], a);
        let discovery = PathBuf::from(event[2].as_str().unwrap());
        assert!(discovery.starts_with(&scratch) && discovery.is_dir());
        let client_pids = std::fs::read_to_string(client_pids).unwrap();
        let client_pids = client_pids.lines().map(|line| Pid::from_raw(line.parse().unwrap())).collect::<Vec<_>>();
        assert_eq!(client_pids.len(), 2);
        for pid in &client_pids { assert_eq!(kill(*pid, None), Ok(())); }
        assert!(workspaces[0].join(".venv/pyvenv.cfg").is_file());
        assert!(!workspaces[1].join(".venv").exists());
        for (child, _) in &mut running {
            assert!(child.try_wait().unwrap().is_none());
            terminate(child);
        }
        assert_eq!(
            pending.read(&mut [0u8; 1]).await.unwrap(),
            0,
            "SIGTERM closes the held uv only after cancellation"
        );
        for (mut child, client) in running {
            assert!(child.wait().await.unwrap().success());
            client.close().await;
        }
        assert_eq!(
            kill(Pid::from_raw(i32::try_from(pending_pid).unwrap()), None),
            Err(nix::errno::Errno::ESRCH),
            "shutdown joined the uv process"
        );
        for pid in client_pids {
            assert_eq!(kill(pid, None), Err(nix::errno::Errno::ESRCH), "Server reaped owned Client {pid}");
        }
        assert!(!discovery.exists(), "orderly Client exit removed its discovery directory");
        drop(diagnostic_pipes);
    })
    .await;
}
