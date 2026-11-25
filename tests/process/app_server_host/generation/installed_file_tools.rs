//! Budgeted native file calls through the installed Server and Client pair.

use super::*;
use std::sync::{Arc, Mutex};

const MARKER: &str = "EXEC-04-window-tail";
const BASH_MARKER: &str = "EXEC-04-bash-private-marker";
const SOURCE_BYTES: usize = 64 * 1024 * 1024;
const LINE_BYTES: usize = 128;

#[derive(Debug, Default, serde::Serialize)]
struct Traffic {
    exchanges: usize,
    to_client: usize,
    to_server: usize,
    largest_reply: usize,
}

impl Traffic {
    fn record(&mut self, request: usize, reply: usize) {
        self.exchanges += 1;
        self.to_client += request;
        self.to_server += reply;
        self.largest_reply = self.largest_reply.max(reply);
    }

    fn total_bytes(&self) -> usize {
        self.to_client + self.to_server
    }
}

#[derive(Debug, Default, serde::Serialize)]
struct Wire {
    pair: Traffic,
    tools: Traffic,
    read: usize,
    grep: usize,
    edit: usize,
    commit: usize,
    bash_read: usize,
    bash_grep: usize,
}

fn carries_tool_work(request: &ClientRequest) -> bool {
    match request {
        // Watching the Workspace can trigger ordinary source revalidation
        // after Edit publishes. Count lookups of either tool file, regardless
        // of how the tool chooses to obtain its content.
        ClientRequest::GetMetadata { relative_path }
        | ClientRequest::ReadRange { relative_path, .. }
        | ClientRequest::ResolveResourceEntry { relative_path } => {
            relative_path == Path::new("large.txt") || relative_path == Path::new("small.txt")
        }
        ClientRequest::Process(_)
        | ClientRequest::Save(_)
        | ClientRequest::ReadSavedOutput { .. }
        | ClientRequest::ReadSavedOutputRange { .. }
        | ClientRequest::SearchSavedOutput { .. }
        | ClientRequest::ReadBashOutput { .. }
        | ClientRequest::SearchBashOutput { .. }
        | ClientRequest::Search { .. }
        | ClientRequest::ReadLines { .. }
        | ClientRequest::ReadDocument { .. }
        | ClientRequest::PrepareEdit { .. }
        | ClientRequest::PrepareWrite { .. }
        | ClientRequest::BeginWrite { .. }
        | ClientRequest::WriteChunk { .. }
        | ClientRequest::CommitWrite { .. }
        | ClientRequest::AbortWrite { .. } => true,
        _ => false,
    }
}

async fn count_wire(mut client: UnixStream, mut server: UnixStream, wire: Arc<Mutex<Wire>>) {
    while let Ok((request, request_bytes)) = read_frame::<ClientRequest>(&mut server).await {
        write(&mut client, &request).await.unwrap();
        let (reply, reply_bytes) = read_frame::<ClientReply>(&mut client).await.unwrap();
        {
            let mut report = wire.lock().unwrap();
            report.pair.record(request_bytes, reply_bytes);
            if carries_tool_work(&request) {
                report.tools.record(request_bytes, reply_bytes);
            }
            report.read += usize::from(matches!(request, ClientRequest::ReadLines { .. }));
            report.grep += usize::from(matches!(
                request,
                ClientRequest::Search {
                    spec: rustx_execution_protocol::SearchSpec::Grep { .. },
                    ..
                }
            ));
            report.edit += usize::from(matches!(request, ClientRequest::PrepareEdit { .. }));
            report.commit += usize::from(matches!(request, ClientRequest::CommitWrite { .. }));
            report.bash_read +=
                usize::from(matches!(request, ClientRequest::ReadBashOutput { .. }));
            report.bash_grep +=
                usize::from(matches!(request, ClientRequest::SearchBashOutput { .. }));
        }
        if write(&mut server, &reply).await.is_err() {
            break;
        }
    }
}

fn seed(workspace: &Path) {
    use std::io::Write as _;
    let mut file = std::fs::File::create(workspace.join("large.txt")).unwrap();
    let mut line = [b'x'; LINE_BYTES];
    line[LINE_BYTES - 1] = b'\n';
    let block = line.repeat(512);
    for _ in 0..SOURCE_BYTES / block.len() {
        file.write_all(&block).unwrap();
    }
    writeln!(file, "{MARKER}").unwrap();
    std::fs::write(workspace.join("small.txt"), b"old line\n").unwrap();
}

