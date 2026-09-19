//! Real standalone process, pipe and network boundaries; shared semantic authority.
use futures_util::{SinkExt, StreamExt};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
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
                "--tool-calls",
                "true",
                "--reasoning",
                "false",
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
        source["agent"].as_table_mut().unwrap().insert("tools".into(), toml::Value::try_from(serde_json::json!({"builtin": ["read", "write", "edit", "glob", "grep", "bash", "execution"]})).unwrap());
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
            sessions.push(
                controller
                    .create_session(SessionPersistentState::from_input(
                        &SessionConfigInput::new(workspace),
                    ))
                    .await
                    .unwrap()
                    .session
                    .id,
            );
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_rustx"));
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
    async fn ws(&self) -> (Child, String) {
        let mut child = self
            .command("ws://127.0.0.1:0")
            .arg("--token-file")
            .arg(self.root.path().join("token"))
            .spawn()
            .unwrap();
        let line = BufReader::new(child.stderr.as_mut().unwrap())
            .lines()
            .next_line()
            .await
            .unwrap()
            .unwrap();
        (
            child,
            line.strip_prefix("rustx app-server listening ")
                .unwrap_or_else(|| panic!("server startup: {line}"))
                .to_owned(),
        )
    }
}
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_mins(1), future)
        .await
        .expect("outer liveness guard")
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

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol_version":9,"client":{"name":"boundary","version":"1"},"presentation":{"images":false,"questionnaires":false,"reviews":false}}}"#;

#[tokio::test]
async fn app_server_stdio_real_process_shared_conformance() {
    bounded(async {
        let f = Fixture::new().await;
        let mut child = f.command("stdio").spawn().unwrap();
        let driver = driver::jsonl(child.stdout.take().unwrap(), child.stdin.take().unwrap());
        app_server_conformance::representative_scenario(&driver, f.sessions.clone()).await;
        driver.close().await;
        detach_then_shutdown(&mut child).await;
        assert!(child.wait().await.unwrap().success());
    })
    .await;
}

