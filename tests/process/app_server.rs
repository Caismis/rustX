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
use std::{path::PathBuf, process::Stdio};
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
        let config = home.join(".config/rustx");
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
        let host =
            rustx::local_runtime::HostEnvironment::from_paths(root.path().into(), home, None, None)
                .unwrap();
        let controller = SessionController::open(&root.path().join("runtime")).unwrap();
        let mut sessions = Vec::new();
        for name in ["a", "b"] {
            let workspace = root.path().join(name);
            std::fs::create_dir(&workspace).unwrap();
            rustx::local_runtime::launch::change_trust(
                &rustx::local_runtime::LaunchRequest {
                    workspace: Some(workspace.clone()),
                    ..Default::default()
                },
                &host,
                rustx::local_runtime::TrustAction::Grant,
            )
            .unwrap();
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

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol_version":1,"client":{"name":"boundary","version":"1"},"presentation":{"images":false,"questionnaires":false,"reviews":false}}}"#;

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
            1
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
        for offer in [
            None,
            Some("rustx.app-server.v1"),
            Some("rustx.app-server.v1, rustx-token.wrong"),
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
                "settings" => f.root.path().join("home/.config/rustx/settings.toml"),
                "catalog" => f.root.path().join("home/.config/rustx/models.toml"),
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
async fn app_server_explicit_user_settings_are_authoritative_for_both_transports() {
    bounded(async {
        use app_server_conformance::AppServerConformanceDriver;
        use rustx::app_server::protocol::*;
        for ws in [false, true] {
            for explicit in [false, true] {
                let f = Fixture::new().await;
                let ambient = f.root.path().join("home/.config/rustx");
                let selected = f.root.path().join("selected");
                std::fs::create_dir(&selected).unwrap();
                std::fs::copy(ambient.join("models.toml"), selected.join("models.toml")).unwrap();
                let settings = std::fs::read_to_string(ambient.join("settings.toml")).unwrap();
                // Different model catalogs prove source ownership and that authored
                // relative bindings use the selected document parent, not launch cwd.
                std::fs::write(
                    selected.join("settings.toml"),
                    format!("models = \"models.toml\"\n{settings}"),
                )
                .unwrap();
                std::fs::write(
                    ambient.join("settings.toml"),
                    format!("models = \"ambient-models.toml\"\n{settings}"),
                )
                .unwrap();
                let catalog = std::fs::read_to_string(ambient.join("models.toml")).unwrap();
                std::fs::write(
                    ambient.join("ambient-models.toml"),
                    catalog.replace("context_window = 128000", "context_window = 64000"),
                )
                .unwrap();
                let mut command = f.command(if ws { "ws://127.0.0.1:0" } else { "stdio" });
                if explicit {
                    command.args(["--user-settings", "selected/settings.toml"]);
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
async fn app_server_explicit_user_settings_fail_before_readiness() {
    bounded(async {
        let f = Fixture::new().await;
        for contents in [None, Some("secret-sentinel invalid TOML")] {
            let path = f.root.path().join("selected.toml");
            if let Some(contents) = contents {
                std::fs::write(&path, contents).unwrap();
            }
            for listen in ["stdio", "ws://127.0.0.1:0"] {
                let mut command = f.command(listen);
                command.arg("--user-settings").arg(&path);
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
                let settings = f.root.path().join("home/.config/rustx/settings.toml");
                let mut text = std::fs::read_to_string(&settings).unwrap();
                text.push_str("\n[app_server]\nshutdown_deadline_ms = 2000\n");
                std::fs::write(settings, text).unwrap();
            }
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let models = f.root.path().join("home/.config/rustx/models.toml");
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
            let content = vec![rustx::message::types::UserContentBlock::Text(
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
