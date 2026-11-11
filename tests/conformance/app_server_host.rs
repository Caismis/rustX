//! Broadcast Host Tools over real `WebSockets`, catalog, adapter, HTTP and parser.
use crate::{common::provider_emulator::ProviderEmulator, launch_fixture::LaunchFixture};
use rustx::{
    app_server::{host::AppServerHost, protocol::*, transport::websocket},
    credentials::CredentialSnapshot,
    durable::{CanonicalStore, SqliteConversationStore},
    local_runtime::{
        HostEnvironment, StartupSession,
        app_server_policy::AppServerPolicy,
        composition::LocalRuntimeDependencies,
        session::SessionPersistentState,
        session_controller::SessionController,
        session_runtime_manager::{RuntimeResidencyPolicy, SessionRuntimeManager},
    },
    message::{content::TextBlock, types::MessageBlock},
    tools::{ToolExecutionStatus, host::HostToolDefinition},
};
use tokio_util::sync::CancellationToken;

#[path = "../support/app_server_peer.rs"]
mod peer;
use peer::Peer;

fn definition(namespace: &str) -> HostToolDefinition {
    HostToolDefinition {
        namespace: Some(namespace.into()),
        name: "lookup".into(),
        description: "Look up one application record".into(),
        input_schema: serde_json::json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false}),
    }
}

