//! A real `rustx app-server --listen stdio` parent owning one Session.
//!
//! The parent is the product process: its Session is created by the durable
//! Session owner before launch (as a client would through `session/create`),
//! every control and read goes through the App Server protocol, and shutdown
//! is the owner's explicit SIGTERM drain — never an inferred meaning of EOF.
//!
//! Readiness is the protocol itself: a request blocks on its correlated
//! response. Wall-clock timeouts are outer liveness guards only.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use rustx::app_server::protocol::{
    AttachmentTarget, ClientIdentity, Failure, InitializeParams, JsonRpcVersion, Method,
    MethodResult, Notification, NotificationMethod, PresentationCapabilities, Request, RequestId,
    Response, UserInputBlock,
};
use rustx::local_runtime::SessionId;
use rustx::runtime_client::{RuntimeClientCursor, RuntimeClientSnapshot};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

/// The outer liveness guard of one process interaction.
const LIVENESS: std::time::Duration = std::time::Duration::from_mins(2);

enum Record {
    Response(Response),
    Notification(Notification),
}

pub(crate) struct Parent {
    pub(crate) child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: Lines<BufReader<tokio::process::ChildStdout>>,
    next_id: i64,
    pub(crate) session_id: SessionId,
    target: Option<AttachmentTarget>,
    notifications: VecDeque<NotificationMethod>,
}

/// The parent's process bindings: the authored user document, the runtime
/// root, and the Session workspace.
pub(crate) struct ParentBindings {
    pub(crate) config: PathBuf,
    pub(crate) runtime_root: PathBuf,
    pub(crate) workspace: PathBuf,
    pub(crate) home: PathBuf,
}

