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
#[path = "app_server_host/save_backpressure.rs"]
mod save_backpressure;
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

struct SaveStaging<'a> {
    scratch: &'a std::path::Path,
    receipts: Option<&'a mut save_backpressure::SaveReceipts>,
    refuse_save: bool,
    wait_for_winner: bool,
    backpressure: Option<save_backpressure::SaveBackpressure>,
}

async fn stream_large_result(
    first: &mut Peer,
    second: &mut Peer,
    call: &ServerRequest,
    copy: &ServerRequest,
    body: &str,
    SaveStaging {
        scratch,
        mut receipts,
        refuse_save,
        wait_for_winner,
        mut backpressure,
    }: SaveStaging<'_>,
) {
    let a = candidate(call);
    let b = candidate(copy);
    let parts = ["start-of-host-output", "", body, ""];
    let items = json!(parts.map(|part| json!({"typeTag": 1, "length": part.len().to_string()})));
    let total = parts.iter().map(|part| part.len()).sum::<usize>();
    let begin =
        |candidate: &Value| json!({"candidate": candidate, "success": true, "items": items});
    assert_accepted(upload_request(first, "begin", begin(&a)).await, false);
    assert_refused(
        upload_request(second, "begin", begin(&a)).await,
        HostUploadRefusal::Foreign,
    );
    assert_accepted(upload_request(second, "begin", begin(&b)).await, false);
    assert_accepted(upload_request(second, "begin", begin(&b)).await, true);
    let mut loser_bytes = None;
    let chunk_params =
        |candidate: &Value, item_index, sequence: u64, offset: usize, chunk: &[u8]| {
            json!({
                "candidate": candidate,
                "itemIndex": item_index,
                "sequence": sequence.to_string(),
                "offset": offset.to_string(),
                "dataBase64": base64::engine::general_purpose::STANDARD.encode(chunk),
                "sha256": format!("{:x}", Sha256::digest(chunk)),
            })
        };
    if let Some(observer) = receipts.as_mut()
        && !refuse_save
    {
        let first_item = parts[0].as_bytes();
        let prefix = &body.as_bytes()[..MAX_CHUNK_BYTES];
        for params in [
            chunk_params(&a, 0, 1, 0, first_item),
            chunk_params(&a, 2, 2, 0, prefix),
        ] {
            assert_accepted(upload_request(first, "chunk", params).await, false);
        }
        let staged = first_item.len() + 2 + prefix.len();
        observer.wait_for_files(scratch, &[staged as u64]).await;
        loser_bytes = Some(staged as u64);
    }
    let mut sequence = 1;
    let mut prior = 0;
    let mut storage_conflict = false;
    for (item_index, part) in parts.iter().enumerate() {
        for (index, chunk) in part.as_bytes().chunks(MAX_CHUNK_BYTES).enumerate() {
            let offset = index * MAX_CHUNK_BYTES;
            let params = chunk_params(&b, item_index, sequence, offset, chunk);
            assert_accepted(upload_request(second, "chunk", params.clone()).await, false);
            if sequence == 1 {
                assert_accepted(upload_request(second, "chunk", params).await, true);
            }
            if sequence == 2
                && let Some(blocked) = backpressure.as_mut()
            {
                blocked.wait_for_begin().await;
                let [file] = saved_files(scratch).try_into().unwrap();
                assert_eq!(file.metadata().unwrap().len(), 0);
            }
            if sequence == 6
                && let Some(blocked) = backpressure.take()
            {
                // The first over-preview fragment and the next three fill
                // the writer's four slots. The fifth leaves storage
                // unavailable while the Host receive loop still progresses.
                blocked.check_health_and_release().await;
                assert_eq!(saved_files(scratch), [] as [std::path::PathBuf; 0]);
            }
            let staged = prior + item_index + offset + chunk.len();
            if let Some(observer) = receipts.as_mut()
                && staged > FOREGROUND_TOOL_RESULT_PREVIEW_BYTES
            {
                let staged = staged as u64;
                if let Some(loser) = loser_bytes {
                    observer.wait_for_files(scratch, &[loser, staged]).await;
                } else if refuse_save && !storage_conflict {
                    observer.wait_for_files(scratch, &[staged]).await;
                    // Advance the actual private file outside its writer so the
                    // Client's next contiguous append detects a storage error.
                    let [file] = saved_files(scratch).try_into().unwrap();
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(file)
                        .unwrap()
                        .set_len(staged + 1)
                        .unwrap();
                    storage_conflict = true;
                }
            }
            sequence += 1;
        }
        prior += part.len();
    }
    let digest = format!(
        "{:x}",
        sha2::digest::Output::<Sha256>::from(canonical_digest(
            &parts.map(|part| (1, part.as_bytes()))
        ))
    );
    let end = json!({
        "candidate": b, "success": true, "items": items,
        "total": total.to_string(), "digest": digest,
    });
    let mut incomplete_end = end.clone();
    assert!(
        backpressure.is_none(),
        "the stalled save must reach its bound"
    );
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
    if let Some(observer) = receipts.as_mut() {
        if refuse_save {
            assert!(storage_conflict);
            observer.wait_for_files(scratch, &[]).await;
        } else if wait_for_winner {
            observer
                .wait_for_files(scratch, &[(total + parts.len() - 1) as u64])
                .await;
        }
    }
}