struct ServerFixture {
    root: tempfile::TempDir,
    workspace: std::path::PathBuf,
    sessions: SessionController,
    manager: SessionRuntimeManager,
    host: AppServerHost,
    endpoint: String,
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl ServerFixture {
    async fn start(provider: &ProviderEmulator) -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let config = root.path().join("rustx.toml");
        std::fs::write(&config,toml::to_string_pretty(&serde_json::json!({
                // Host broadcast conformance is not a permission test.
                "approval_mode": "full_access",
                "agent":{"model":{"model":"emulator/chat-model"},"tools":{"builtin":[]}},
                "providers":{"emulator":{"protocol":"openai_chat_completions","base_url":provider.openai_base_url(),"api_key":"$HOST_TEST_KEY"}},
                "models":{"emulator/chat-model":{"provider":"emulator","id":"chat-model","context_window":128_000,"max_output_tokens":1024,"capabilities":{"input_modalities":["text"],"output_modalities":["text"]},"compat":{"chat_reasoning_replay":"omit"}}}
            })).unwrap()).unwrap();
        let paths = LaunchFixture {
            config,
            workspace: workspace.clone(),
            runtime_root: root.path().join("runtime"),
            startup_session: StartupSession::Empty,
        };
        let environment =
            HostEnvironment::from_paths(workspace.clone(), root.path().join("home")).unwrap();
        let (configuration, _) = paths.request().session_input(&environment).unwrap();
        let sessions = SessionController::open(&paths.runtime_root).unwrap();
        let manager = SessionRuntimeManager::new(
            sessions.clone(),
            configuration,
            CredentialSnapshot::new([("HOST_TEST_KEY".into(), "fixture".into())]),
            LocalRuntimeDependencies::default(),
            RuntimeResidencyPolicy {
                max_resident_runtimes: 8,
                idle_grace_ms: 300_000,
            },
        )
        .unwrap();
        let host = AppServerHost::new(manager.clone(), AppServerPolicy::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let server = tokio::spawn(websocket::serve(
            listener,
            host.clone(),
            websocket::Credential::new("trusted-host-test".into()).unwrap(),
            shutdown.clone(),
        ));
        Self {
            root,
            workspace,
            sessions,
            manager,
            host,
            endpoint,
            shutdown,
            server,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn three_clients_broadcast_first_response_error_and_invalid_payload_commit_once() {
    tokio::time::timeout(std::time::Duration::from_secs(180),async {
        for winner in ["success","error","invalid"] {
            let Some(provider)=ProviderEmulator::start("app_server_host_broadcast").await else {return;};
            let ServerFixture { root,workspace,sessions,manager,host,endpoint,shutdown,server }=ServerFixture::start(&provider).await;
            let mut first=Peer::connect(&endpoint).await; let mut second=Peer::connect(&endpoint).await; let mut generic=Peer::connect(&endpoint).await;
            let definitions=vec![definition("business"),definition("inventory")];
            let MethodResult::SessionTransition {thread,session,..}=first.rpc(Method::SessionCreate {dynamic_tools:definitions.clone(),submission:None,settings:SessionPersistentState {cwd:workspace.clone(),model:None}}).await else {panic!("Thread start")};
            sessions.rename_session(&session.id,"Host conformance").await.unwrap();
            assert_eq!(thread.dynamic_tools,definitions);
            let a=first.follow(thread.id.clone()).await; let b=second.follow(thread.id.clone()).await; let c=generic.follow(thread.id.clone()).await;
            assert_eq!(a.runtime_incarnation,b.runtime_incarnation); assert_eq!(b.runtime_incarnation,c.runtime_incarnation);
            let input_id=uuid::Uuid::new_v4().to_string();
            let MethodResult::TurnInputAccepted {turn_id,disposition:TurnInputDisposition::Started,..}=first.rpc(Method::TurnStart {input_id:input_id.clone(),target:a.clone(),content:vec![UserInputBlock::Text(TextBlock {text:"shared host request".into()})]}).await else {panic!("native Attempt receipt")};
            provider.await_gate("host-call").await; provider.release_gate("host-call").await;
            let ServerMessage::Request(call)=first.callback().await else {panic!("Host call")};
            let ServerMessage::Request(copy)=second.callback().await else {panic!("broadcast")};
            let ServerMessage::Request(unhandled)=generic.callback().await else {panic!("generic subscriber")};
            assert_eq!(call,copy); assert_eq!(copy,unhandled);
            let MethodResult::Thread {thread:working}=generic.rpc(Method::ThreadRead{thread_id:thread.id.clone()}).await else {panic!("Thread status")};
            assert_eq!(working.status,rustx::app_server::thread_status::ThreadStatus::Active{active_flags:vec![]},"Host callbacks are ordinary active execution without human waits");
            let ServerRequestMethod::ToolCall(tool)=&call.call else {panic!("tool callback")};
            assert_eq!(tool.thread_id,thread.id); assert_eq!(tool.turn_id,turn_id); assert_eq!(tool.namespace.as_deref(),Some("business")); assert_eq!(tool.tool,"lookup"); assert_eq!(tool.arguments,serde_json::json!({"key":"invoice"}));
            // Both application handlers have observed/executed the call before
            // either answer is selected. Generic clients deliberately ignore it.
            let effects = root.path().join("application-effects");
            for (handler, request) in [("first", &call), ("second", &copy)] {
                use std::io::Write as _;
                let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&effects).unwrap();
                writeln!(file, "{handler}:{:?}", request.id).unwrap();
            }
            assert_eq!(std::fs::read_to_string(&effects).unwrap().lines().count(), 2);
            let selected=match winner {
                "success"=>serde_json::json!({"jsonrpc":"2.0","id":call.id,"result":{"success":true,"contentItems":[{"type":"inputText","text":"authoritative-host-result"}]}}),
                "error"=>serde_json::json!({"jsonrpc":"2.0","id":call.id,"error":{"code":-32000,"message":"remote result unknown"}}),
                _=>serde_json::json!({"jsonrpc":"2.0","id":call.id,"result":"invalid domain payload"}),
            };
            first.reply(selected).await;
            let ServerMessage::Resolved {params,..}=second.callback().await else {panic!("resolution")}; assert_eq!(params.request_id,call.id);
            second.reply(serde_json::json!({"jsonrpc":"2.0","id":call.id,"result":{"success":true,"contentItems":[{"type":"inputText","text":"late-result"}]}})).await;
            first.settled(&turn_id).await;
            assert!(matches!(generic.callback().await,ServerMessage::Resolved {..}));
            assert!(matches!(first.rpc(Method::InboundSubmissionLookup {target:a.clone(),input_id}).await,MethodResult::InboundSubmission {acceptance:Some(_)}));
            let access=sessions.acquire_session(&session.id,Some(&thread.node_id)).await.unwrap();
            let store=SqliteConversationStore::open_existing(thread.id.clone(),&access.database_path).unwrap();
            let canonical=store.load_canonical().unwrap(); let results:Vec<_>=canonical.iter().filter_map(|message|if let MessageBlock::Tool(tool)=message {Some(&tool.result)}else{None}).collect();
            assert_eq!(results.len(),1,"one canonical ToolResult");
            assert_eq!(matches!(results[0].status,ToolExecutionStatus::Success),winner=="success");
            if winner!="success" {assert!(matches!(results[0].status,ToolExecutionStatus::OutcomeUnknown {..}));}
            assert_eq!(provider.requests().await.len(),2,"one model continuation");
            drop(store); drop(access); drop(first); drop(second); drop(generic); shutdown.cancel();server.await.unwrap().unwrap();
            manager.unload(&thread.id).await.unwrap(); drop(host); drop(manager); provider.finish().await;
        }
    }).await.expect("Host callback conformance completes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn shared_start_steer_sparse_queue_stale_controls_and_disconnect_use_one_native_writer() {
    tokio::time::timeout(std::time::Duration::from_secs(180), async {
        let Some(provider) = ProviderEmulator::start("app_server_shared_control").await else {
            return;
        };
        let f = ServerFixture::start(&provider).await;
        let mut first = Peer::connect(&f.endpoint).await;
        let mut second = Peer::connect(&f.endpoint).await;
        let mut third = Peer::connect(&f.endpoint).await;
        let MethodResult::SessionTransition {
            thread, session, ..
        } = first
            .rpc(Method::SessionCreate {
                dynamic_tools: Vec::new(),
                submission: None,
                settings: SessionPersistentState {
                    cwd: f.workspace.clone(),
                    model: None,
                },
            })
            .await
        else {
            panic!("Thread")
        };
        f.sessions
            .rename_session(&session.id, "Shared control")
            .await
            .unwrap();
        let a = first.follow(thread.id.clone()).await;
        let b = second.follow(thread.id.clone()).await;
        let c = third.follow(thread.id.clone()).await;
        assert_eq!(a.runtime_incarnation, b.runtime_incarnation);
        assert_eq!(b.runtime_incarnation, c.runtime_incarnation);
        let text = |value: &str| vec![UserInputBlock::Text(TextBlock { text: value.into() })];
        let MethodResult::TurnInputAccepted {
            turn_id: first_turn,
            disposition: TurnInputDisposition::Started,
            ..
        } = first
            .rpc(Method::TurnStart {
                target: a.clone(),
                input_id: "start-one".into(),
                content: text("start-one"),
            })
            .await
        else {
            panic!("Started")
        };
        provider.await_gate("active-request").await;
        let MethodResult::TurnInputAccepted {
            turn_id,
            disposition: TurnInputDisposition::Steered,
            ..
        } = second
            .rpc(Method::TurnStart {
                target: b.clone(),
                input_id: "start-two".into(),
                content: text("start-two"),
            })
            .await
        else {
            panic!("Steered")
        };
        assert_eq!(turn_id, first_turn);
        let MethodResult::InboundAccepted {
            message_id: queued, ..
        } = third
            .rpc(Method::InboundQueue {
                target: c.clone(),
                input_id: "queued-next".into(),
                content: text("queued-next"),
            })
            .await
        else {
            panic!("queued")
        };
        let MethodResult::TurnInputAccepted {
            turn_id,
            disposition: TurnInputDisposition::Steered,
            ..
        } = first
            .rpc(Method::TurnSteer {
                target: a.clone(),
                expected_turn_id: first_turn.clone(),
                input_id: "steer-after-queue".into(),
                content: text("steer-after-queue"),
            })
            .await
        else {
            panic!("exact steer")
        };
        assert_eq!(turn_id, first_turn);
        provider.release_gate("active-request").await;
        provider.await_gate("steered-request").await;
        let MethodResult::Checkpoint { checkpoint } = third
            .rpc(Method::SessionCheckpoint {
                target: c.clone(),
                trace_records: Vec::new(),
            })
            .await
        else {
            panic!("checkpoint")
        };
        assert_eq!(
            checkpoint
                .snapshot
                .inbound
                .pending
                .iter()
                .map(|item| &item.message.id)
                .collect::<Vec<_>>(),
            vec![&queued],
            "explicit queue survives sparse steering adoption"
        );
        provider.release_gate("steered-request").await;
        provider.await_gate("queued-request").await;
        let MethodResult::Checkpoint { checkpoint } = third
            .rpc(Method::SessionCheckpoint {
                target: c.clone(),
                trace_records: Vec::new(),
            })
            .await
        else {
            panic!("checkpoint")
        };
        let next_turn = checkpoint.snapshot.attempt.unwrap().attempt_id.unwrap();
        assert_ne!(next_turn, first_turn);
        assert!(matches!(
            second
                .response(Method::TurnCancel {
                    target: b.clone(),
                    turn_id: first_turn.clone()
                })
                .await,
            Response::Failure(_)
        ));
        assert!(matches!(
            second
                .response(Method::TurnSteer {
                    target: b.clone(),
                    expected_turn_id: first_turn,
                    input_id: "stale".into(),
                    content: text("must-not-appear")
                })
                .await,
            Response::Failure(_)
        ));
        drop(first);
        provider.release_gate("queued-request").await;
        second.settled(&next_turn).await;
        assert_eq!(provider.requests().await.len(), 3);
        for input_id in ["start-one", "start-two", "queued-next", "steer-after-queue"] {
            assert!(matches!(
                third
                    .rpc(Method::InboundSubmissionLookup {
                        target: c.clone(),
                        input_id: input_id.into()
                    })
                    .await,
                MethodResult::InboundSubmission {
                    acceptance: Some(_)
                }
            ));
        }
        drop(second);
        drop(third);
        f.shutdown.cancel();
        f.server.await.unwrap().unwrap();
        f.manager.unload(&thread.id).await.unwrap();
        provider.finish().await;
    })
    .await
    .expect("shared control conformance completes");
}
