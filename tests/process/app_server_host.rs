//! Host callback uncertainty across actual App Server SIGKILL and native reopen.
#![cfg(unix)]
use super::app_server_readiness::websocket_startup;
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

async fn start(root: &Path) -> (Child, String, tokio::task::JoinHandle<()>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustx"))
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
        .kill_on_drop(true)
        .spawn()
        .unwrap();
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
            "agent":{"model":{"model":"emulator/chat-model"},"tools":{"builtin":[]}},
            "providers":{"emulator":{"base_url":provider.openai_base_url(),"api_key":"$HOST_TEST_KEY"}},
            "models":{"emulator/chat-model":{"provider":"emulator","id":"chat-model","protocol":"openai_chat_completions","context_window":128_000,"max_output_tokens":1024,"capabilities":{"input_modalities":["text"],"output_modalities":["text"]},"compat":{"chat_reasoning_replay":"omit"}}}
        })).unwrap()).unwrap();
        let (mut process,endpoint,diagnostics)=start(root.path()).await;
        let mut client=Peer::connect(&endpoint).await;
        let definition=HostToolDefinition {namespace:Some("business".into()),name:"lookup".into(),description:"Read application record".into(),input_schema:serde_json::json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false})};
        let MethodResult::SessionTransition {thread,session,..}=client.rpc(Method::SessionCreate {settings:SessionPersistentState {cwd:workspace,model:None},submission:None,dynamic_tools:vec![definition.clone()]}).await else {panic!("Thread")};
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