#[tokio::test]
async fn app_server_websocket_real_process_shared_conformance_and_listener_survives() {
    bounded(async {
        let f = Fixture::new().await;
        let (mut child, url) = f.ws().await;
        let client = driver::websocket(&url).await;
        app_server_conformance::representative_scenario(&client, f.sessions.clone()).await;
        client.close().await;
        let mut replacement = driver::socket(&url).await;
        replacement.send(INITIALIZE.into()).await.unwrap();
        assert_eq!(
            json_response(&mut replacement).await["result"]["protocol_version"],
            9
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

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
async fn json_response(socket: &mut Socket) -> serde_json::Value {
    let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
        panic!("text response");
    };
    serde_json::from_str(text.as_str()).unwrap()
}

#[tokio::test]
async fn app_server_websocket_authentication_framing_and_protocol_errors() {
    bounded(async {
        let f = Fixture::new().await;
        let (mut child, url) = f.ws().await;
        let old_offer = format!("rustx.app-server.v7, rustx-token.{}", driver::TOKEN);
        for offer in [
            None,
            Some("rustx.app-server.v9"),
            Some(old_offer.as_str()),
            Some("rustx.app-server.v9, rustx-token.wrong"),
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
                r#"{"jsonrpc":"2.0","id":3,"method":"session/create","params":{}}"#,
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
        let reply: serde_json::Value =
            serde_json::from_str(&output.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(reply["id"], 1);
        drop(input);
        detach_then_shutdown(&mut child).await;
        assert!(child.wait().await.unwrap().success());
        assert!(output.next_line().await.unwrap().is_none());
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
            assert!(output.stdout.is_empty());
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
                assert!(output.stdout.is_empty());
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
                let client = if ws {
                    let line = BufReader::new(child.stderr.take().unwrap())
                        .lines()
                        .next_line()
                        .await
                        .unwrap()
                        .unwrap();
                    driver::websocket(line.strip_prefix("rustx app-server listening ").unwrap())
                        .await
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
                                session_id: session_id.clone(),
                                node_id: None,
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
                assert!(output.stdout.is_empty());
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
    let Response::Success(response) = rpc(
        client,
        id,
        Method::SessionAttach {
            session_id: session,
            node_id: None,
        },
    )
    .await
    else {
        panic!("attach")
    };
    let MethodResult::Attached { target, .. } = response.result else {
        panic!("attached")
    };
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
            let (mut child, url) = f.ws().await;
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
                rpc(&client, 4, Method::TurnStart { target: a, content }).await,
                Response::Success(_)
            ));
            let (mut request, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            assert!(
                request.read(&mut bytes).await.unwrap() > 0,
                "provider request really started"
            );
            // Hold durable terminal publication at a real boundary. Native drain
            // still owns the attempt; refusing new input needs no storage lock.
            let database = rusqlite::Connection::open(database).unwrap();
            database.execute_batch("BEGIN IMMEDIATE").unwrap();
            terminate(&child);
            let mut lines = BufReader::new(child.stderr.as_mut().unwrap()).lines();
            let line = lines.next_line().await.unwrap().unwrap();
            assert!(
                line.contains("Draining; new semantic admission closed"),
                "{line}"
            );
            let Response::Failure(response) = rpc(
                &client,
                5,
                Method::SessionAttach {
                    session_id: f.sessions[0].clone(),
                    node_id: None,
                },
            )
            .await
            else {
                panic!("post-drain admission")
            };
            assert_eq!(response.error.data, Some(ErrorData::ServerDraining));
            assert!(
                child.try_wait().unwrap().is_none(),
                "settlement not yet proven"
            );
            if forced {
                let output = child.wait_with_output().await.unwrap();
                assert_eq!(output.status.code(), Some(3));
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("runtime settlement unproven")
                );
                database.execute_batch("ROLLBACK").unwrap();
            } else {
                database.execute_batch("ROLLBACK").unwrap();
                assert!(child.wait().await.unwrap().success());
            }
            drop(request);
            client.close().await;
            let (mut replacement, url) = f.ws().await;
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

async fn snapshot(
    client: &driver::Driver,
    target: &rustx::app_server::protocol::AttachmentTarget,
) -> serde_json::Value {
    use rustx::app_server::protocol::*;
    let MethodResult::Snapshot { snapshot, .. } = result(
        client,
        Method::SessionSnapshot {
            trace_records: vec![],
            target: target.clone(),
        },
    )
    .await
    else {
        panic!("snapshot")
    };
    serde_json::to_value(snapshot).unwrap()
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
        let value = snapshot(client, target).await;
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
        let (mut child, url) = f.ws().await;
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
                    settings: SessionPersistentState::from_input(&SessionConfigInput::new(
                        f.root.path().join(cwd),
                    )),
                },
            )
            .await
            else {
                panic!("create")
            };
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
            let NotificationMethod::Event { target, event, .. } =
                client.next_notification().await.notification
            else {
                panic!("unexpected invalidation")
            };
            assert!(target == a || target == b);
            let rendered = serde_json::to_string(&event).unwrap();
            assert!(!rendered.contains(if target == a {
                "B answered"
            } else {
                "A is working"
            }));
            if target == b
                && matches!(
                    *event,
                    rustx::runtime_client::event::RuntimeClientEvent::AttemptSettled { .. }
                )
            {
                break;
            }
        }
        assert_eq!(
            snapshot(&client, &a).await["attempt"]["phase"]["type"],
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
        assert!(
            a_done["pending_interactions"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(!a_done["transcript"].to_string().contains("cancelled"));
        assert_eq!(provider.requests().await.len(), 2, "no transport retry");
        for (target, cwd) in [(&resumed, "a"), (&b, "b")] {
            let MethodResult::Settings { settings, .. } = result(
                &reconnected,
                Method::SettingsRead {
                    session_id: target.session_id.clone(),
                },
            )
            .await
            else {
                panic!("settings")
            };
            assert_eq!(settings.cwd, f.root.path().join(cwd));
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
        let (mut child, url) = f.ws().await;
        let client = driver::websocket(&url).await;
        initialize_client(&client).await;
        let a = attach(&client, f.sessions[0].clone(), 2).await;
        let a_initial = snapshot(&client, &a).await;
        // Explicit model choice belongs to the durable Session; omitted
        // extension defaults remain current source authority on cold load.
        let MethodResult::Settings {
            revision,
            mut settings,
            ..
        } = result(
            &client,
            Method::SettingsRead {
                session_id: a.session_id.clone(),
            },
        )
        .await
        else {
            panic!("settings")
        };
        settings.model =
            Some(serde_json::from_value(a_initial["model"]["configured"].clone()).unwrap());
        result(
            &client,
            Method::SettingsReplace {
                session_id: a.session_id.clone(),
                expected_revision: revision,
                settings: settings.clone(),
            },
        )
        .await;
        start_turn(&client, &a, "tui multi-session: session A long task").await;
        provider.await_gate("session-a-holding").await;
        let admitted = snapshot(&client, &a).await["attempt"].clone();
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
            snapshot(&client, &a).await["effective_plugins"],
            a_initial["effective_plugins"]
        );
        assert_eq!(
            snapshot(&client, &a).await["attempt"]["model"],
            admitted["model"]
        );
        let b = attach(&client, f.sessions[1].clone(), 3).await;
        let b_initial = snapshot(&client, &b).await;
        assert!(b_initial["effective_plugins"]["todo"].is_null());
        assert!(!a_initial["effective_plugins"]["todo"].is_null());
        start_turn(&client, &b, "tui multi-session: session B quick task").await;
        provider.await_gate("session-b-holding").await;
        provider.release_gate("session-a-holding").await;
        let before = settled(&client, &a).await;
        // Composition reconstruction is a process-owner operation here. The
        // internal manager suite proves targeted replacement independently.
        assert_eq!(
            snapshot(&client, &b).await["effective_plugins"],
            b_initial["effective_plugins"]
        );
        assert_eq!(
            snapshot(&client, &b).await["attempt"]["phase"]["type"],
            "running"
        );
        provider.release_gate("session-b-holding").await;
        settled(&client, &b).await;
        client.close().await;
        terminate(&child);
        assert!(child.wait().await.unwrap().success());
        let (mut child, url) = f.ws().await;
        let client = driver::websocket(&url).await;
        initialize_client(&client).await;
        let cold = attach(&client, a.session_id.clone(), 4).await;
        assert_eq!(cold.conversation_id, a.conversation_id);
        let after = snapshot(&client, &cold).await;
        assert!(after["effective_plugins"]["todo"].is_null());
        assert_eq!(
            after["model"]["configured"],
            a_initial["model"]["configured"]
        );
        assert_eq!(after["messages"], before["messages"]);
        assert_eq!(diagnostics(&client).await.loaded, 1);
        let MethodResult::Settings {
            settings: persisted,
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
        assert_eq!(persisted, settings);
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
    bounded(async {
        use rustx::app_server::protocol::*;
        let Some(pa) = provider_emulator::ProviderEmulator::start("app_server_user_a").await else { return };
        let Some(pb) = provider_emulator::ProviderEmulator::start("app_server_user_b").await else { return };
        let users = [("a", Fixture::new().await, &pa), ("b", Fixture::new().await, &pb)];
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
            source.as_table_mut().unwrap().insert("native_tools".into(), toml::Value::try_from(serde_json::json!({"bash": {"approval": "never"}})).unwrap());
            std::fs::write(&authored, toml::to_string_pretty(&source).unwrap()).unwrap();
            let mut text = std::fs::read_to_string(&authored).unwrap();
            write!(text, "\n[environment]\nRUSTX_HOST_MARKER = \"{identity}\"\n").unwrap();
            std::fs::write(authored, text).unwrap();
            let mut child = user.command("ws://127.0.0.1:0")
                .arg("--config").arg(config.join("rustx.toml"))
                .arg("--token-file").arg(user.root.path().join("token"))
                .env("TEST_KEY", format!("fake-{identity}"))
                .env("RUSTX_HOST_MARKER", identity).spawn().unwrap();
            let line = BufReader::new(child.stderr.as_mut().unwrap()).lines().next_line().await.unwrap().unwrap();
            let endpoint = line.strip_prefix("rustx app-server listening ").unwrap();
            let client = driver::websocket(endpoint).await;
            initialize_client(&client).await;
            let MethodResult::SessionTransition { session, .. } = result(&client, Method::SessionCreate {
                settings: SessionPersistentState::from_input(&SessionConfigInput::new(user.root.path().join("a")))
            }).await else { panic!("created") };
            result(&client, Method::SessionName { session_id: session.id.clone(), name: format!("user-{identity}-private") }).await;
            let target = attach(&client, session.id, 2).await;
            start_turn(&client, &target, &format!("host user {identity}")).await;
            provider.await_gate("host-result").await;
            assert!(provider.requests().await.iter().all(|request| request["credentialHeaders"].as_array().unwrap().iter().any(|header| header == "authorization")));
            let requests = provider.requests().await;
            assert!(requests[0]["body"].to_string().contains(&format!("host-{identity}-only resource")));
            assert!(!requests[0]["body"].to_string().contains(if *identity == "a" { "host-b-only" } else { "host-a-only" }));
            assert_eq!(std::fs::read(user.root.path().join("a/host-effect")).unwrap(), b"x");
            let projected = snapshot(&client, &target).await;
            assert!(projected["messages"].to_string().contains(&format!("{identity}:unset")));
            assert!(projected["messages"].to_string().contains(user.root.path().join("a").to_str().unwrap()));
            processes.push(child); clients.push(client); targets.push(target);
        }
        assert_ne!(processes[0].id(), processes[1].id());
        // IDs are scoped to a user's root (and may have identical spellings).
        // Even that spelling on the other process reads only its own metadata.
        for (index, client) in clients.iter().enumerate() {
            let other = 1 - index;
            let list = result(client, Method::SessionList { query: Some(format!("user-{}-private", users[other].0)), offset: 0, limit: 32 }).await;
            let MethodResult::Sessions { sessions, .. } = list else { panic!("list") };
            assert!(sessions.is_empty());
            let Response::Failure(failure) = rpc(client, 100, Method::SessionRead { session_id: targets[other].session_id.clone() }).await else { panic!("foreign Session must be absent") };
            assert_eq!(failure.error.data, Some(ErrorData::UnknownSession { session_id: targets[other].session_id.clone() }));
        }
        // Prove absent identities fail at the other root too, in both directions.
        for (index, count) in [(0, 1), (1, 2)] {
            let mut id = targets[index].session_id.clone();
            for _ in 0..count {
                let MethodResult::SessionTransition { session, .. } = result(&clients[index], Method::SessionCreate {
                    settings: SessionPersistentState::from_input(&SessionConfigInput::new(users[index].1.root.path().join("b")))
                }).await else { panic!("private Session") };
                id = session.id;
            }
            let Response::Failure(failure) = rpc(&clients[1 - index], 12, Method::SessionRead { session_id: id.clone() }).await else { panic!("foreign Session readable") };
            assert_eq!(failure.error.data, Some(ErrorData::UnknownSession { session_id: id }));
        }
        for (index, client) in clients.iter().enumerate() {
            use app_server_conformance::AppServerConformanceDriver;
            let mut model: rustx::model::session::SessionModelConfig = serde_json::from_value(snapshot(client, &targets[index]).await["model"]["configured"].clone()).unwrap();
            model.request_params.insert("temperature".into(), serde_json::json!(0.42));
            result(client, Method::ModelSet { target: targets[index].clone(), config: Box::new(model) }).await;
            loop {
                let notification = client.next_notification().await;
                let NotificationMethod::Event { target, event, .. } = notification.notification else { panic!("unexpected invalidation") };
                assert_eq!(target, targets[index]);
                assert!(!serde_json::to_string(&event).unwrap().contains(&format!("{}:unset", users[1 - index].0)));
                if matches!(*event, rustx::runtime_client::event::RuntimeClientEvent::SessionModelChanged { .. }) { break; }
            }
        }
        let a_pid = processes[0].id();
        let a_before = snapshot(&clients[0], &targets[0]).await;
        // B dies after its tool result has entered the second provider request.
        // It cannot have received that request's gated answer, and restarting
        // must neither replay the command nor start the untouched Sessions.
        processes[1].kill().await.unwrap();
        pb.await_client_disconnect().await;
        pb.release_gate("host-result").await;
        assert!(processes[0].try_wait().unwrap().is_none());
        assert_eq!(processes[0].id(), a_pid);
        let (mut replacement, url) = users[1].1.ws().await;
        let recovered = driver::websocket(&url).await;
        initialize_client(&recovered).await;
        assert_eq!(diagnostics(&recovered).await.loaded, 0);
        let MethodResult::Sessions { sessions, .. } = result(&recovered, Method::SessionList { query: None, offset: 0, limit: 32 }).await else { panic!("catalog") };
        assert_eq!(sessions.len(), 5);
        assert_eq!(diagnostics(&recovered).await.loaded, 0, "listing never starts Sessions");
        let cold = attach(&recovered, targets[1].session_id.clone(), 3).await;
        let after = snapshot(&recovered, &cold).await;
        assert!(after["messages"].to_string().contains("b:unset"));
        assert!(!after["messages"].to_string().contains("result-b"));
        assert_eq!(std::fs::read(users[1].1.root.path().join("a/host-effect")).unwrap(), b"x");
        assert_eq!(pb.requests().await.len(), 2);
        assert_eq!(snapshot(&clients[0], &targets[0]).await["attempt"], a_before["attempt"]);
        // Changing A's source is confined to A; B's admitted composition stays.
        let authored = users[0].1.root.path().join("home/rustx/rustx.toml");
        let mut source: toml::Value = toml::from_str(&std::fs::read_to_string(&authored).unwrap()).unwrap();
        source["agent"]["plugins"]["todo"]["enabled"] = false.into();
        std::fs::write(authored, toml::to_string_pretty(&source).unwrap()).unwrap();
        assert_eq!(snapshot(&recovered, &cold).await["effective_plugins"], after["effective_plugins"]);
        assert_eq!(snapshot(&recovered, &cold).await["model"], after["model"]);
        assert_eq!(pb.requests().await.len(), 2, "source edits in A cannot trigger B replay");
        pa.release_gate("host-result").await;
        let completed = settled(&clients[0], &targets[0]).await;
        assert!(completed["messages"].to_string().contains("result-a"));
        assert!(!completed["messages"].to_string().contains("result-b"));
        assert_eq!(pa.requests().await.len(), 2);
        recovered.close().await;
        terminate(&replacement); assert!(replacement.wait().await.unwrap().success());
        for client in clients { client.close().await; }
        terminate(&processes[0]); assert!(processes[0].wait().await.unwrap().success());
        pa.finish().await; pb.finish().await;
    }).await;
}
