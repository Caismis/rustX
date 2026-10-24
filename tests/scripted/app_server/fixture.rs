//! The two-Session native fixture behind every App Server contract: a gated
//! provider, two Workspaces, a real catalog and manager, and the bounded
//! helpers the suites share.
#![allow(clippy::too_many_lines)]
use super::*;
use std::fmt::Write as _;

/// Bounded fixture timings, emitted only when an existing operation is dropped.
pub(super) struct FixturePhase {
    label: &'static str,
    started: std::time::Instant,
    entered: std::time::Instant,
    stage: &'static str,
    finished: [(&'static str, u128); 32],
    count: usize,
    complete: bool,
}

impl FixturePhase {
    pub(super) fn new(label: &'static str) -> Self {
        let started = std::time::Instant::now();
        Self {
            label,
            started,
            entered: started,
            stage: "start",
            finished: [("", 0); 32],
            count: 0,
            complete: false,
        }
    }
    pub(super) fn enter(&mut self, stage: &'static str) {
        let now = std::time::Instant::now();
        if self.count < self.finished.len() {
            self.finished[self.count] = (self.stage, now.duration_since(self.entered).as_micros());
            self.count += 1;
        }
        self.stage = stage;
        self.entered = now;
    }
    pub(super) fn finish(&mut self) {
        self.complete = true;
    }
}

impl Drop for FixturePhase {
    fn drop(&mut self) {
        if !self.complete {
            eprintln!(
                "incomplete fixture phases: label={} stage={} elapsed_us={} stage_us={} finished_us={:?}",
                self.label,
                self.stage,
                self.started.elapsed().as_micros(),
                self.entered.elapsed().as_micros(),
                &self.finished[..self.count],
            );
        }
    }
}

pub(super) struct Fixture {
    _root: tempfile::TempDir,
    pub(super) archive_root: std::path::PathBuf,
    pub(super) manager: SessionRuntimeManager,
    pub(super) host: crate::app_server::host::AppServerHost,
    pub(super) sessions: Vec<SessionSnapshot>,
    pub(super) gates: [Arc<HeaderGate>; 2],
    pub(super) provider: FixtureServer,
    pub(super) workspaces: [std::path::PathBuf; 2],
}

impl Fixture {
    pub(super) fn install_scheduled_workflow_definition(&self, definition: &serde_json::Value) {
        self.install_scheduled_workflow("Native scheduled interaction");
        std::fs::write(
            self.workspaces[0].join(".agents/workflows/scheduled.yaml"),
            serde_json::to_vec_pretty(definition).unwrap(),
        )
        .unwrap();
    }
    pub(super) fn install_scheduled_workflow(&self, description: &str) {
        let directory = self.workspaces[0].join(".agents/workflows");
        std::fs::create_dir_all(&directory).unwrap();
        let definition = serde_json::json!({"description":description,"block":{"input":{"type":"object","properties":{},"additionalProperties":false},"output":{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false},"entry":"done","nodes":{"done":{"type":"return","output":{"type":"literal","value":{"answer":"native durable return"}}}},"edges":[]}});
        std::fs::write(
            directory.join("scheduled.yaml"),
            serde_json::to_vec_pretty(&definition).unwrap(),
        )
        .unwrap();
        let path = self.workspaces[0].join("rustx.toml");
        let mut config: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        config.as_table_mut().unwrap().insert(
            "agent".into(),
            toml::Value::try_from(serde_json::json!({"workflows":["scheduled"]})).unwrap(),
        );
        std::fs::write(path, toml::to_string(&config).unwrap()).unwrap();
    }

    pub(super) async fn new() -> Self {
        Self::with_tool(None).await
    }

    pub(super) async fn with_tool(tool: Option<&'static str>) -> Self {
        Self::with_session_count(tool, 2).await
    }
    pub(super) async fn with_session_count(tool: Option<&'static str>, count: usize) -> Self {
        Self::with_provider_gating(tool, count, false, None).await
    }

    pub(super) async fn with_summary_gate() -> Self {
        Self::with_provider_gating(None, 1, true, None).await
    }

    pub(super) async fn with_titles() -> Self {
        Self::with_provider_gating(Some("title"), 1, false, None).await
    }

    pub(super) async fn with_anthropic_titles(title: &str) -> Self {
        Self::with_provider_gating(Some("title"), 1, false, Some(title.to_owned())).await
    }

    async fn with_provider_gating(
        tool: Option<&'static str>,
        count: usize,
        gate_summary: bool,
        anthropic_title: Option<String>,
    ) -> Self {
        let mut phases = FixturePhase::new("Session fixture construction");
        phases.enter("provider listener");
        let gates = [HeaderGate::new(), HeaderGate::new()];
        let server_gates = gates.clone();
        let anthropic = anthropic_title.is_some();
        let provider = FixtureServer::start_with_body(move |_, _, body| {
            let request: serde_json::Value = serde_json::from_str(body).unwrap();
            let index = if tool == Some("title") || gate_summary {
                usize::from(request["tools"].as_array().is_none_or(Vec::is_empty))
            } else if tool == Some("delivery-child") {
                delivery_child_input_index(&request)
            } else {
                usize::from(body.contains("request-B"))
            };
            if let Some(title) = &anthropic_title {
                let thinking = request["thinking"]["budget_tokens"].as_u64().unwrap();
                let max_tokens = request["max_tokens"].as_u64().unwrap();
                assert!(thinking >= 1024 && thinking < max_tokens, "valid native thinking budget: {request}");
                assert_eq!(max_tokens, 4096);
                assert!(request["tools"].as_array().is_none_or(Vec::is_empty));
                let events = [
                    ("message_start", serde_json::json!({"type":"message_start","message":{"id":"title","type":"message","role":"assistant","model":"a","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":0}}})),
                    ("content_block_start", serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}})),
                    ("content_block_delta", serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking": "Reasoning about the task and an appropriate concise label. ".repeat(12)}})),
                    ("content_block_delta", serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signature"}})),
                    ("content_block_stop", serde_json::json!({"type":"content_block_stop","index":0})),
                    ("content_block_start", serde_json::json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}})),
                    ("content_block_delta", serde_json::json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":title}})),
                    ("content_block_stop", serde_json::json!({"type":"content_block_stop","index":1})),
                    ("message_delta", serde_json::json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":1100}})),
                    ("message_stop", serde_json::json!({"type":"message_stop"})),
                ];
                let mut stream = String::new();
                for (event, data) in events {
                    write!(stream, "event: {event}\ndata: {data}\n\n").unwrap();
                }
                return FixtureReply::body(200, "OK", "text/event-stream", stream).with_header_gate(server_gates[1].clone());
            }
            if tool == Some("delivery-child") && request["tools"].as_array().is_some_and(|tools| tools.iter().any(|tool| tool["function"]["name"] == "subagent"))
                && !request["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool") {
                let arguments = serde_json::json!({"title":"Delivery child","agent":"reviewer","task":"request-B"});
                return tool_call_reply("child-delivery", "subagent", &arguments.to_string()).with_header_gate(server_gates[index].clone());
            }
            if tool == Some("review") {
                let offers = |name: &str| request["tools"].as_array().unwrap().iter().any(|tool| tool["function"]["name"] == name);
                let child = offers("workflow_output");
                let messages = request["messages"].as_array().unwrap();
                // Root calls the Workflow only when its adopted closure offers it.
                if child || (offers("review") && messages.last().is_some_and(|message| message["role"] == "user")) {
                    let name = if child { "workflow_output" } else { "review" };
                    return tool_call_reply("workflow", name, "{}").with_header_gate(server_gates[index].clone());
                }
            }
            if let Some(name) = tool
                && !matches!(name, "review" | "delivery-child" | "title")
                && !request["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool") {
                let arguments = if name == "ask_user" {
                    serde_json::json!({"questions":[{"question":"Continue?", "header":"Decision", "options":[
                        {"label":"Continue", "description":"Proceed with the test"},
                        {"label":"Stop", "description":"Do not proceed"}]}]})
                } else if name == "present" { serde_json::json!({"files":[{"path":"报告 file.md", "description":"Declared report"}]}) } else { serde_json::json!({"path":"rustx.toml"}) };
                return tool_call_reply("interaction", name, &arguments.to_string()).with_header_gate(server_gates[index].clone());
            }
            FixtureReply::body(200, "OK", "text/event-stream", concat!(
                "data: {\"id\":\"response\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"a\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"response\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"a\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n")).with_header_gate(server_gates[index].clone())
        }).await;
        phases.enter("workspace and configuration");
        if tool == Some("delivery-child") {
            eprintln!("ChildDelivery Fixture: listener ready; native initialization");
        }
        // Match the runtime's canonical workspace identity, including macOS's
        // /var -> /private/var temporary-directory alias. Keep strict cwd checks.
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let workspaces = [root.path().join("a"), root.path().join("b")];
        for (workspace, marker) in workspaces.iter().zip(["A", "B"]) {
            std::fs::create_dir(workspace).unwrap();
            std::fs::write(
                workspace.join("rustx.toml"),
                format!("[environment]\nRESIDENCY_SESSION = \"{marker}\"\n"),
            )
            .unwrap();
        }
        let host =
            HostEnvironment::from_paths(workspaces[0].clone(), root.path().join("home")).unwrap();
        let args = [
            "--template",
            "openai-chat",
            "--provider",
            "local",
            "--model-id",
            "a",
            "--endpoint",
            &provider.url("/v1"),
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
        ]
        .map(str::to_owned);
        let crate::local_runtime::cli::Command::Init { request, .. } =
            crate::local_runtime::cli::parse_command(std::iter::once("init".into()).chain(args))
                .unwrap()
        else {
            panic!("initialization intent")
        };
        let mut documents = crate::local_runtime::initialization::documents(&request).unwrap();
        // Two identities, identical text-only production adapter capabilities.
        // Admission races can change the selected model without inventing a
        // multimodal adapter or bypassing the real request validator.
        let mut catalog: toml::Value =
            toml::from_str(std::str::from_utf8(&documents[0]).unwrap()).unwrap();
        if anthropic {
            let model = &mut catalog["models"]["local/a"];
            model["protocol"] = toml::Value::String("anthropic_messages".into());
            model["capabilities"]["reasoning"] = toml::Value::Boolean(true);
            model.as_table_mut().unwrap().remove("compat");
            model.as_table_mut().unwrap().insert(
                "request_params".into(),
                toml::Value::String(
                    r#"{"thinking":{"type":"enabled","budget_tokens":1024}}"#.into(),
                ),
            );
        }
        let mut alternate = catalog["models"]["local/a"].clone();
        alternate["id"] = toml::Value::String("b".into());
        catalog["models"]
            .as_table_mut()
            .unwrap()
            .insert("local/b".into(), alternate);
        catalog["agent"].as_table_mut().unwrap().insert("tools".into(),
            toml::Value::try_from(serde_json::json!({"builtin":["read", "write", "edit", "glob", "grep", "bash", "present", "ask_user", "job_status"]})).unwrap());
        documents[0] = toml::to_string(&catalog).unwrap().into_bytes();
        crate::local_runtime::initialization::initialize(&host, &documents);
        phases.enter("launch analysis");
        let paths = launch::analyze(&LaunchRequest::default(), &host)
            .unwrap()
            .admit(CredentialSnapshot::default)
            .unwrap();
        if tool == Some("delivery-child") {
            eprintln!("ChildDelivery Fixture: configuration ready; catalog open");
        }
        let controller = SessionController::open(&paths.runtime_root).unwrap();
        let mut sessions = Vec::new();
        for (ordinal, workspace) in workspaces.iter().cycle().take(count).enumerate() {
            phases.enter("create native Session");
            if tool == Some("delivery-child") {
                eprintln!("ChildDelivery Fixture: create native Session {ordinal}");
            }
            sessions.push(
                controller
                    .create_session(SessionPersistentState::from_input(
                        &SessionConfigInput::new(workspace.clone()),
                    ))
                    .await
                    .unwrap()
                    .session,
            );
            if tool != Some("title") {
                // These fixtures prove primary execution/transport contracts.
                // A saved manual label suppresses independent optional inference.
                let session = sessions.last_mut().expect("created Session");
                *session = controller
                    .rename_session(&session.id, &format!("Fixture {ordinal}"))
                    .await
                    .unwrap();
            }
        }
        if tool == Some("delivery-child") {
            eprintln!("ChildDelivery Fixture: Sessions created; manager wiring");
        }
        let archive_root = paths.runtime_root.clone();
        phases.enter("manager construction");
        let manager = SessionRuntimeManager::new(
            controller,
            UserConfigManager::new(paths.sources.clone()).unwrap(),
            CredentialSnapshot::new([("TEST_KEY".into(), "fixture".into())]),
            LocalRuntimeDependencies {
                child_program: matches!(tool, Some("review" | "delivery-child")).then(|| {
                    std::env::current_exe()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .join("rustx")
                }),
                ..LocalRuntimeDependencies::default()
            },
            RuntimeResidencyPolicy {
                max_resident_runtimes: 8,
                idle_grace_ms: 300_000,
            },
        )
        .unwrap();
        phases.finish();
        Self {
            _root: root,
            archive_root,
            host: crate::app_server::host::AppServerHost::new(
                manager.clone(),
                crate::local_runtime::app_server_policy::AppServerPolicy::default(),
            ),
            manager,
            sessions,
            gates,
            provider,
            workspaces,
        }
    }
    pub(super) async fn id(&self, index: usize) -> ConversationId {
        self.manager
            .sessions
            .acquire_session(&self.sessions[index].id, None)
            .await
            .unwrap()
            .node
            .conversation_id
    }
    pub(super) fn load(
        &self,
        index: usize,
    ) -> tokio::task::JoinHandle<Result<Arc<ManagedRuntime>, RuntimeManagerError>> {
        let manager = self.manager.clone();
        let id = self.sessions[index].id.clone();
        tokio::spawn(async move { manager.load(&id, None).await })
    }
    pub(super) async fn close(&self) {
        for gate in &self.gates {
            gate.release();
        }
        for i in 0..self.sessions.len() {
            if let Ok(access) = self
                .manager
                .sessions
                .acquire_session(&self.sessions[i].id, None)
                .await
            {
                self.manager
                    .unload(&access.node.conversation_id)
                    .await
                    .unwrap();
            }
        }
    }
}

/// One streamed OpenAI-chat completion that proposes a single tool call
/// `<id>-call` and finishes with `tool_calls`.
fn tool_call_reply(id: &str, name: &str, arguments: &str) -> FixtureReply {
    let chunk = serde_json::json!({"id":id,"object":"chat.completion.chunk","created":1,"model":"a",
        "choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":format!("{id}-call"),"type":"function",
            "function":{"name":name,"arguments":arguments}}]},"finish_reason":"tool_calls"}]});
    FixtureReply::body(
        200,
        "OK",
        "text/event-stream",
        format!("data: {chunk}\n\ndata: [DONE]\n\n"),
    )
}