fn model_calls<const N: usize>(calls: [(&str, Value); N]) -> crate::common::FixtureReply {
    let calls = calls
        .into_iter()
        .enumerate()
        .map(|(index, (name, arguments))| {
            let mut envelope = json!({"arguments": arguments});
            if name == "bash" {
                envelope["execution_mode"] = json!("foreground");
            }
            json!({"index": index, "id": format!("call_{name}"), "type": "function", "function":
            {"name": name, "arguments": envelope.to_string()}})
        })
        .collect::<Vec<_>>();
    let response = json!({
        "id": "chatcmpl-file-window", "object": "chat.completion.chunk", "created": 1, "model": "chat-model",
        "choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": calls}, "finish_reason": "tool_calls"}]
    });
    crate::common::FixtureReply::body(
        200,
        "OK",
        "text/event-stream",
        format!("data: {response}\n\ndata: [DONE]\n\n"),
    )
}

fn tool_content(request: &Value, name: &str) -> String {
    let id = format!("call_{name}");
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == id)
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn spill_calls(body: &str) -> crate::common::FixtureReply {
    let request: Value = serde_json::from_str(body).unwrap();
    let preview = tool_content(&request, "bash");
    assert!(!preview.contains(BASH_MARKER));
    let locator = preview
        .lines()
        .find_map(|line| line.strip_prefix("Complete output: "))
        .unwrap_or_else(|| panic!("Bash must advertise complete capture: {preview}"));
    model_calls([
        ("read", json!({"path": locator, "offset": 8193, "limit": 1})),
        (
            "grep",
            json!({"path": locator, "pattern": BASH_MARKER, "literal":true, "limit":1}),
        ),
    ])
}

