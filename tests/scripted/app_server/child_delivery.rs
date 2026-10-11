// Exact native child ownership and physical-root admission, with existing fences.
use crate::durable::ConversationStore;
use crate::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
    ToolCallOccurrenceRef, ToolMessageBlock,
};
use crate::runtime::identity::{AgentId, MessageId, ToolCallId, ToolId};
use crate::tools::session_files::{SessionFileReadFailure, SessionFileReference};
use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};
use futures_util::{SinkExt, StreamExt};

struct ChildDelivery {
    fixture: Fixture,
    connection: AppServerConnection,
    target: AttachmentTarget,
    agent: AgentId,
    workspace: std::path::PathBuf,
    store: crate::durable::SqliteConversationStore,
    message: MessageId,
}
impl ChildDelivery {
    async fn new(isolated: bool) -> Self {
        let fixture = Fixture::with_tool(Some("delivery-child")).await;
        let workspace = &fixture.workspaces[0];
        let profile = workspace.join(".agents/agents/reviewer.toml");
        std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
        std::fs::write(&profile, format!("description='Delivery child'\ninstructions='Inspect native delivery'\n[worktree]\nenabled={isolated}\n")).unwrap();
        std::fs::write(
            workspace.join("rustx.toml"),
            "[agent]\nagents=['reviewer']\n",
        )
        .unwrap();
        if isolated {
            for args in [
                vec!["init", "--quiet"],
                vec!["add", "."],
                vec![
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.test",
                    "commit",
                    "--quiet",
                    "-m",
                    "Freeze child source",
                ],
            ] {
                let output = std::process::Command::new("git")
                    .args(args)
                    .current_dir(workspace)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        let connection = AppServerConnection::with_delivery_access(
            fixture.host.clone(),
            tokio_util::sync::CancellationToken::new(),
        );
        initialize(&connection).await;
        let target = attach(&connection, &fixture, 0).await;
        fixture.gates[0].release();
        call(
            &connection,
            8100,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("request-A"),
            },
        )
        .await;
        fixture.gates[1].wait_entered().await;
        // The child is physically parked; the parent's unrelated continuation
        // must settle before file reads assert that no model request is created.
        await_attempt_settled(&connection, &target.session_id).await;
        let runtime = fixture
            .manager
            .configuration_runtime(&fixture.sessions[0].id)
            .unwrap();
        let registry = runtime.subagent_registry().unwrap();
        let (agent, _) = registry.list_agents(1).agents.into_iter().next().unwrap();
        let workspace = registry.agent_workspace(&agent.agent_id).unwrap();
        assert_eq!(
            matches!(
                workspace.isolation,
                crate::runtime::workspace::WorkspaceIsolation::GitWorktree(_)
            ),
            isolated
        );
        assert_eq!(
            workspace.logical_workspace == fixture.workspaces[0],
            !isolated
        );
        // Fixture commits use a writer; the production inspection accessor is
        // intentionally read-only and must retain that contract.
        let store = crate::durable::SqliteConversationStore::open(
            agent.conversation_id.clone(),
            &registry
                .agent_artifact_root(&agent.agent_id)
                .unwrap()
                .join("conversation.sqlite"),
        )
        .unwrap();
        std::fs::write(
            workspace.logical_workspace.join("child-report.md"),
            DELIVERED,
        )
        .unwrap();
        let file = crate::tools::session_files::declare(
            &workspace.logical_workspace,
            store.conversation_id(),
            "child-report.md",
            None,
        )
        .unwrap();
        let message = commit_delivery(&store, "child-present", file);
        Self {
            fixture,
            connection,
            target,
            agent: agent.agent_id,
            workspace: workspace.logical_workspace,
            store,
            message,
        }
    }
    fn host_read(&self) -> crate::app_server::product_host::FileRead {
        crate::app_server::product_host::FileRead {
            target: self.target.clone(),
            source: crate::app_server::product_host::ReadSource::SessionFile {
                agent_id: Some(self.agent.clone()),
                message_id: self.message.clone(),
                delivery_index: 0,
            },
            roots: vec![self.fixture.workspaces[0].clone()],
        }
    }
    fn native_read(&self, id: i64) -> Request {
        Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call: Method::AgentDeliveryRead {
                target: self.target.clone(),
                agent_id: self.agent.clone(),
                message_id: self.message.clone(),
                delivery_index: 0,
            },
        }
    }
    fn evidence(&self) -> (u64, usize, usize, crate::runtime::identity::SubagentId) {
        let runtime = self
            .fixture
            .manager
            .configuration_runtime(&self.fixture.sessions[0].id)
            .unwrap();
        (
            self.store.presentation_frontier().unwrap(),
            self.fixture.provider.request_bodies().len(),
            runtime.subagent_registry().unwrap().list_agents(32).matched,
            runtime
                .subagent_registry()
                .unwrap()
                .list_agents(32)
                .agents
                .into_iter()
                .find(|(agent, _)| agent.agent_id == self.agent)
                .unwrap()
                .1
                .subagent_id,
        )
    }
    async fn close(self) {
        self.fixture.gates[1].release();
        // A durable ordinary Agent can remain resident after its Turn ends.
        // Session unload owns process cancellation and physical settlement.
        self.connection.close();
        self.fixture.close().await;
    }
}
fn commit_delivery(
    store: &crate::durable::SqliteConversationStore,
    id: &str,
    file: SessionFileReference,
) -> MessageId {
    let assistant = MessageId::new(format!("{id}-proposal"));
    let message = MessageId::new(id);
    let call = ToolCall {
        id: ToolCallId::new(id),
        tool_id: ToolId::new("tool-present"),
        name: "present".into(),
        arguments: serde_json::json!({"files":[{"path":file.path}]}),
    };
    let event = |suffix: &str, event| crate::events::types::RuntimeEventEnvelope {
        schema_version: 1,
        event_id: crate::runtime::identity::EventId::new(format!("{id}-{suffix}")),
        sequence: 0,
        conversation_id: store.conversation_id().clone(),
        attempt_id: None,
        turn_id: None,
        timestamp: chrono::Utc::now(),
        event,
    };
    store
        .append_canonical_with_event(
            &MessageBlock::Assistant(AssistantMessageBlock {
                id: assistant.clone(),
                content: vec![AssistantContentBlock::ToolCall(call.clone())],
            }),
            event(
                "proposal",
                crate::events::types::RuntimeEvent::AssistantMessageCommitted {
                    message_id: assistant.clone(),
                },
            ),
        )
        .unwrap();
    let result = ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![],
        duration_ms: 0,
        exit_code: None,
        artifacts: vec![],
        deliveries: vec![file],
        truncation: None,
        workflow: None,
        managed_output: None,
    };
    store
        .append_canonical_with_event(
            &MessageBlock::Tool(ToolMessageBlock {
                id: message.clone(),
                occurrence: ToolCallOccurrenceRef::new(assistant, ContentBlockIndex::new(0)),
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id,
                result,
            }),
            event(
                "result",
                crate::events::types::RuntimeEvent::ToolMessageCommitted {
                    message_id: message.clone(),
                    tool_call_id: call.id,
                },
            ),
        )
        .unwrap();
    message
}
fn unavailable() -> ErrorData {
    ErrorData::SessionFileRead {
        reason: SessionFileReadFailure::Unavailable,
    }
}
fn unauthorized() -> ErrorData {
    ErrorData::SessionFileRead {
        reason: SessionFileReadFailure::Unauthorized,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_delivery_shared_owner_and_product_host_registration_are_exact() {
    Box::pin(bounded(async {
        let child = ChildDelivery::new(false).await;
        let host = TrustedFileHost::new(&child.fixture).await;
        let before = child.evidence();
        let response = host.response(child.host_read()).await;
        assert_eq!(delivered_bytes(&response), DELIVERED);
        let mut unregistered = child.host_read();
        unregistered.roots = vec![child.fixture.workspaces[1].clone()];
        assert_eq!(host.rejected(unregistered).await, unauthorized());
        let mut foreign = child.host_read();
        if let crate::app_server::product_host::ReadSource::SessionFile { agent_id, .. } =
            &mut foreign.source
        {
            *agent_id = Some(AgentId::new("foreign-domain-agent"));
        }
        assert_eq!(host.rejected(foreign).await, unavailable());
        let other_target = attach(&child.connection, &child.fixture, 1).await;
        let mut other_domain = child.host_read();
        other_domain.target = other_target;
        assert_eq!(host.rejected(other_domain).await, unavailable());
        let mut wrong_index = child.host_read();
        if let crate::app_server::product_host::ReadSource::SessionFile { delivery_index, .. } =
            &mut wrong_index.source
        {
            *delivery_index = 1;
        }
        assert_eq!(host.rejected(wrong_index).await, unavailable());
        let mut wrong_message = child.host_read();
        if let crate::app_server::product_host::ReadSource::SessionFile { message_id, .. } =
            &mut wrong_message.source
        {
            *message_id = MessageId::new("parent-or-other-child");
        }
        assert_eq!(host.rejected(wrong_message).await, unavailable());
        let mut file = crate::tools::session_files::declare(
            &child.workspace,
            child.store.conversation_id(),
            "child-report.md",
            None,
        )
        .unwrap();
        file.scope.conversation_id = crate::runtime::identity::ConversationId::generate();
        let wrong_scope = commit_delivery(&child.store, "foreign-scope", file);
        let before = (
            child.store.presentation_frontier().unwrap(),
            before.1,
            before.2,
            before.3,
        );
        let mut read = child.host_read();
        if let crate::app_server::product_host::ReadSource::SessionFile { message_id, .. } =
            &mut read.source
        {
            *message_id = wrong_scope;
        }
        assert_eq!(host.rejected(read).await, unavailable());
        std::fs::remove_file(child.workspace.join("child-report.md")).unwrap();
        assert_eq!(
            host.rejected(child.host_read()).await,
            ErrorData::SessionFileRead {
                reason: SessionFileReadFailure::Missing
            }
        );
        assert_eq!(
            child.evidence(),
            before,
            "reads never activate Agents, request models or commit history"
        );
        host.close().await;
        child.close().await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_delivery_product_host_rejects_isolated_root_but_native_owner_can_read() {
    Box::pin(bounded(async {
        let child = ChildDelivery::new(true).await;
        let host = TrustedFileHost::new(&child.fixture).await;
        let before = child.evidence();
        assert_eq!(host.rejected(child.host_read()).await, unauthorized());
        let mut directly_registered = child.host_read();
        directly_registered.roots.push(child.workspace.clone());
        assert_eq!(
            host.rejected(directly_registered).await,
            unauthorized(),
            "the private lane has no isolated-child delegation policy"
        );
        assert_eq!(
            delivered_bytes(
                &child
                    .connection
                    .handle_request(child.native_read(8110))
                    .await
            ),
            DELIVERED
        );
        assert_eq!(child.evidence(), before);
        host.close().await;
        child.close().await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_delivery_cancellation_revocation_and_detachment_hold_until_physical_settlement() {
    Box::pin(bounded(async {
        let child = ChildDelivery::new(false).await;
        let host = TrustedFileHost::new(&child.fixture).await;
        let before = child.evidence();
        let probe = child.fixture.host.file_read_probe();
        let permits = child.fixture.host.file_reads();
        for (id, gate) in [
            (8120, probe.before_open.clone()),
            (8121, probe.before_bytes.clone()),
        ] {
            let release = gate.arm_scoped();
            let connection = child.connection.clone();
            let request = child.native_read(id);
            let pending = tokio::spawn(async move { connection.handle_request(request).await });
            tokio::task::spawn_blocking(move || gate.wait_entered())
                .await
                .unwrap();
            assert!(cancel_delivery(&child.connection, id).await);
            assert_eq!(
                permits.available_permits(),
                crate::tools::session_files::SESSION_FILE_MAX_READS - 1
            );
            drop(release);
            failed_with(&pending.await.unwrap(), &ErrorData::DeliveryCancelled);
            assert_eq!(
                permits.available_permits(),
                crate::tools::session_files::SESSION_FILE_MAX_READS
            );
        }
        for revoked in ["registration", "credential", "attachment"] {
            let mut completed = probe.completed.subscribe();
            probe.completed.send_replace(None);
            let release = probe.before_bytes.arm_scoped();
            let mut socket = host.open(FILE_HOST_TOKEN).await.unwrap();
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::to_string(&child.host_read()).unwrap().into(),
                ))
                .await
                .unwrap();
            let gate = probe.before_bytes.clone();
            tokio::task::spawn_blocking(move || gate.wait_entered())
                .await
                .unwrap();
            if revoked == "attachment" {
                call(
                    &child.connection,
                    8122,
                    Method::SessionDetach {
                        target: child.target.clone(),
                    },
                )
                .await;
            } else if revoked == "credential" {
                child.fixture.host.bind_product_host(None);
            } else {
                socket.close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame { code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal, reason: "".into() })).await.unwrap();
                let authority = probe.authority.lock().unwrap().clone().unwrap();
                authority.cancelled().await;
                assert_eq!(permits.available_permits(), crate::tools::session_files::SESSION_FILE_MAX_READS - 1);
            }
            drop(release);
            completed.wait_for(Option::is_some).await.unwrap();
            assert_eq!(*completed.borrow(), Some(false));
            while let Some(frame) = socket.next().await {
                if let Ok(tokio_tungstenite::tungstenite::Message::Text(text)) = frame {
                    assert!(matches!(
                        serde_json::from_str::<Response>(&text).unwrap(),
                        Response::Failure(_)
                    ));
                }
            }
            assert_eq!(
                permits.available_permits(),
                crate::tools::session_files::SESSION_FILE_MAX_READS
            );
            if revoked == "credential" {
                child.fixture.host.bind_product_host(Some(
                    crate::app_server::transport::websocket::Credential::new(
                        FILE_HOST_TOKEN.into(),
                    )
                    .unwrap(),
                ));
            }
        }
        assert_eq!(child.evidence(), before);
        host.close().await;
        child.close().await;
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_delivery_mapping_change_and_root_replacement_cannot_retarget_a_read() {
    Box::pin(bounded(async {
        let child = ChildDelivery::new(false).await;
        let host = TrustedFileHost::new(&child.fixture).await;
        let before = child.evidence();
        let controller = &child.fixture.manager.sessions;
        let (revision, settings) = controller
            .read_settings(&child.target.session_id)
            .await
            .unwrap();
        let probe = child.fixture.host.file_read_probe();
        let release = probe.before_bytes.arm_scoped();
        let connection = child.connection.clone();
        let request = child.native_read(8130);
        let pending = tokio::spawn(async move { connection.handle_request(request).await });
        let gate = probe.before_bytes.clone();
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        let mut replacement = settings.clone();
        replacement.cwd = child.fixture.workspaces[1].clone();
        let revision = controller
            .replace_settings(&child.target.session_id, revision, replacement)
            .await
            .unwrap();
        drop(release);
        failed_with(&pending.await.unwrap(), &unavailable());
        let mut both = child.host_read();
        both.roots.push(child.fixture.workspaces[1].clone());
        assert_eq!(host.rejected(both).await, unavailable());
        controller
            .replace_settings(&child.target.session_id, revision, settings)
            .await
            .unwrap();
        assert_eq!(
            delivered_bytes(&host.response(child.host_read()).await),
            DELIVERED
        );
        // Same path spelling is insufficient: the committed reference owns its inode.
        let original = child.workspace.with_extension("original");
        std::fs::rename(&child.workspace, &original).unwrap();
        std::fs::create_dir(&child.workspace).unwrap();
        std::fs::write(child.workspace.join("child-report.md"), b"substitute").unwrap();
        assert_eq!(host.rejected(child.host_read()).await, unavailable());
        std::fs::remove_dir_all(&child.workspace).unwrap();
        std::fs::rename(&original, &child.workspace).unwrap();
        assert_eq!(child.evidence(), before);
        host.close().await;
        child.close().await;
    }))
    .await;
}