fn saved_directories(scratch: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(scratch)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("rustx-save-")
                .then(|| entry.path())
        })
        .collect()
}

fn saved_files(scratch: &std::path::Path) -> Vec<std::path::PathBuf> {
    saved_directories(scratch)
        .into_iter()
        .filter_map(|directory| std::fs::read_dir(directory).ok())
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect()
}

async fn verify_native_continuation(
    manager: &SessionRuntimeManager,
    session: &rustx::runtime::identity::SessionId,
    owner: &rustx::runtime::identity::ConversationId,
    workspace: &std::path::Path,
    artifacts: &std::path::Path,
    locator: &std::path::Path,
) {
    use crate::common::NoopProgress;
    use rustx::{
        runtime::{
            cancellation::{CancellationSignal, ExecutionCancellation},
            identity::{ConversationId, ToolCallId, ToolId},
            types::CancellationReason,
        },
        tools::{
            NativeToolPolicies,
            executor::{ToolExecutionContext, ToolRegistry},
            native::{NativeToolResources, register_native_tools},
            runtime::{ConversationRuntimeConfig, ConversationToolRuntime},
            types::{ToolInvocation, ToolInvocationId, ToolInvocationMode},
        },
    };

    let mut config = ConversationRuntimeConfig::new(workspace, artifacts);
    config.execution_scope = Some(manager.execution_scope(session));
    let runtime = ConversationToolRuntime::from_config(owner.clone(), config).unwrap();
    let mut registry = ToolRegistry::new();
    register_native_tools(
        &mut registry,
        NativeToolResources {
            background: runtime.background().clone(),
            subagents: None,
            subagent_catalog: rustx::runtime::subagent::AgentCatalog::empty(),
        },
        NativeToolPolicies::default(),
    )
    .unwrap();

    let context = |conversation| {
        ToolExecutionContext::new(
            conversation,
            None,
            ExecutionCancellation::detached(
                CancellationSignal::new(),
                CancellationReason::UserRequested,
            ),
            runtime.workspace(),
            &NoopProgress,
            runtime.artifacts(),
            runtime.tool_output(),
            runtime.environment(),
        )
    };
    let invoke = |name, arguments| ToolInvocation {
        id: ToolInvocationId::Agent {
            call_id: ToolCallId::new(format!("readback-{name}")),
        },
        tool_id: ToolId::new(format!("tool-{name}")),
        tool_name: String::from(name),
        mode: ToolInvocationMode::Foreground,
        arguments,
    };
    let read = invoke("read", json!({"path": locator, "limit": 2}));
    let reader = registry.executor(&read.tool_id);
    let read_result = reader.start(read.clone(), context(owner)).completion.await;
    assert_eq!(read_result.status, ToolExecutionStatus::Success);
    assert!(
        read_result
            .model_facing_projection()
            .as_text()
            .contains("start-of-host-output\n")
    );

    let grep = invoke(
        "grep",
        json!({"path": locator, "pattern": "host-middle", "literal": true}),
    );
    let grep_result = registry
        .executor(&grep.tool_id)
        .start(grep, context(owner))
        .completion
        .await;
    assert_eq!(grep_result.status, ToolExecutionStatus::Success);
    assert!(
        grep_result
            .model_facing_projection()
            .as_text()
            .contains("truncated to 500 chars")
    );

    let foreign = ConversationId::new("conv_161360a8-58d1-7c90-8acb-24a22e5d6832");
    let denied = reader.start(read, context(&foreign)).completion.await;
    assert!(matches!(denied.status, ToolExecutionStatus::Failed { .. }));
    assert!(
        !denied
            .model_facing_projection()
            .as_text()
            .contains("start-of-host-output")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn three_clients_broadcast_first_response_error_and_invalid_payload_commit_once() {
    tokio::time::timeout(std::time::Duration::from_secs(180),async {
        for winner in ["success","error","invalid","stream","stream_saved","stream_save_refused","stream_save_saturated","stream_save_receipt_lost","stream_disk_full","stream_save_cancelled",
            "ordinary_unbound","ordinary_saved","ordinary_disk_full",
            "small_stream_unbound","small_stream_saved","small_stream_disk_full"] {
            if winner.ends_with("_disk_full") && !cfg!(target_os = "linux") { continue; }
            let saved_cancelled = winner == "stream_save_cancelled";
            let ordinary = winner.starts_with("ordinary_");
            let streamed = winner.starts_with("stream") || winner.starts_with("small_stream");
            let saved = winner.ends_with("_saved");
            let body = if ordinary || winner.starts_with("small_stream") { 50 * 1024 } else { 600_000 };
            let body = format!("{}host-middle{}🙂end-of-host-output", "q".repeat(body), "z".repeat(body));
            let scenario = if saved_cancelled { "app_server_host_save_cancel" } else { "app_server_host_broadcast" };
            let Some(provider)=ProviderEmulator::start(scenario).await else {return;};
            let ServerFixture { root,workspace,sessions,manager,host,endpoint,shutdown,server }=ServerFixture::start(&provider).await;
            let mut first=Peer::connect(&endpoint).await; let mut second=Peer::connect(&endpoint).await; let mut generic=Peer::connect(&endpoint).await;
            let definitions=vec![definition("business"),definition("inventory")];
            let MethodResult::SessionTransition {thread,session,..}=first.rpc(Method::SessionCreate {input_references: None,dynamic_tools:definitions.clone(),submission:None,settings:SessionPersistentState {session_config: None,session_resources: None,cwd:workspace.clone(),model:None}}).await else {panic!("Thread start")};
            sessions.rename_session(&session.id,"Host conformance").await.unwrap();
            assert!(manager.execution_endpoint(&session.id).is_err());
            let scratch = root.path().join("client-scratch");
            std::fs::create_dir(&scratch).unwrap();
            let mut backpressure = None;
            let mut missing_receipt = None;
            let mut disk_failure = None;
            let mut cancel_at_seal = None;
            let mut save_receipts = None;
            let mut client = if winner == "stream_save_saturated" {
                let (child, blocked) = save_backpressure::spawn(&manager, &session.id, &scratch).await;
                backpressure = Some(blocked);
                Some(child)
            } else if winner == "stream_save_receipt_lost" {
                let (child, receipt, receipts) = save_backpressure::lose_publication_receipt(&manager, &session.id, &scratch).await;
                missing_receipt = Some(receipt);
                save_receipts = Some(receipts);
                Some(child)
            } else if winner.ends_with("_disk_full") {
                let (child, receipt) = save_backpressure::spawn_disk_limited(&manager, &session.id, &scratch).await;
                disk_failure = Some(receipt);
                Some(child)
            } else if winner == "stream_save_cancelled" {
                let (child, seal, receipts) = save_backpressure::hold_seal_receipt(&manager, &session.id, &scratch).await;
                cancel_at_seal = Some(seal);
                save_receipts = Some(receipts);
                Some(child)
            } else if saved || winner == "stream_save_refused" {
                let (child, receipts) = save_backpressure::observe_save_receipts(&manager, &session.id, &scratch).await;
                save_receipts = Some(receipts);
                Some(child)
            } else { None };
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
                _ if ordinary => {
                    let items = ["start-of-host-output", "", body.as_str(), ""]
                        .map(|text| json!({"type":"inputText","text":text}));
                    json!({"jsonrpc":"2.0","id":call.id,"result":{"success":true,"contentItems":items}})
                }

                _=>serde_json::json!({"jsonrpc":"2.0","id":call.id,"result":"invalid domain payload"}),
            };
            if streamed {
                stream_large_result(&mut first, &mut second, &call, &copy, &body, SaveStaging {
                    scratch: &scratch, receipts: save_receipts.as_mut(),
                    refuse_save: winner == "stream_save_refused", wait_for_winner: saved,
                    backpressure,
                }).await;
            } else {
                first.reply(selected).await;
            }
            let ServerMessage::Resolved {params,..}=second.callback().await else {panic!("resolution")}; assert_eq!(params.request_id,call.id);
            second.reply(serde_json::json!({"jsonrpc":"2.0","id":call.id,"result":{"success":true,"contentItems":[{"type":"inputText","text":"late-result"}]}})).await;
            if let Some(mut seal) = cancel_at_seal {
                seal.wait_for_seal().await;
                let MethodResult::CancellationAccepted { attempt_id } = first.rpc(Method::TurnCancel {
                    target: a.clone(), turn_id: turn_id.clone(),
                }).await else { panic!("cancel the originating Attempt"); };
                assert_eq!(attempt_id, turn_id);
                seal.release_and_confirm_removal().await;
            }
            first.settled(&turn_id).await;
            if let Some(receipt) = disk_failure {
                receipt.await.expect("real failed write and confirmed candidate removal");
            }
            if let Some(receipt) = missing_receipt.take() {
                let (key, locator) = receipt.reattach_after_canonical_commit().await;
                let binding = manager.execution_endpoint(&session.id).unwrap();
                let admission = binding.admission().unwrap();
                assert_eq!(key.owner.instance, admission.instance);
                assert_eq!(key.owner.generation, admission.generation);
                assert!(String::from(locator).starts_with("save:"));
            }
            assert!(matches!(generic.callback().await,ServerMessage::Resolved {..}));
            assert!(matches!(first.rpc(Method::InboundSubmissionLookup {target:a.clone(),input_id}).await,MethodResult::InboundSubmission {acceptance:Some(_)}));
            let access=sessions.acquire_session(&session.id,Some(&thread.node_id)).await.unwrap();
            let store=SqliteConversationStore::open_existing(thread.id.clone(),&access.database_path).unwrap();
            let canonical=store.load_canonical().unwrap(); let results:Vec<_>=canonical.iter().filter_map(|message|if let MessageBlock::Tool(tool)=message {Some(&tool.result)}else{None}).collect();
            assert_eq!(results.len(),1,"one canonical ToolResult");
            assert_eq!(matches!(results[0].status,ToolExecutionStatus::Success),winner == "success" || streamed || ordinary);
            if matches!(winner, "error" | "invalid") {assert!(matches!(results[0].status,ToolExecutionStatus::OutcomeUnknown {..}));}
            assert_eq!(manager.execution_endpoint(&session.id).is_ok(), client.is_some());
            let requests = provider.requests().await;
            assert_eq!(requests.len(), if saved_cancelled { 1 } else { 2 }, "no business replay or post-cancel inference");
            if streamed || ordinary {
                let result = results[0];
                let raw_bytes = ("start-of-host-output".len() + body.len()) as u64;
                assert_eq!(result.truncation.as_ref().unwrap().original_bytes, Some(raw_bytes));
                if saved {
                    let Some(ManagedOutputContinuation::Complete { locator }) = result.managed_output.as_deref() else {
                        panic!("confirmed Client publication: {result:?}");
                    };
                    assert!(locator.starts_with("/.rustx-output/import"));
                    assert!(!locator.exists(), "Server-side path does not resolve to a file");
                    let directories = saved_directories(&scratch);
                    assert_eq!(directories.len(), 1);
                    let physical = std::fs::read_dir(&directories[0]).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
                    assert_eq!(physical.len(), 1, "only the winner's complete output is retained");
                    let text = std::fs::read_to_string(physical[0].path()).unwrap();
                    assert_eq!(text, format!("start-of-host-output\n\n{body}\n"));
                    verify_native_continuation(&manager, &session.id, &thread.id, &workspace, &root.path().join("readback"), locator).await;
                } else {
                    assert!(matches!(result.managed_output.as_deref(), Some(ManagedOutputContinuation::Unavailable { .. })));
                    if winner == "stream_save_receipt_lost" {
                        let [file] = saved_files(&scratch).try_into().unwrap();
                        assert_eq!(file.metadata().unwrap().len(), 1_200_056);
                    } else {
                        assert_eq!(saved_files(&scratch), [] as [std::path::PathBuf; 0]);
                    }
                }
                if !saved_cancelled {
                let tool_messages = requests[1]["body"]["messages"].as_array().unwrap()
                    .iter().filter(|message| message["role"] == "tool").collect::<Vec<_>>();
                assert_eq!(tool_messages.len(), 1);
                assert_eq!(tool_messages[0]["tool_call_id"], "host-call-1");
                let continuation = tool_messages[0]["content"].as_str().unwrap();
                assert!(continuation.contains("start-of-host-output\n\nqqq"));
                assert!(continuation.contains("end-of-host-output"));
                assert!(!continuation.contains("host-middle"));
                assert!(continuation.len() <= FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 4096);
                }
            }
            drop(store); drop(access); drop(first); drop(second); drop(generic); shutdown.cancel();server.await.unwrap().unwrap();
            manager.unload(&thread.id).await.unwrap();
            if let Some(child) = client.as_mut() {
                assert_eq!(host.drain().await, Vec::<String>::new());
                assert!(child.wait().await.unwrap().success());
                assert_eq!(saved_directories(&scratch), Vec::<std::path::PathBuf>::new());
            }
            drop(host); drop(manager); provider.finish().await;
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
                input_references: None,
                dynamic_tools: Vec::new(),
                submission: None,
                settings: SessionPersistentState {
                    session_config: None,
                    session_resources: None,
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