fn peak_rss(process: &Path) -> u64 {
    std::fs::read_to_string(process.join("status"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .unwrap()
        .split_ascii_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_native_file_tools_keep_large_results_on_the_client() {
    installed_file_tools(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_bash_continuation_reads_only_the_selected_client_output() {
    installed_file_tools(true).await;
}

async fn installed_file_tools(spill: bool) {
    bounded(async {
        use super::super::super::app_server::process_cpu_and_write_counters as counters;
        use rustx::runtime::identity::AttemptId;
        use std::time::Instant;
        let fixture = crate::common::FixtureServer::start_with_body(move |attempt, _, body| match (spill, attempt) {
            (true, 0) => model_calls([("bash", json!({
                "command": format!("awk 'BEGIN {{ for (i=1;i<=16384;i++) {{ printf \"%0255d\\n\", i; if (i==8192) print \"{BASH_MARKER}\" }} }}'")
            }))]),
            (true, 1) => spill_calls(body),
            (false, 0) => model_calls([
                ("read", json!({"path":"large.txt","offset":SOURCE_BYTES / LINE_BYTES + 1,"limit":1})),
                ("grep", json!({"path":"large.txt","pattern":MARKER,"literal":true,"limit":1})),
                ("edit", json!({"path":"small.txt","edits":[{"oldText":"old line","newText":"new line"}]})),
            ]),
            _ => crate::common::sse_fixture("openai_chat", "plain_text.sse"),
        }).await;
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(root.path().join("home")).unwrap();
        std::fs::write(root.path().join("token"), "trusted-host-test").unwrap();
        std::fs::write(root.path().join("rustx.toml"), toml::to_string_pretty(&json!({
            "approval_mode": "full_access",
            "agent": {"model": {"model": "emulator/chat-model"}, "tools": {"builtin": ["read", "grep", "edit", "bash"]}},
            "providers": {"emulator": {"protocol": "openai_chat_completions", "base_url": fixture.url("/v1"), "api_key": "$HOST_TEST_KEY"}},
            "models": {"emulator/chat-model": {"provider": "emulator", "id": "chat-model", "context_window": 128_000, "max_output_tokens": 1024, "capabilities": {"input_modalities": ["text"], "output_modalities": ["text"]}, "compat": {"chat_reasoning_replay": "omit"}}},
            "app_server": {"environment_roots": [workspace]}
        })).unwrap()).unwrap();
        if !spill { seed(&workspace); }
        let initial = {
            let catalog = SessionController::open(&root.path().join("runtime")).unwrap();
            let session = catalog.create_session(SessionPersistentState::from_input(
                &rustx::local_runtime::SessionConfigInput::new(workspace.clone())
            )).await.unwrap().session;
            catalog.rename_session(&session.id, "Installed file measurement").await.unwrap();
            session
        };
        let (_bin, executable) = super::super::super::app_server::isolated_server_pair();
        let scratch = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let witness_path = scratch.path().join("witness.sock");
        let witness = UnixListener::bind(&witness_path).unwrap();
        stage_wrapper(&executable, &witness_path);

        let wire = Arc::new(Mutex::new(Wire::default()));
        let forwarded = Arc::clone(&wire);
        let (started, admitted) = oneshot::channel();
        let relay = tokio::spawn(async move {
            let (client, (data, server_data), (mut control, mut server_control)) =
                proxy_launch(&witness, scratch.path(), 1).await;
            started.send(client.pid).unwrap();
            let _ = tokio::join!(
                count_wire(data, server_data, forwarded),
                tokio::io::copy_bidirectional(&mut control, &mut server_control)
            );
        });
        let mut command = server_command(root.path(), &executable);
        command.args(["--local-execution-session", initial.id.as_str(), "--local-execution-workspace"])
            .arg(&workspace);
        let (server, address, diagnostics) = launch(command).await;
        let client = admitted.await.unwrap();
        let server_proc = PathBuf::from(format!("/proc/{}", server.id().unwrap()));
        let client_proc = PathBuf::from(format!("/proc/{client}"));
        let mut peer = Peer::connect(&address).await;
        let target = peer.follow(initial.active_conversation_id).await;
        // Attachment may capture configuration. Counters start at the turn.
        *wire.lock().unwrap() = Wire::default();
        let (server_before, server_writes) = counters(&server_proc);
        let (client_before, client_writes) = counters(&client_proc);
        let begin = Instant::now();
        let MethodResult::TurnInputAccepted { turn_id, .. } = peer.rpc(Method::TurnStart {
            input_id: uuid::Uuid::new_v4().to_string(),
            target,
            content: vec![UserInputBlock::Text(TextBlock { text: "Operate on the bounded file windows".into() })],
        }).await else {
            panic!("tool turn admitted");
        };
        let turn: AttemptId = turn_id;
        peer.settled(&turn).await;
        let elapsed = begin.elapsed();
        let (server_after, server_written) = counters(&server_proc);
        let (client_after, client_written) = counters(&client_proc);
        let requests = fixture.request_bodies();
        assert_eq!(requests.len(), if spill { 3 } else { 2 });
        let final_request = requests.last().unwrap();
        let request: Value = serde_json::from_str(final_request).unwrap();
        let marker = if spill { BASH_MARKER } else { MARKER };
        for name in ["read", "grep"] {
            let content = tool_content(&request, name);
            assert!(content.contains(marker), "{name} returned {content:?}");
        }
        {
            let wire = wire.lock().unwrap();
            if spill {
                assert_eq!((wire.bash_read, wire.bash_grep), (1, 1));
                assert_eq!((wire.read, wire.grep, wire.edit, wire.commit), (0, 0, 0, 0));
                assert!(wire.tools.total_bytes() <= 512 * 1024,
                    "output capture and the selected readback stay bounded: {wire:?}");
            } else {
                assert!(final_request.len() < 64 * 1024);
                assert_eq!(std::fs::read(workspace.join("small.txt")).unwrap(), b"new line\n");
                assert_eq!((wire.read, wire.grep, wire.edit, wire.commit), (1, 1, 1, 1));
                assert!(wire.tools.exchanges <= 16, "native file work is bounded: {wire:?}");
                assert!(wire.pair.largest_reply <= 8192 && wire.tools.total_bytes() <= 16 * 1024,
                    "the wire carries only selected results and commit metadata: {wire:?}");
            }
            if std::env::var_os("RUSTX_FILE_TOOL_MEASUREMENT").is_some() {
                println!("FILE_TOOL_MEASUREMENT {}", json!({
                    "spill": spill,
                    "settlement_ms": elapsed.as_secs_f64() * 1000.0,
                    "source_bytes": if spill { 4 * 1024 * 1024 + BASH_MARKER.len() + 1 } else { SOURCE_BYTES + MARKER.len() + 1 },
                    "server_cpu_ticks": server_after - server_before,
                    "client_cpu_ticks": client_after - client_before,
                    "server_write_bytes": server_written - server_writes,
                    "client_write_bytes": client_written - client_writes,
                    "server_peak_rss_kib": peak_rss(&server_proc),
                    "client_peak_rss_kib": peak_rss(&client_proc),
                    "wire": *wire
                }));
            }
        }
        drop(peer);
        shutdown(server, diagnostics).await;
        relay.await.unwrap();
    }).await;
}
