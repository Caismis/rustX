//! Broadcast Host Tools over real `WebSockets`, catalog, adapter, HTTP and parser.
use crate::{common::provider_emulator::ProviderEmulator, launch_fixture::LaunchFixture};
use base64::Engine as _;
use rustx::app_server::host_upload::{HostUploadRefusal, MAX_CHUNK_BYTES, canonical_digest};
use rustx::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
use rustx::tools::types::ManagedOutputContinuation;
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
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

#[path = "../support/app_server_peer.rs"]
mod peer;
use peer::Peer;

fn definition(namespace: &str) -> HostToolDefinition {
    HostToolDefinition {
        namespace: Some(namespace.into()),
        name: "lookup".into(),
        description: "Look up one application record".into(),
        permissions: None,
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
                max_resident_runtimes: None,
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

fn candidate(request: &ServerRequest) -> Value {
    let ServerRequestMethod::ToolCall(call) = &request.call else {
        panic!("Host callback")
    };
    let mut candidate = serde_json::to_value(call.upload_scope.as_ref().unwrap()).unwrap();
    candidate["requestId"] = json!(request.id);
    candidate["threadId"] = json!(call.thread_id);
    candidate["turnId"] = json!(call.turn_id);
    candidate["callId"] = json!(call.call_id);
    candidate["uploadId"] = json!(uuid::Uuid::new_v4().to_string());
    candidate
}

async fn upload_request(peer: &mut Peer, method: &str, params: Value) -> Response {
    peer.raw_request(json!({
        "jsonrpc": "2.0",
        "method": format!("item/tool/result/{method}"),
        "params": params,
    }))
    .await
}

fn assert_accepted(response: Response, expected_duplicate: bool) {
    let Response::Success(success) = response else {
        panic!("expected upload acknowledgement, got {response:?}")
    };
    assert!(
        matches!(
            success.result,
            MethodResult::HostUploadAccepted { duplicate } if duplicate == expected_duplicate
        ),
        "{success:?}"
    );
}

fn assert_refused(response: Response, expected: HostUploadRefusal) {
    let Response::Failure(failure) = response else {
        panic!("expected {expected:?}, got {response:?}")
    };
    assert!(
        matches!(
            failure.error.data,
            Some(ErrorData::HostUploadRejected { reason }) if reason == expected
        ),
        "{failure:?}"
    );
}

async fn stream_large_result(
    first: &mut Peer,
    second: &mut Peer,
    call: &ServerRequest,
    copy: &ServerRequest,
) {
    let a = candidate(call);
    let b = candidate(copy);
    let body = format!(
        "start-of-host-output{}host-middle{}🙂end-of-host-output",
        "q".repeat(600_000),
        "z".repeat(600_000)
    );
    let items = json!([{"typeTag": 1, "length": body.len().to_string()}]);
    let begin =
        |candidate: &Value| json!({"candidate": candidate, "success": true, "items": items});
    assert_accepted(upload_request(first, "begin", begin(&a)).await, false);
    assert_refused(
        upload_request(second, "begin", begin(&a)).await,
        HostUploadRefusal::Foreign,
    );
    assert_accepted(upload_request(second, "begin", begin(&b)).await, false);
    assert_accepted(upload_request(second, "begin", begin(&b)).await, true);
    for (index, chunk) in body.as_bytes().chunks(MAX_CHUNK_BYTES).enumerate() {
        let params = json!({
            "candidate": b,
            "itemIndex": 0,
            "sequence": (index + 1).to_string(),
            "offset": (index * MAX_CHUNK_BYTES).to_string(),
            "dataBase64": base64::engine::general_purpose::STANDARD.encode(chunk),
            "sha256": format!("{:x}", Sha256::digest(chunk)),
        });
        assert_accepted(upload_request(second, "chunk", params.clone()).await, false);
        if index == 0 {
            assert_accepted(upload_request(second, "chunk", params).await, true);
        }
    }
    let digest = format!(
        "{:x}",
        sha2::digest::Output::<Sha256>::from(canonical_digest(&[(1, body.as_bytes())]))
    );
    let end = json!({
        "candidate": b, "success": true, "items": items,
        "total": body.len().to_string(), "digest": digest,
    });
    let mut incomplete_end = end.clone();
    incomplete_end["candidate"] = a.clone();
    assert_refused(
        upload_request(first, "end", incomplete_end).await,
        HostUploadRefusal::Incomplete,
    );
    let mut invalid_end = end.clone();
    invalid_end["digest"] = json!("0".repeat(64));
    assert_refused(
        upload_request(second, "end", invalid_end).await,
        HostUploadRefusal::DigestMismatch,
    );
    assert_accepted(upload_request(second, "end", end.clone()).await, false);
    assert_refused(
        upload_request(second, "end", end).await,
        HostUploadRefusal::Late,
    );
    assert_refused(
        upload_request(first, "abort", json!({"candidate": a})).await,
        HostUploadRefusal::Late,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn three_clients_broadcast_first_response_error_and_invalid_payload_commit_once() {
    tokio::time::timeout(std::time::Duration::from_secs(180),async {
        for winner in ["success","error","invalid","stream"] {
            let Some(provider)=ProviderEmulator::start("app_server_host_broadcast").await else {return;};
            let ServerFixture { root,workspace,sessions,manager,host,endpoint,shutdown,server }=ServerFixture::start(&provider).await;
            let mut first=Peer::connect(&endpoint).await; let mut second=Peer::connect(&endpoint).await; let mut generic=Peer::connect(&endpoint).await;
            let definitions=vec![definition("business"),definition("inventory")];
            let MethodResult::SessionTransition {thread,session,..}=first.rpc(Method::SessionCreate {dynamic_tools:definitions.clone(),submission:None,settings:SessionPersistentState {cwd:workspace.clone(),model:None}}).await else {panic!("Thread start")};
            sessions.rename_session(&session.id,"Host conformance").await.unwrap();
            assert!(manager.execution_endpoint(&session.id).is_err());
            assert_eq!(thread.dynamic_tools,definitions);
            let a=first.follow(thread.id.clone()).await; let b=second.follow(thread.id.clone()).await; let c=generic.follow(thread.id.clone()).await;
            assert_eq!(a.runtime_incarnation,b.runtime_incarnation); assert_eq!(b.runtime_incarnation,c.runtime_incarnation);
            let input_id=uuid::Uuid::new_v4().to_string();
            let MethodResult::TurnInputAccepted {turn_id,disposition:TurnInputDisposition::Started,..}=first.rpc(Method::TurnStart {input_id:input_id.clone(),target:a.clone(),content:vec![UserInputBlock::Text(TextBlock {text:"shared host request".into()})]}).await else {panic!("native Attempt receipt")};
            provider.await_gate("host-call").await; provider.release_gate("host-call").await;
            let ServerMessage::Request(call)=first.callback().await else {panic!("Host call")};
            let ServerMessage::Request(copy)=second.callback().await else {panic!("broadcast")};
            let ServerMessage::Request(unhandled)=generic.callback().await else {panic!("generic subscriber")};
            let mut common = Vec::new();
            let mut scopes = std::collections::BTreeSet::new();
            for mut received in [call.clone(), copy.clone(), unhandled.clone()] {
                let ServerRequestMethod::ToolCall(tool) = &mut received.call else {panic!("tool callback")};
                assert!(scopes.insert(tool.upload_scope.take().unwrap().connection_incarnation));
                common.push(received);
            }
            assert_eq!(common[0], common[1]); assert_eq!(common[1], common[2]);
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
            if winner == "stream" {
                stream_large_result(&mut first, &mut second, &call, &copy).await;
            } else {
                first.reply(selected).await;
            }
            let ServerMessage::Resolved {params,..}=second.callback().await else {panic!("resolution")}; assert_eq!(params.request_id,call.id);
            second.reply(serde_json::json!({"jsonrpc":"2.0","id":call.id,"result":{"success":true,"contentItems":[{"type":"inputText","text":"late-result"}]}})).await;
            first.settled(&turn_id).await;
            assert!(matches!(generic.callback().await,ServerMessage::Resolved {..}));
            assert!(matches!(first.rpc(Method::InboundSubmissionLookup {target:a.clone(),input_id}).await,MethodResult::InboundSubmission {acceptance:Some(_)}));
            let access=sessions.acquire_session(&session.id,Some(&thread.node_id)).await.unwrap();
            let store=SqliteConversationStore::open_existing(thread.id.clone(),&access.database_path).unwrap();
            let canonical=store.load_canonical().unwrap(); let results:Vec<_>=canonical.iter().filter_map(|message|if let MessageBlock::Tool(tool)=message {Some(&tool.result)}else{None}).collect();
            assert_eq!(results.len(),1,"one canonical ToolResult");
            assert_eq!(matches!(results[0].status,ToolExecutionStatus::Success),matches!(winner, "success" | "stream"));
            if matches!(winner, "error" | "invalid") {assert!(matches!(results[0].status,ToolExecutionStatus::OutcomeUnknown {..}));}
            assert!(manager.execution_endpoint(&session.id).is_err());
            let requests = provider.requests().await;
            assert_eq!(requests.len(),2,"one model continuation");
            if winner == "stream" {
                let result = results[0];
                assert!(matches!(result.managed_output.as_deref(), Some(ManagedOutputContinuation::Unavailable { .. })));
                assert!(result.truncation.as_ref().unwrap().original_bytes.unwrap() > 1_048_576);
                let tool_messages = requests[1]["body"]["messages"].as_array().unwrap()
                    .iter().filter(|message| message["role"] == "tool").collect::<Vec<_>>();
                assert_eq!(tool_messages.len(), 1);
                assert_eq!(tool_messages[0]["tool_call_id"], "host-call-1");
                let continuation = serde_json::to_string(&tool_messages[0]["content"]).unwrap();
                assert!(continuation.contains("start-of-host-output"));
                assert!(continuation.contains("end-of-host-output"));
                assert!(!continuation.contains("host-middle"));
                assert!(continuation.len() <= FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 4096);
            }
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