/// Provider fixtures distinguish ordinary task text from model-visible Context
/// that mentions it. Both native OpenAI-chat text representations are accepted.
fn delivery_child_input_index(request: &serde_json::Value) -> usize {
    usize::from(
        request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["role"] == "user"
                    && (message["content"].as_str() == Some("request-B")
                        || message["content"].as_array().is_some_and(|parts| {
                            parts.iter().any(|part| {
                                part["type"] == "text" && part["text"].as_str() == Some("request-B")
                            })
                        }))
            }),
    )
}

#[test]
fn delivery_child_gate_uses_exact_task_text_not_parent_context_mentions() {
    for context in [
        serde_json::json!("AgentStatus: Delivery Child task request-B"),
        serde_json::json!([{"type":"text","text":"AgentStatus: Delivery Child task request-B"}]),
    ] {
        let parent = serde_json::json!({"messages":[{"role":"user","content":"request-A"},{"role":"user","content":context}]});
        assert_eq!(delivery_child_input_index(&parent), 0);
    }
    for content in [
        serde_json::json!("request-B"),
        serde_json::json!([{"type":"text","text":"request-B"}]),
    ] {
        let child = serde_json::json!({"messages":[{"role":"user","content":"request-A"},{"role":"user","content":content}]});
        assert_eq!(delivery_child_input_index(&child), 1);
    }
    let tool = serde_json::json!({"messages":[{"role":"tool","content":"request-B"},{"role":"user","content":"request-A"}]});
    assert_eq!(delivery_child_input_index(&tool), 0);
}

pub(super) async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(30), future)
        .await
        .expect("test liveness")
}

pub(super) fn events(runtime: &ConversationRuntime) -> Vec<RuntimeEvent> {
    runtime
        .tool_runtime()
        .durable_store()
        .read_events(None, 256)
        .unwrap()
        .events
        .into_iter()
        .map(|e| e.event)
        .collect()
}

pub(super) async fn gate_entered(gate: &Arc<crate::runtime::conversation_runtime::Gate>) {
    let gate = gate.clone();
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
}
