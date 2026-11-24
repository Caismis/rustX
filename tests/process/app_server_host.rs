//! Host callback uncertainty across actual App Server SIGKILL and native reopen.
#![cfg(unix)]
use super::{
    app_server::{
        assert_standalone_product_files, bounded, isolated_server_bin, server_program, terminate,
    },
    app_server_readiness::websocket_startup,
};
use crate::common::provider_emulator::ProviderEmulator;
use rustx::{
    app_server::protocol::*,
    durable::{CanonicalStore, SqliteConversationStore},
    local_runtime::{session::SessionPersistentState, session_controller::SessionController},
    message::{content::TextBlock, types::MessageBlock},
    tools::{ToolExecutionStatus, host::HostToolDefinition},
};
use std::{path::Path, process::Stdio};
use tokio::process::{Child, Command};
#[path = "../support/app_server_peer.rs"]
mod peer;
use peer::Peer;

#[cfg(target_os = "linux")]
mod generation;

fn server_command(root: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(root)
        .env("HOME", root.join("home"))
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .env("HOST_TEST_KEY", "fixture")
        .args(["app-server", "--config"])
        .arg(root.join("rustx.toml"))
        .arg("--runtime-root")
        .arg(root.join("runtime"))
        .args(["--listen", "ws://127.0.0.1:0", "--token-file"])
        .arg(root.join("token"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn start(root: &Path) -> (Child, String, tokio::task::JoinHandle<()>) {
    launch(server_command(root, server_program())).await
}

async fn launch(mut command: Command) -> (Child, String, tokio::task::JoinHandle<()>) {
    let mut child = command.spawn().unwrap();
    let (endpoint, mut stderr) = websocket_startup(&mut child).await;
    let diagnostics = tokio::spawn(async move {
        tokio::io::copy(&mut stderr, &mut tokio::io::sink())
            .await
            .unwrap();
    });
    (child, endpoint, diagnostics)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_effect_is_unknown_after_process_death_and_is_never_automatically_replayed() {
    tokio::time::timeout(std::time::Duration::from_secs(180),async {
        let Some(provider)=ProviderEmulator::start("app_server_host_crash").await else {return;};
        let root=tempfile::tempdir().unwrap();let workspace=root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();std::fs::create_dir(root.path().join("home")).unwrap();
        std::fs::write(root.path().join("token"),"trusted-host-test").unwrap();
        std::fs::write(root.path().join("rustx.toml"),toml::to_string_pretty(&serde_json::json!({
            // Host crash recovery is not a permission-mode test; Full Access
            // admits Host tools without Ask so the fixture reaches the
            // SIGKILL / OutcomeUnknown boundary.
            "approval_mode":"full_access",
            "agent":{"model":{"model":"emulator/chat-model"},"tools":{"builtin":[]}},
            "providers":{"emulator":{"protocol":"openai_chat_completions","base_url":provider.openai_base_url(),"api_key":"$HOST_TEST_KEY"}},
            "models":{"emulator/chat-model":{"provider":"emulator","id":"chat-model","context_window":128_000,"max_output_tokens":1024,"capabilities":{"input_modalities":["text"],"output_modalities":["text"]},"compat":{"chat_reasoning_replay":"omit"}}}
        })).unwrap()).unwrap();
        let (mut process,endpoint,diagnostics)=start(root.path()).await;
        let mut client=Peer::connect(&endpoint).await;
        let definition=HostToolDefinition {namespace:Some("business".into()),name:"lookup".into(),description:"Read application record".into(),permissions: None,
            input_schema:serde_json::json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false})};
        let MethodResult::SessionTransition {thread,session,..}=client.rpc(Method::SessionCreate {input_references: None,settings:SessionPersistentState {session_config: None,session_resources: None,cwd:workspace,model:None},submission:None,dynamic_tools:vec![definition.clone()]}).await else {panic!("Thread")};
        client.rpc(Method::SessionName {session_id:session.id.clone(),name:"Host crash recovery".into()}).await;
        let target=client.follow(thread.id.clone()).await;
        client.rpc(Method::TurnStart {target,input_id:"crash-input".into(),content:vec![UserInputBlock::Text(TextBlock {text:"crash-host".into()})]}).await;
        let ServerMessage::Request(request)=client.callback().await else {panic!("Host callback")};
        assert!(matches!(request.call,ServerRequestMethod::ToolCall(_)));
        // This is an actual application effect, followed by no response. The
        // native server cannot infer whether it happened from its own history.
        let effects=root.path().join("external-effects");
        let mut file=std::fs::File::create(&effects).unwrap();
        std::io::Write::write_all(&mut file,b"effect committed once\n").unwrap();file.sync_all().unwrap();drop(file);
        process.kill().await.unwrap();
        let status=process.wait().await.unwrap();
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status),Some(9));
        diagnostics.await.unwrap();drop(client);
        let (mut reopened,endpoint,diagnostics)=start(root.path()).await;
        let mut client=Peer::connect(&endpoint).await;
        let MethodResult::Thread {thread:stored}=client.rpc(Method::ThreadRead {thread_id:thread.id.clone()}).await else {panic!("persisted Thread")};
        assert_eq!(stored.dynamic_tools,vec![definition]);assert_eq!(stored.node_id,thread.node_id);
        let target=client.follow(thread.id.clone()).await;
        let MethodResult::Checkpoint {checkpoint}=client.rpc(Method::SessionCheckpoint {target,trace_records:Vec::new()}).await else {panic!("recovery checkpoint")};
        assert!(checkpoint.snapshot.attempt.as_ref().is_none_or(|attempt|matches!(attempt.phase,rustx::runtime_client::RuntimeClientAttemptPhase::Settled {..})));
        drop(client);
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(i32::try_from(reopened.id().unwrap()).unwrap()),nix::sys::signal::Signal::SIGTERM).unwrap();
        assert!(reopened.wait().await.unwrap().success());diagnostics.await.unwrap();
        let sessions=SessionController::open(&root.path().join("runtime")).unwrap();
        let access=sessions.acquire_session(&session.id,Some(&thread.node_id)).await.unwrap();
        let store=SqliteConversationStore::open_existing(thread.id.clone(),&access.database_path).unwrap();
        let canonical=store.load_canonical().unwrap();
        let results:Vec<_>=canonical.iter().filter_map(|message|if let MessageBlock::Tool(tool)=message {Some(&tool.result)}else {None}).collect();
        assert_eq!(results.len(),1);assert!(matches!(results[0].status,ToolExecutionStatus::OutcomeUnknown {..}));
        assert_eq!(std::fs::read_to_string(effects).unwrap(),"effect committed once\n");
        assert_eq!(provider.requests().await.len(),1,"recovery never resends model or Host invocation");
        drop(store);drop(access);drop(sessions);
provider.finish().await;
    }).await.expect("actual process recovery completes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn standalone_server_validates_large_host_stream_without_a_client() {
    bounded(async {
        use base64::Engine as _;
        use rustx::{
            app_server::{host_upload::{HostUploadRefusal, MAX_CHUNK_BYTES, canonical_digest}, transport::MAX_MESSAGE_BYTES},
            tools::{limits::MAX_MODEL_TOOL_RESULT_BYTES, types::ManagedOutputContinuation},
        };
        use serde_json::{Value, json};
        use sha2::{Digest as _, Sha256};

        async fn send(host: &mut peer::Peer, counts: &mut (usize, usize), method: &str, params: Value) -> Response {
            let frame = json!({"jsonrpc": "2.0", "method": format!("item/tool/result/{method}"), "params": params});
            let frame_bytes = host.next_request_bytes(frame.clone());
            assert!(frame_bytes < MAX_MESSAGE_BYTES);
            counts.0 += 1;
            counts.1 += frame_bytes;
            host.raw_request(frame).await
        }

        let Some(provider) = ProviderEmulator::start("app_server_host_broadcast").await else { return; };
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let scratch = root.path().join("scratch");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&scratch).unwrap();
        std::fs::create_dir(root.path().join("home")).unwrap();
        std::fs::write(root.path().join("token"), "trusted-host-test").unwrap();
        std::fs::write(root.path().join("rustx.toml"), toml::to_string_pretty(&json!({
            "approval_mode": "full_access",
            "agent":{"model":{"model":"emulator/chat-model"},"tools":{"builtin":[]}},
            "providers":{"emulator":{"protocol":"openai_chat_completions","base_url":provider.openai_base_url(),"api_key":"$HOST_TEST_KEY"}},
            "models":{"emulator/chat-model":{"provider":"emulator","id":"chat-model","context_window":128_000,"max_output_tokens":1024,"capabilities":{"input_modalities":["text"],"output_modalities":["text"]},"compat":{"chat_reasoning_replay":"omit"}}}
        })).unwrap()).unwrap();
        let (bin, server) = isolated_server_bin();
        let mut command = server_command(root.path(), server);
        command.env_clear().env("HOME", root.path().join("home")).env("PATH", bin.path())
            .env("TMPDIR", &scratch).env("HOST_TEST_KEY", "fixture");
        let (mut child, endpoint, diagnostics) = launch(command).await;
        let mut host = Peer::connect(&endpoint).await;
        let definitions = ["business", "inventory"].map(|namespace| HostToolDefinition {
            namespace: Some(namespace.into()), name: "lookup".into(), description: "Look up a record".into(),
            permissions: None, input_schema: json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false}),
        });
        let MethodResult::SessionTransition { session, thread, .. } = host.rpc(Method::SessionCreate {
            input_references: None,
            dynamic_tools: definitions.to_vec(), submission: None,
            settings: SessionPersistentState { session_config: None, session_resources: None, cwd: workspace.clone(), model: None },
        }).await else { panic!("Session creation") };
        host.rpc(Method::SessionName { session_id: session.id.clone(), name: "Installed Host stream".into() }).await;
        let target = host.follow(thread.id.clone()).await;
        let MethodResult::TurnInputAccepted { turn_id, disposition: TurnInputDisposition::Started, .. } = host.rpc(Method::TurnStart {
            target: target.clone(), input_id: uuid::Uuid::new_v4().to_string(),
            content: vec![UserInputBlock::Text(TextBlock { text: "shared host request".into() })],
        }).await else { panic!("turn admission") };
        tokio::select! {
            _ = provider.await_gate("host-call") => {}
            () = host.settled(&turn_id) => panic!("Attempt ended before the installed Host received a callback ({} provider requests)", provider.requests().await.len()),
        }
        provider.release_gate("host-call").await;
        let ServerMessage::Request(request) = host.callback().await else { panic!("Host callback") };
        let ServerRequestMethod::ToolCall(tool) = request.call else { panic!("Host tool call") };
        assert_eq!(tool.arguments, json!({"key":"invoice"}));
        let mut candidate = serde_json::to_value(tool.upload_scope.unwrap()).unwrap();
        candidate["requestId"] = json!(request.id);
        candidate["threadId"] = json!(thread.id);
        candidate["turnId"] = json!(turn_id);
        candidate["callId"] = json!(tool.call_id);
        candidate["uploadId"] = json!(uuid::Uuid::new_v4().to_string());

        #[cfg(target_os = "linux")]
        let measurement = std::env::var("RUSTX_HOST_MEASUREMENT_MIB").ok().map(|value| {
            let mebibytes = value.parse::<usize>().expect("measurement size in MiB");
            assert!((2..=16).contains(&mebibytes));
            let proc = std::path::PathBuf::from(format!("/proc/{}", child.id().unwrap()));
            let write_bytes = |proc: &Path| {
                std::fs::read_to_string(proc.join("io")).unwrap().lines()
                    .find_map(|line| line.strip_prefix("write_bytes: "))
                    .unwrap().parse::<u64>().unwrap()
            };
            (mebibytes, write_bytes(&proc), proc, write_bytes)
        });
        #[cfg(target_os = "linux")]
        let padding = measurement.as_ref().map_or(600_000, |(mib, ..)| (mib * 1_048_576 - 64) / 2);
        #[cfg(not(target_os = "linux"))]
        let padding = 600_000;
        let body = format!("installed-head:{}installed-middle{}:installed-tail", "q".repeat(padding), "z".repeat(padding));
        assert!(body.len() > MAX_MESSAGE_BYTES);
        let items = json!([{"typeTag": 1, "length": body.len().to_string()}]);
        let accepted = |response: Response| assert!(matches!(&response, Response::Success(reply)
            if matches!(reply.result, MethodResult::HostUploadAccepted { duplicate: false })), "{response:?}");
        let mut counts = (0, 0);
        #[cfg(target_os = "linux")]
        let upload_started = std::time::Instant::now();
        accepted(send(&mut host, &mut counts, "begin", json!({"candidate":candidate,"items":items,"success":true})).await);
        for (sequence, chunk) in body.as_bytes().chunks(MAX_CHUNK_BYTES).enumerate() {
            accepted(send(&mut host, &mut counts, "chunk", json!({
                "candidate":candidate,"itemIndex":0,"sequence":(sequence + 1).to_string(),
                "offset":(sequence * MAX_CHUNK_BYTES).to_string(),
                "dataBase64":base64::engine::general_purpose::STANDARD.encode(chunk),
                "sha256":format!("{:x}", Sha256::digest(chunk)),
            })).await);
        }
        let mut end = json!({"candidate":candidate,"items":items,"success":true,"total":body.len().to_string(),"digest":"0".repeat(64)});
        assert!(matches!(send(&mut host, &mut counts, "end", end.clone()).await, Response::Failure(error)
            if matches!(error.error.data, Some(ErrorData::HostUploadRejected { reason: HostUploadRefusal::DigestMismatch }))));
        let digest: sha2::digest::Output<Sha256> = canonical_digest(&[(1, body.as_bytes())]).into();
        end["digest"] = json!(format!("{digest:x}"));
        accepted(send(&mut host, &mut counts, "end", end).await);
        #[cfg(target_os = "linux")]
        let upload_ms = upload_started.elapsed().as_secs_f64() * 1000.0;
        host.settled(&turn_id).await;
        #[cfg(target_os = "linux")]
        if let Some((_, start_writes, proc, write_bytes)) = measurement {
            let status = std::fs::read_to_string(proc.join("status")).unwrap();
            let peak_kib = status.lines().find_map(|line| line.strip_prefix("VmHWM:"))
                .unwrap().split_ascii_whitespace().next().unwrap().parse::<u64>().unwrap();
            println!("HOST_MEASUREMENT {}", json!({
                "body_bytes": body.len(), "request_frames": counts.0, "host_to_server_json_bytes": counts.1,
                "server_peak_rss_kib": peak_kib, "server_write_bytes": write_bytes(&proc) - start_writes,
                "upload_ms": upload_ms, "including_settlement_ms": upload_started.elapsed().as_secs_f64() * 1000.0,
                "client_save_rpcs": 0,
            }));
        }

        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0, "no Server Spill before shutdown");
        let requests = provider.requests().await;
        assert_eq!(requests.len(), 2, "the model call and one bounded tool continuation");
        let tool_content: Vec<_> = requests[1]["body"]["messages"].as_array().unwrap().iter()
            .filter(|message| message["role"] == "tool").collect();
        assert_eq!(tool_content.len(), 1);
        let preview = tool_content[0]["content"].as_str().unwrap();
        assert!(preview.len() <= MAX_MODEL_TOOL_RESULT_BYTES);
        assert!(preview.contains("installed-head:") && preview.contains(":installed-tail"));
        assert!(!preview.contains("installed-middle"));
        host.rpc(Method::SessionDetach { target }).await;
        drop(host);
        terminate(&child);
        assert!(child.wait().await.unwrap().success());
        diagnostics.await.unwrap();
        let sessions = SessionController::open(&root.path().join("runtime")).unwrap();
        let access = sessions.acquire_session(&session.id, Some(&thread.node_id)).await.unwrap();
        let store = SqliteConversationStore::open_existing(thread.id, &access.database_path).unwrap();
        let canonical = store.load_canonical().unwrap();
        let results: Vec<_> = canonical.iter().filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(&tool.result),
            _ => None,
        }).collect();
        let [result] = results.as_slice() else { panic!("exactly one canonical tool result") };
        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(result.truncation.as_ref().unwrap().original_bytes, Some(body.len() as u64));
        assert!(matches!(result.managed_output.as_deref(), Some(ManagedOutputContinuation::Unavailable { .. })));
        assert_eq!(std::fs::read_dir(workspace).unwrap().count(), 0);
        assert_standalone_product_files(bin.path());
        provider.finish().await;
    }).await;
}