impl Parent {
    /// Launches the parent over `session`, creating a new Session with the
    /// bound workspace when none is given.
    pub(crate) async fn spawn(
        bindings: &ParentBindings,
        environment: &[(&str, &str)],
        session: Option<&SessionId>,
    ) -> Self {
        let session_id = match session {
            Some(session) => session.clone(),
            None => {
                rustx::local_runtime::session_controller::SessionController::open(
                    &bindings.runtime_root,
                )
                .expect("the durable Session owner opens")
                .create_session(
                    rustx::local_runtime::session::SessionPersistentState::from_input(
                        &rustx::local_runtime::SessionConfigInput::new(bindings.workspace.clone()),
                    ),
                )
                .await
                .expect("the parent Session is created")
                .session
                .id
            }
        };
        std::fs::create_dir_all(&bindings.home).expect("isolated home");
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rustx"));
        command
            .args(["app-server", "--listen", "stdio", "--config"])
            .arg(&bindings.config)
            .arg("--runtime-root")
            .arg(&bindings.runtime_root)
            .current_dir(&bindings.workspace)
            .env_clear()
            .env("HOME", &bindings.home)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, value) in environment {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("spawn the rustx App Server");
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout is piped")).lines();
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            next_id: 1,
            session_id,
            target: None,
            notifications: VecDeque::new(),
        }
    }

    /// Sends one request and returns its correlated outcome. Notifications
    /// that arrive first are retained in order for [`Self::next_event`].
    pub(crate) async fn call(&mut self, call: Method) -> Result<MethodResult, Failure> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::to_string(&Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call,
        })
        .expect("serialize the request");
        tokio::time::timeout(LIVENESS, async {
            let stdin = self.stdin.as_mut().expect("the parent transport is open");
            stdin
                .write_all(format!("{request}\n").as_bytes())
                .await
                .expect("write");
            stdin.flush().await.expect("flush");
            loop {
                match self.read_record().await {
                    Record::Response(Response::Success(success)) => {
                        assert_eq!(success.id, RequestId::Integer(id), "responses correlate");
                        return Ok(success.result);
                    }
                    Record::Response(Response::Failure(failure)) => {
                        assert_eq!(failure.id, Some(RequestId::Integer(id)));
                        return Err(failure);
                    }
                    Record::Notification(notification) => {
                        self.notifications.push_back(notification.notification);
                    }
                }
            }
        })
        .await
        .expect("the parent must answer")
    }

    async fn read_record(&mut self) -> Record {
        let Some(record) = self
            .stdout
            .next_line()
            .await
            .expect("read a protocol record")
        else {
            let stderr = self.stderr().await;
            panic!(
                "the parent closed stdout (status={:?})\nstderr:\n{stderr}",
                self.child.try_wait().expect("poll the parent")
            );
        };
        let value: serde_json::Value = serde_json::from_str(&record).unwrap_or_else(|error| {
            panic!("stdout carries protocol records only: {record} ({error})")
        });
        if value.get("id").is_some() {
            Record::Response(serde_json::from_value(value).expect("a correlated response"))
        } else {
            Record::Notification(serde_json::from_value(value).expect("a notification"))
        }
    }

    /// Initializes the connection and attaches the parent's Session.
    pub(crate) async fn attach(&mut self) -> RuntimeClientSnapshot {
        let initialized = self
            .call(Method::Initialize(InitializeParams {
                protocol_version: rustx::app_server::protocol::APP_SERVER_PROTOCOL_VERSION,
                client: ClientIdentity {
                    name: "subagent-boundary".into(),
                    version: "1".into(),
                },
                presentation: PresentationCapabilities {
                    images: false,
                    questionnaires: false,
                    reviews: false,
                },
            }))
            .await;
        assert!(
            matches!(initialized, Ok(MethodResult::Initialized { .. })),
            "{initialized:?}"
        );
        let attached = self
            .call(Method::SessionAttach {
                session_id: self.session_id.clone(),
                node_id: None,
            })
            .await;
        let Ok(MethodResult::Attached { target, .. }) = attached else {
            panic!("the parent Session attaches: {attached:?}");
        };
        self.target = Some(target);
        self.snapshot().await
    }

    pub(crate) fn target(&self) -> AttachmentTarget {
        self.target.clone().expect("the parent Session is attached")
    }

    pub(crate) async fn snapshot_with_cursor(
        &mut self,
    ) -> (RuntimeClientSnapshot, RuntimeClientCursor) {
        let target = self.target();
        match self
            .call(Method::SessionCheckpoint {
                target,
                trace_records: Vec::new(),
            })
            .await
        {
            Ok(MethodResult::Checkpoint { checkpoint }) => (checkpoint.snapshot, checkpoint.cursor),
            other => panic!("session/checkpoint must succeed: {other:?}"),
        }
    }

    pub(crate) async fn snapshot(&mut self) -> RuntimeClientSnapshot {
        self.snapshot_with_cursor().await.0
    }

    /// Reads original Tool definitions at the advertised capability cut.
    pub(crate) async fn capability_tools(
        &mut self,
    ) -> Vec<rustx::runtime_client::snapshot::RuntimeClientTool> {
        let Ok(MethodResult::DomainPage { page: header }) = self
            .call(Method::Capability {
                target: self.target(),
            })
            .await
        else {
            panic!("capability source header")
        };
        let domain = "capabilities.tools";
        let total = header
            .collections
            .iter()
            .find(|row| row.domain == domain)
            .expect("advertised Tool source")
            .total;
        let mut offset = 0;
        let mut tools = Vec::new();
        loop {
            let Ok(MethodResult::CheckpointPage { page }) = self
                .call(Method::ConversationCheckpointPage {
                    target: self.target(),
                    agent_id: None,
                    cut: header.read_cut.clone(),
                    domain: domain.into(),
                    offset,
                    limit: 32,
                })
                .await
            else {
                panic!("exact native Tool page")
            };
            assert_eq!(page.read_cut, header.read_cut);
            assert_eq!(page.domain, domain);
            assert_eq!(page.total, total);
            assert!(page.items.len() <= 32);
            let previous = offset;
            for item in page.items {
                assert_eq!(item.offset, offset);
                let value = match item.value.filter(|_| !item.deferred) {
                    Some(value) => value,
                    None => {
                        self.selected_value(None, &header.read_cut, domain, offset, &[])
                            .await
                    }
                };
                tools.push(
                    rustx::runtime_client::checkpoint::decode_domain_value(value)
                        .expect("original typed Tool definition"),
                );
                offset += 1;
            }
            assert_eq!(page.next_offset, (offset < total).then_some(offset));
            if page.next_offset.is_none() {
                assert_eq!(u64::try_from(tools.len()).unwrap(), total);
                return tools;
            }
            assert!(offset > previous, "Tool continuation advances");
        }
    }

    /// Reconstructs only an explicitly selected original value through bounded chunks.
    pub(crate) async fn selected_value(
        &mut self,
        agent_id: Option<rustx::runtime::identity::AgentId>,
        cut: &rustx::runtime_client::checkpoint::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
    ) -> serde_json::Value {
        use base64::Engine;
        let mut offset = 0;
        let mut bytes = Vec::new();
        let mut readable_end = None;
        loop {
            let result = self
                .call(Method::ConversationCheckpointValueRead {
                    target: self.target(),
                    agent_id: agent_id.clone(),
                    cut: cut.clone(),
                    domain: domain.into(),
                    item_offset,
                    path: path.to_vec(),
                    offset,
                    max_bytes: 65_536,
                })
                .await;
            let Ok(MethodResult::CheckpointValueChunk { chunk }) = result else {
                panic!("original selected value: {result:?}")
            };
            assert_eq!(chunk.cut, *cut);
            assert_eq!(chunk.domain, domain);
            assert_eq!(chunk.item_offset, item_offset);
            assert_eq!(chunk.path, path);
            assert_eq!(chunk.offset, offset);
            assert_eq!(
                *readable_end.get_or_insert(chunk.readable_end),
                chunk.readable_end
            );
            assert_eq!(
                chunk.representation,
                rustx::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1
            );
            let previous = offset;
            let part = base64::engine::general_purpose::STANDARD
                .decode(chunk.bytes_base64)
                .unwrap();
            assert!(part.len() <= 65_536);
            offset += u64::try_from(part.len()).unwrap();
            bytes.extend(part);
            assert_eq!(
                chunk.next_offset,
                (offset < chunk.readable_end).then_some(offset)
            );
            if chunk.next_offset.is_none() {
                assert_eq!(offset, chunk.readable_end);
                return serde_json::from_slice(&bytes).expect("complete original JSON value");
            }
            assert!(offset > previous, "original byte continuation advances");
        }
    }

    /// Starts one ordinary user turn.
    pub(crate) async fn start_turn(&mut self, text: &str) {
        let target = self.target();
        let started = self
            .call(Method::TurnStart {
                target,
                content: vec![UserInputBlock::Text(rustx::message::content::TextBlock {
                    text: text.to_owned(),
                })],
            })
            .await;
        assert!(
            matches!(started, Ok(MethodResult::InboundAccepted { .. })),
            "turn/start must be accepted: {started:?}"
        );
    }

    pub(crate) async fn subscribe(&mut self, after_cursor: RuntimeClientCursor) {
        let target = self.target();
        let subscribed = self
            .call(Method::SessionSubscribe {
                target,
                after_cursor,
            })
            .await;
        assert!(
            matches!(subscribed, Ok(MethodResult::Subscribed { .. })),
            "{subscribed:?}"
        );
    }

    /// The next runtime event of the attached Session, in stream order.
    pub(crate) async fn next_event(&mut self) -> rustx::runtime_client::event::RuntimeClientEvent {
        loop {
            let notification = match self.notifications.pop_front() {
                Some(notification) => notification,
                None => match self.read_record().await {
                    Record::Notification(notification) => notification.notification,
                    Record::Response(response) => panic!("uncorrelated response: {response:?}"),
                },
            };
            if let NotificationMethod::Event { event, .. } = notification {
                return *event;
            }
        }
    }

    /// The owner's explicit shutdown: SIGTERM drains the App Server; the
    /// process exits only after runtime settlement is proven or refused.
    pub(crate) async fn shutdown(mut self) -> (std::process::ExitStatus, String) {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(
                i32::try_from(self.child.id().expect("parent pid")).unwrap(),
            ),
            nix::sys::signal::Signal::SIGTERM,
        )
        .expect("SIGTERM the parent");
        let status = tokio::time::timeout(LIVENESS, self.child.wait())
            .await
            .expect("the parent must exit after its explicit drain")
            .expect("wait");
        drop(self.stdin.take());
        let stderr = self.stderr().await;
        (status, stderr)
    }

    /// Closes the protocol input without an owner shutdown.
    pub(crate) fn close_input(&mut self) {
        drop(self.stdin.take());
    }

    async fn stderr(&mut self) -> String {
        let mut stderr = String::new();
        if let Some(mut handle) = self.child.stderr.take() {
            use tokio::io::AsyncReadExt;
            let _ = handle.read_to_string(&mut stderr).await;
        }
        stderr
    }
}

impl ParentBindings {
    /// The conventional layout of this target's fixtures under one root.
    pub(crate) fn under(root: &Path, workspace: &Path) -> Self {
        Self {
            config: root.join("rustx.toml"),
            runtime_root: root.join("private"),
            workspace: workspace.to_path_buf(),
            home: root.join("host"),
        }
    }
}
