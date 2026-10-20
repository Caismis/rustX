//! Standalone ordinary optimized fixture host; no product source or hooks changed.
use std::{io::{self, Write}, path::PathBuf, sync::Arc, time::Instant};
use rustx::{
    app_server::{host::AppServerHost, transport::websocket},
    credentials::CredentialSnapshot,
    durable::{ConversationStore, SqliteConversationStore},
    local_runtime::{HostEnvironment, launch::LaunchRequest,
        composition::LocalRuntimeDependencies, session::SessionPersistentState,
        session_controller::SessionController,
        session_runtime_manager::{SessionRuntimeManager, RuntimeResidencyPolicy},
        app_server_policy::AppServerPolicy},
    message::types::MessageBlock,
    model::catalog::MapCredentialEnvironment,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;
mod request_oracle;
mod common;
include!("verify_requests.rs");
include!("canonical_history.rs");

fn process() -> Value {
    json!({"stat":std::fs::read_to_string("/proc/self/stat").ok(),
        "io":std::fs::read_to_string("/proc/self/io").ok(),
        "smaps_rollup":std::fs::read_to_string("/proc/self/smaps_rollup").ok()})
}
fn point(points: &mut Vec<Value>, name: &str, began: Instant) {
    points.push(json!({"phase":name,"elapsed_us":began.elapsed().as_micros(),"process":process()}));
}
fn emit(value: Value) { println!("{value}"); io::stdout().flush().unwrap(); }

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let args: Vec<_> = std::env::args().collect();
    let root=PathBuf::from(&args[1]); let case=&args[2];
    assert!(matches!(case.as_str(),"short-small"|"short-large"|"tool"));
    let began=Instant::now();let mut lifecycle=Vec::new();
    point(&mut lifecycle,"before_initialization",began);
    let workspace=root.join("workspace");std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("note.txt"),"deterministic-note-payload-6d41").unwrap();
    emit(json!({"kind":"provider_needed","workspace":workspace,"scenario":format!("runtime_benchmark_{case}")}));
    let mut line=String::new();io::stdin().read_line(&mut line).unwrap();
    let ready:Value=serde_json::from_str(&line).unwrap();
    assert_eq!(ready["host"],"127.0.0.1");
    assert_eq!(ready["scenario"],format!("runtime_benchmark_{case}"));
    point(&mut lifecycle,"after_provider_start",began);
    let settings=root.join("rustx.toml");
    let config=format!(r#"agent_id = "agent-issue47"
[agent.model]
model = "emulator/chat-model"
[agent.tools]
builtin = ["read","write","edit","glob","grep","bash"]
[agent.plugins.agent_status]
enabled = true
[agent.plugins.todo]
enabled = true
[context]
reserve_tokens = 1024
keep_recent_tokens = 8192
[providers.emulator]
base_url = "http://127.0.0.1:{}/v1"
api_key = "$RUSTX_CONFORMANCE_KEY"
[models."emulator/chat-model"]
provider = "emulator"
id = "chat-model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 1024
capabilities = {{ input_modalities = ["text"], output_modalities = ["text"], tool_calls = true, reasoning = true }}
compat = {{ chat_reasoning_replay = "omit" }}
"#,ready["port"].as_u64().unwrap());
    std::fs::write(&settings,config).unwrap();
    let home=root.join("home");std::fs::create_dir(&home).unwrap();
    let native=root.join("native");
    let env=HostEnvironment::from_paths(workspace.clone(),home).unwrap();
    let request=LaunchRequest{config:Some(settings),workspace:Some(workspace.clone()),runtime_root:Some(native.clone()),model:None};
    let(configuration,input)=request.session_input(&env).unwrap();
    let sessions=SessionController::open(&native).unwrap();
    let session=sessions.create_session(SessionPersistentState::from_input(&input)).await.unwrap().session;
    let dependencies=LocalRuntimeDependencies{
        status_clock:Arc::new(request_oracle::FixedStatusClock::default()),
        credentials:Some(Arc::new(MapCredentialEnvironment::new([("RUSTX_CONFORMANCE_KEY".into(),"conformance-secret".into())]))),
        ..LocalRuntimeDependencies::default()
    };
    let manager=SessionRuntimeManager::new(sessions.clone(),configuration,
        CredentialSnapshot::new([("RUSTX_CONFORMANCE_KEY".into(),"conformance-secret".into())]),dependencies,
        RuntimeResidencyPolicy{max_resident_runtimes:8,idle_grace_ms:300_000}).unwrap();
    let runtime=manager.load(&session.id,None).await.unwrap();drop(runtime);
    let access=sessions.acquire_session(&session.id,None).await.unwrap();
    let cid=access.node.conversation_id.clone();let database=access.database_path.clone();
    let store=Arc::new(SqliteConversationStore::open_existing(cid.clone(),&database).unwrap());
    let host=AppServerHost::new(manager.clone(),AppServerPolicy::default());
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint=format!("ws://{}",listener.local_addr().unwrap());
    let token="inspector-fixture-only-000000000000000000000000000000";
    let stop=CancellationToken::new();
    let serve=tokio::spawn(websocket::serve(listener,host.clone(),websocket::Credential::new(token.into()).unwrap(),stop.clone()));
    point(&mut lifecycle,"after_initialization",began);
    emit(json!({"kind":"ready","endpoint":endpoint,"token":token,"session_id":session.id,
        "conversation_id":cid,"workspace":workspace,"protocol_version":rustx::app_server::protocol::APP_SERVER_PROTOCOL_VERSION}));
    let mut commands=BufReader::new(tokio::io::stdin()).lines();
    let mut wire=Vec::new();let mut live=Vec::new();let mut canonical=Vec::new();
    while let Some(line)=commands.next_line().await.unwrap(){
        let command:Value=serde_json::from_str(&line).unwrap();
        match command["op"].as_str().unwrap(){
            "verify"=>{
                point(&mut lifecycle,"after_turns",began);
                wire=command["requests"].as_array().unwrap().clone();
                live=verify_requests(store.clone(),case,&workspace,&wire);
                canonical=canonical_history(store.as_ref());
                point(&mut lifecycle,"after_verification",began);
                emit(json!({"kind":"verified","requests":live.len()}));
            },
            "stop"=>break,
            other=>panic!("unknown controller command {other}"),
        }
    }
    assert!(!live.is_empty(),"controller must complete exact request verification");
    host.begin_drain();stop.cancel();serve.await.unwrap().unwrap();
    assert!(host.drain().await.is_empty());point(&mut lifecycle,"after_shutdown",began);
    drop(host);drop(manager);drop(sessions);drop(access);drop(store);
    point(&mut lifecycle,"after_driver_drop",began);
    drop(SqliteConversationStore::open(cid.clone(),&database).unwrap());
    point(&mut lifecycle,"after_final_writer_close",began);
    let reopened=Arc::new(SqliteConversationStore::open_existing(cid.clone(),&database).unwrap());
    let reopened_requests=verify_requests(reopened.clone(),case,&workspace,&wire);
    assert_eq!(live,reopened_requests);assert_eq!(canonical,canonical_history(reopened.as_ref()));
    drop(reopened);point(&mut lifecycle,"after_reopen_verification",began);
    std::fs::remove_dir_all(&native).unwrap();point(&mut lifecycle,"after_cleanup",began);
    emit(json!({"kind":"complete","oracle":"passed","case":case,"workload_version":3,
        "model_time":request_oracle::MODEL_TIME,"lifecycle":lifecycle,"wire_requests":wire,
        "live_requests":live,"reopened_requests":reopened_requests,"canonical":canonical}));
}
