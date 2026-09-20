//! Residency races at the real manager, native composition, provider and
//! allocation boundaries. Timeouts are liveness guards, never race evidence.
#![allow(clippy::too_many_lines)]
#[path = "../../support/app_server_conformance.rs"]
mod app_server_conformance;
mod configuration;
mod inbound_model;
mod protocol;
mod residency_policy;
mod transports;
use super::*;
use crate::events::types::RuntimeEvent;
use crate::local_runtime::configuration::SessionConfigInput;
use crate::local_runtime::launch::{self, HostEnvironment, LaunchRequest};
use crate::local_runtime::session::{SessionPersistentState, SessionSnapshot};
use crate::message::content::TextBlock;
use crate::message::types::UserContentBlock;
use crate::scripted_suites::common::{FixtureReply, FixtureServer, HeaderGate};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
pub(super) struct AsyncGate {
    entered: watch::Sender<bool>,
    release: watch::Sender<bool>,
}
impl Default for AsyncGate {
    fn default() -> Self {
        Self {
            entered: watch::channel(false).0,
            release: watch::channel(true).0,
        }
    }
}
impl AsyncGate {
    fn arm(&self) {
        self.entered.send_replace(false);
        self.release.send_replace(false);
    }
    pub(super) async fn park(&self) {
        self.entered.send_replace(true);
        self.release.subscribe().wait_for(|v| *v).await.unwrap();
    }
    async fn entered(&self) {
        self.entered.subscribe().wait_for(|v| *v).await.unwrap();
    }
    fn release(&self) {
        self.release.send_replace(true);
    }
}
#[derive(Debug)]
pub(super) struct Probe {
    pub(super) configuration_preparations: AtomicUsize,
    pub(super) fail_configuration_once: std::sync::atomic::AtomicBool,
    pub(super) after_configuration_persistence: AsyncGate,
    pub(super) before_configuration_prepare: AsyncGate,
    pub(super) before_configuration_publish: AsyncGate,
    pub(super) idle_before_claim: Arc<crate::runtime::conversation_runtime::Gate>,
    pub(super) idle_after_claim: Arc<crate::runtime::conversation_runtime::Gate>,
    pub(super) activation: Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>,
    pub(super) compositions: AtomicUsize,
    pub(super) acquisitions: AtomicUsize,
    pub(super) before_allocation: AsyncGate,
    pub(super) after_writer_transfer: AsyncGate,
    pub(super) joined: watch::Sender<usize>,
    pub(super) unloads_joined: watch::Sender<usize>,
    pub(super) fail_compose_once: std::sync::atomic::AtomicBool,
    pub(super) panic_once: std::sync::atomic::AtomicBool,
    pub(super) before_compose: AsyncGate,
    pub(super) after_delete_fence: AsyncGate,
    pub(super) before_shutdown: AsyncGate,
    pub(super) before_operation: AsyncGate,
    pub(super) draining_operations: watch::Sender<bool>,
}
impl Default for Probe {
    fn default() -> Self {
        Self {
            configuration_preparations: AtomicUsize::new(0),
            fail_configuration_once: std::sync::atomic::AtomicBool::new(false),
            after_configuration_persistence: AsyncGate::default(),
            before_configuration_prepare: AsyncGate::default(),
            before_configuration_publish: AsyncGate::default(),
            idle_before_claim: Arc::default(),
            idle_after_claim: Arc::default(),
            activation: Mutex::new(None),
            compositions: AtomicUsize::new(0),
            acquisitions: AtomicUsize::new(0),
            before_allocation: AsyncGate::default(),
            after_writer_transfer: AsyncGate::default(),
            joined: watch::channel(0).0,
            unloads_joined: watch::channel(0).0,
            fail_compose_once: std::sync::atomic::AtomicBool::new(false),
            panic_once: std::sync::atomic::AtomicBool::new(false),
            before_compose: AsyncGate::default(),
            after_delete_fence: AsyncGate::default(),
            before_shutdown: AsyncGate::default(),
            before_operation: AsyncGate::default(),
            draining_operations: watch::channel(false).0,
        }
    }
}
impl Probe {
    async fn joined(&self, count: usize) {
        self.joined
            .subscribe()
            .wait_for(|n| *n >= count)
            .await
            .unwrap();
    }
}
struct Fixture {
    _root: tempfile::TempDir,
    archive_root: std::path::PathBuf,
    manager: SessionRuntimeManager,
    host: crate::app_server::host::AppServerHost,
    sessions: [SessionSnapshot; 2],
    gates: [Arc<HeaderGate>; 2],
    provider: FixtureServer,
    workspaces: [std::path::PathBuf; 2],
}
impl Fixture {
    async fn new() -> Self {
        Self::with_tool(None).await
    }

    async fn with_tool(tool: Option<&'static str>) -> Self {
        let gates = [HeaderGate::new(), HeaderGate::new()];
        let server_gates = gates.clone();
        let provider = FixtureServer::start_with_body(move |_, _, body| {
            let index = usize::from(body.contains("request-B"));
            let request: serde_json::Value = serde_json::from_str(body).unwrap();
            if tool == Some("review") {
                let child = request["tools"].as_array().unwrap().iter().any(|tool| tool["function"]["name"] == "workflow_output");
                let messages = request["messages"].as_array().unwrap();
                if child || messages.last().is_some_and(|message| message["role"] == "user") {
                    let name = if child { "workflow_output" } else { "review" };
                    let chunk = serde_json::json!({"id":"workflow","object":"chat.completion.chunk","created":1,"model":"a",
                        "choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"workflow-call","type":"function",
                            "function":{"name":name,"arguments":"{}"}}]},"finish_reason":"tool_calls"}]});
                    return FixtureReply::body(200,"OK","text/event-stream",format!("data: {chunk}\n\ndata: [DONE]\n\n"))
                        .with_header_gate(server_gates[index].clone());
                }
            }
            if let Some(name) = tool
                && name != "review"
                && !request["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool") {
                let arguments = if name == "ask_user" {
                    serde_json::json!({"questions":[{"question":"Continue?", "header":"Decision", "options":[
                        {"label":"Continue", "description":"Proceed with the test"},
                        {"label":"Stop", "description":"Do not proceed"}]}]})
                } else { serde_json::json!({"path":"rustx.toml"}) };
                let chunk = serde_json::json!({"id":"interaction","object":"chat.completion.chunk","created":1,"model":"a",
                    "choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"interaction-call","type":"function",
                        "function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]});
                return FixtureReply::body(200,"OK","text/event-stream",format!("data: {chunk}\n\ndata: [DONE]\n\n"))
                    .with_header_gate(server_gates[index].clone());
            }
            FixtureReply::body(200, "OK", "text/event-stream", concat!(
                "data: {\"id\":\"response\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"a\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"response\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"a\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n")).with_header_gate(server_gates[index].clone())
        }).await;
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
        let mut documents = crate::local_runtime::initialization::documents(&args).unwrap();
        // Two identities, identical text-only production adapter capabilities.
        // Admission races can change the selected model without inventing a
        // multimodal adapter or bypassing the real request validator.
        let mut catalog: toml::Value =
            toml::from_str(std::str::from_utf8(&documents[0]).unwrap()).unwrap();
        let mut alternate = catalog["models"]["local/a"].clone();
        alternate["id"] = toml::Value::String("b".into());
        catalog["models"]
            .as_table_mut()
            .unwrap()
            .insert("local/b".into(), alternate);
        catalog["agent"].as_table_mut().unwrap().insert("tools".into(),
            toml::Value::try_from(serde_json::json!({"builtin":["read", "write", "edit", "glob", "grep", "bash", "ask_user", "execution"]})).unwrap());
        documents[0] = toml::to_string(&catalog).unwrap().into_bytes();
        crate::local_runtime::initialization::initialize(&host, &documents);
        let paths = launch::analyze(&LaunchRequest::default(), &host)
            .unwrap()
            .admit(CredentialSnapshot::default)
            .unwrap();
        let controller = SessionController::open(&paths.runtime_root).unwrap();
        let mut sessions = Vec::new();
        for workspace in &workspaces {
            sessions.push(
                controller
                    .create_session(SessionPersistentState::from_input(
                        &SessionConfigInput::new(workspace.clone()),
                    ))
                    .await
                    .unwrap()
                    .session,
            );
        }
        let archive_root = paths.runtime_root.clone();
        let manager = SessionRuntimeManager::new(
            controller,
            UserConfigManager::new(paths.sources.clone()).unwrap(),
            CredentialSnapshot::new([("TEST_KEY".into(), "fixture".into())]),
            LocalRuntimeDependencies {
                child_program: (tool == Some("review")).then(|| {
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
        Self {
            _root: root,
            archive_root,
            host: crate::app_server::host::AppServerHost::new(
                manager.clone(),
                crate::local_runtime::app_server_policy::AppServerPolicy::default(),
            ),
            manager,
            sessions: sessions.try_into().unwrap(),
            gates,
            provider,
            workspaces,
        }
    }
    async fn id(&self, index: usize) -> ConversationId {
        self.manager
            .sessions
            .acquire_session(&self.sessions[index].id, None)
            .await
            .unwrap()
            .node
            .conversation_id
    }
    fn load(
        &self,
        index: usize,
    ) -> tokio::task::JoinHandle<Result<Arc<ManagedRuntime>, RuntimeManagerError>> {
        let manager = self.manager.clone();
        let id = self.sessions[index].id.clone();
        tokio::spawn(async move { manager.load(&id, None).await })
    }
    async fn close(&self) {
        for i in 0..2 {
            self.gates[i].release();
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
fn input(text: &str) -> Vec<UserContentBlock> {
    vec![UserContentBlock::Text(TextBlock { text: text.into() })]
}
async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(30), future)
        .await
        .expect("test liveness")
}
fn events(runtime: &ConversationRuntime) -> Vec<RuntimeEvent> {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_id_single_flight_and_cancelled_claimant() {
    bounded(async {
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        probe.before_compose.arm();
        let a = f.load(0);
        probe.before_compose.entered().await;
        let b = f.load(0);
        probe.joined(2).await;
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        assert!(!a.is_finished() && !b.is_finished());
        a.abort();
        assert!(a.await.unwrap_err().is_cancelled());
        let c = f.load(0);
        probe.joined(3).await;
        probe.before_compose.release();
        let b = b.await.unwrap().unwrap();
        let c = c.await.unwrap().unwrap();
        assert!(Arc::ptr_eq(&b, &c));
        assert!(f.manager.is_current(&id, b.incarnation_id()));
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_flight_is_shared_retryable_and_isolated_from_running_b() {
    bounded(async {
        let f = Fixture::new().await;
        let b = f.load(1).await.unwrap().unwrap();
        let live_b = b.inspect_runtime().unwrap();
        live_b.submit_inbound(input("request-B")).unwrap();
        f.gates[1].wait_entered().await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        probe.before_compose.arm();
        std::fs::write(f.workspaces[0].join("rustx.toml"), "invalid = [").unwrap();
        let a = f.load(0);
        probe.before_compose.entered().await;
        let second = f.load(0);
        probe.joined(2).await;
        probe.before_compose.release();
        let error = a.await.unwrap().unwrap_err();
        assert_eq!(error, second.await.unwrap().unwrap_err());
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        let settled = live_b.settlement_signal().notified();
        f.gates[1].release();
        settled.await;
        std::fs::remove_file(f.workspaces[0].join("rustx.toml")).unwrap();
        f.load(0).await.unwrap().unwrap();
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 2);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn different_conversations_overlap_provider_and_keep_durable_state_isolated() {
    bounded(async {
        let f = Fixture::new().await;
        let a = f.load(0).await.unwrap().unwrap();
        let live_a = a.inspect_runtime().unwrap();
        let (attachment, _) = a
            .resident
            .upgrade()
            .unwrap()
            .composition
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .host()
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        live_a.submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        // The actual admitted attachment disconnects during the provider turn.
        drop(attachment);
        let b = f.load(1).await.unwrap().unwrap();
        let live_b = b.inspect_runtime().unwrap();
        live_b.submit_inbound(input("request-B")).unwrap();
        f.gates[1].wait_entered().await;
        assert_eq!(f.manager.diagnostics().active_roots, 2);
        assert_eq!(f.manager.diagnostics().loaded, 2);
        assert_eq!(f.provider.request_bodies().len(), 2);
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        assert_eq!(
            f.manager.residency(b.conversation_id()),
            ResidencyState::Loaded
        );
        assert_ne!(a.workspace_identity(), b.workspace_identity());
        assert!(!Arc::ptr_eq(
            &live_a.runtime_resources(),
            &live_b.runtime_resources()
        ));
        for (runtime, marker) in [(&live_a, "A"), (&live_b, "B")] {
            assert!(
                runtime
                    .tool_runtime()
                    .environment()
                    .child_environment(runtime.tool_runtime().workspace().root())
                    .contains(&("RESIDENCY_SESSION".to_owned(), marker.to_owned()))
            );
            assert!(
                runtime
                    .tool_runtime()
                    .durable_store()
                    .read_events(None, 256)
                    .unwrap()
                    .events
                    .iter()
                    .all(|event| &event.conversation_id == runtime.conversation_id())
            );
        }
        assert_eq!(live_a.tool_runtime().workspace().root(), f.workspaces[0]);
        assert_eq!(live_b.tool_runtime().workspace().root(), f.workspaces[1]);
        let b_done = live_b.settlement_signal().notified();
        f.gates[1].release();
        b_done.await;
        assert!(
            !events(&live_a)
                .iter()
                .any(|e| matches!(e, RuntimeEvent::AttemptCompleted { .. }))
        );
        assert!(
            live_a.has_current_attempt(),
            "A still owns its blocked provider turn after B completes"
        );
        assert!(
            events(&live_b)
                .iter()
                .any(|e| matches!(e, RuntimeEvent::AttemptCompleted { .. }))
        );
        let a_done = live_a.settlement_signal().notified();
        f.gates[0].release();
        a_done.await;
        assert_eq!(
            events(&live_a)
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::AttemptCompleted { .. }))
                .count(),
            1
        );
        let history_a = live_a.historical_canonical_history().unwrap();
        let history_b = live_b.historical_canonical_history().unwrap();
        assert!(format!("{history_a:?}").contains("request-A"));
        assert!(!format!("{history_a:?}").contains("request-B"));
        assert!(format!("{history_b:?}").contains("request-B"));
        assert!(!format!("{history_b:?}").contains("request-A"));
        let (_reattached, _) = a
            .resident
            .upgrade()
            .unwrap()
            .composition
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .host()
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        assert_ne!(live_a.conversation_id(), live_b.conversation_id());
        assert_ne!(
            live_a
                .tool_runtime()
                .durable_store()
                .read_events(None, 256)
                .unwrap()
                .events,
            live_b
                .tool_runtime()
                .durable_store()
                .read_events(None, 256)
                .unwrap()
                .events
        );
        f.close().await;
    })
    .await;
}

fn unload_task(
    f: &Fixture,
    id: ConversationId,
) -> tokio::task::JoinHandle<Result<(), RuntimeManagerError>> {
    let manager = f.manager.clone();
    tokio::spawn(async move { manager.unload(&id).await })
}
fn replace_task(
    f: &Fixture,
    index: usize,
) -> tokio::task::JoinHandle<Result<Arc<ManagedRuntime>, RuntimeManagerError>> {
    let manager = f.manager.clone();
    let session = f.sessions[index].id.clone();
    tokio::spawn(async move { manager.replace(&session, None).await })
}
async fn gate_entered(gate: &Arc<crate::runtime::conversation_runtime::Gate>) {
    let gate = gate.clone();
    tokio::task::spawn_blocking(move || gate.wait_entered())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unload_waits_for_load_then_load_waits_for_unload() {
    bounded(async {
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        probe.before_compose.arm();
        let first = f.load(0);
        probe.before_compose.entered().await;
        let unload = unload_task(&f, id.clone());
        probe
            .unloads_joined
            .subscribe()
            .wait_for(|n| *n == 1)
            .await
            .unwrap();
        // Park unload after its residency claim and before native shutdown.
        probe.before_shutdown.arm();
        probe.before_compose.release();
        let first = first.await.unwrap().unwrap();
        let old = first.inspect_runtime().unwrap();
        probe.before_shutdown.entered().await;
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloading);
        let reload = f.load(0);
        probe.joined(2).await;
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        assert!(!reload.is_finished());
        // Dropping the unload caller does not abandon its claimed transition.
        unload.abort();
        let _ = unload.await;
        probe.before_shutdown.release();
        let second = reload.await.unwrap().unwrap();
        assert_ne!(first.incarnation_id(), second.incarnation_id());
        assert!(old.submit_inbound(input("stale")).is_err());
        assert!(first.inspect_runtime().is_none());
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 2);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unload_and_inbound_have_both_native_admission_winners() {
    bounded(async {
        use crate::runtime::conversation_runtime::Gate;
        let f = Fixture::new().await;
        let managed = f.load(0).await.unwrap().unwrap();
        let live = managed.inspect_runtime().unwrap();
        let submit_gate = Arc::new(Gate::default());
        let release = submit_gate.arm_scoped();
        live.install_residency_probe(Some(submit_gate.clone()), None);
        let arrival = Arc::new(tokio::sync::Notify::new());
        let drained = Arc::new(tokio::sync::Notify::new());
        live.install_drain_signals(arrival.clone(), drained.clone());
        let inbound = live.clone();
        let submit =
            tokio::task::spawn_blocking(move || inbound.submit_inbound(input("request-A")));
        gate_entered(&submit_gate).await;
        let unload = unload_task(&f, managed.conversation_id().clone());
        arrival.notified().await; // Shutdown has arrived, submit holds coordinator.
        assert!(!unload.is_finished());
        drop(release);
        assert_eq!(submit.await.unwrap().unwrap().inbound_sequence.get(), 1);
        unload.await.unwrap().unwrap();
        assert!(live.submit_inbound(input("late-A")).is_err());
        // Opposite ordering: native drain CAS signals before inbound is released.
        let managed = f.load(0).await.unwrap().unwrap();
        let live = managed.inspect_runtime().unwrap();
        let drained = Arc::new(tokio::sync::Notify::new());
        live.install_drain_signals(Arc::new(tokio::sync::Notify::new()), drained.clone());
        let unload = unload_task(&f, managed.conversation_id().clone());
        drained.notified().await;
        assert!(live.submit_inbound(input("late-B")).is_err());
        unload.await.unwrap().unwrap();
        assert_eq!(
            f.manager.residency(managed.conversation_id()),
            ResidencyState::Unloaded
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacement_waits_for_active_attempt_task_and_changes_only_incarnation_a() {
    bounded(async {
        use crate::runtime::conversation_runtime::Gate;
        use crate::runtime::types::ConversationLifecycleState;
        let f = Fixture::new().await;
        let a = f.load(0).await.unwrap().unwrap();
        let old = a.inspect_runtime().unwrap();
        let b = f.load(1).await.unwrap().unwrap();
        let gate = Arc::new(Gate::default());
        let release = gate.arm_scoped();
        old.install_residency_probe(None, Some(gate.clone()));
        let drain = Arc::new(tokio::sync::Notify::new());
        old.install_drain_signals(Arc::new(tokio::sync::Notify::new()), drain.clone());
        old.submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        let replacement = replace_task(&f, 0);
        drain.notified().await;
        gate_entered(&gate).await;
        assert_eq!(old.lifecycle_state(), ConversationLifecycleState::Draining);
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Unloading
        );
        assert!(!replacement.is_finished());
        assert_eq!(
            f.manager
                .probe(a.conversation_id())
                .compositions
                .load(Ordering::SeqCst),
            1
        );
        assert!(
            !f.manager
                .is_current(a.conversation_id(), a.incarnation_id())
        );
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        drop(release);
        let new = replacement.await.unwrap().unwrap();
        assert_eq!(new.conversation_id(), a.conversation_id());
        assert_ne!(new.incarnation_id(), a.incarnation_id());
        assert!(
            f.manager
                .is_current(new.conversation_id(), new.incarnation_id())
        );
        assert!(old.submit_inbound(input("stale")).is_err());
        assert_eq!(
            a.client().snapshot().unwrap_err(),
            RuntimeManagerError::StaleIncarnation
        );
        assert!(Arc::ptr_eq(&b, &f.load(1).await.unwrap().unwrap()));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacement_failure_is_unloaded_and_retryable_but_shutdown_failure_retains_owner() {
    bounded(async {
        let f = Fixture::new().await;
        let a = f.load(0).await.unwrap().unwrap();
        let old = a.inspect_runtime().unwrap();
        let b = f.load(1).await.unwrap().unwrap();
        let live_b = b.inspect_runtime().unwrap();
        live_b.submit_inbound(input("request-B")).unwrap();
        f.gates[1].wait_entered().await;
        f.manager
            .probe(a.conversation_id())
            .fail_compose_once
            .store(true, Ordering::SeqCst);
        assert!(replace_task(&f, 0).await.unwrap().is_err());
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Unloaded
        );
        assert!(old.submit_inbound(input("old")).is_err());
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        let a = f.load(0).await.unwrap().unwrap();
        a.inspect_runtime().unwrap().fail_residency_settlement();
        let failure = f.manager.unload(a.conversation_id()).await.unwrap_err();
        assert!(
            failure
                .to_string()
                .contains("residency test settlement failure")
        );
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Unloading
        );
        assert!(a.inspect_runtime().is_some());
        assert_eq!(f.load(0).await.unwrap().unwrap_err(), failure);
        assert_eq!(replace_task(&f, 0).await.unwrap().unwrap_err(), failure);
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        let done = live_b.settlement_signal().notified();
        f.gates[1].release();
        done.await;
        f.manager.unload(b.conversation_id()).await.unwrap();
        // Native unproven settlement deliberately remains retained, not a fake
        // successful unload. Process teardown is the terminal failure policy.
    })
    .await;
}

async fn deletion_revision(f: &Fixture, index: usize) -> String {
    let crate::local_runtime::session::deletion::SessionDeleteResult::Preview { preview } = f
        .manager
        .sessions
        .delete_preview(&f.sessions[index].id)
        .await
    else {
        panic!("preview")
    };
    preview.target_revision
}
fn delete_task(
    f: &Fixture,
    index: usize,
    revision: String,
) -> tokio::task::JoinHandle<
    Result<crate::local_runtime::session::deletion::SessionDeleteResult, RuntimeManagerError>,
> {
    let manager = f.manager.clone();
    let id = f.sessions[index].id.clone();
    tokio::spawn(async move { manager.delete_session(&id, &revision).await })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_fence_prevents_loading_publication_and_isolates_other_sessions() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        let revision = deletion_revision(&f, 0).await;
        probe.before_compose.arm();
        let load = f.load(0);
        probe.before_compose.entered().await;
        // Joining callers hold no redundant allocation across retirement.
        let joined_load = f.load(0);
        probe.joined(2).await;
        // Preview remains available while a load owns its allocation.
        assert_eq!(deletion_revision(&f, 0).await, revision);
        probe.after_delete_fence.arm();
        let delete = delete_task(&f, 0, revision);
        probe.after_delete_fence.entered().await;
        assert!(f.load(0).await.unwrap().is_err());
        let other = f.load(1).await.unwrap().unwrap();
        assert!(other.client().validate().is_ok());
        probe.before_compose.release();
        probe.after_delete_fence.release();
        assert!(load.await.unwrap().is_err());
        assert!(joined_load.await.unwrap().is_err());
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        assert!(other.client().validate().is_ok());
        assert!(f.load(0).await.unwrap().is_err());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_fence_rejects_late_operation_attach_and_replacement() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let runtime = f.load(0).await.unwrap().unwrap();
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(runtime.conversation_id());
        probe.after_delete_fence.arm();
        let delete = delete_task(&f, 0, revision);
        probe.after_delete_fence.entered().await;
        assert!(
            runtime
                .client()
                .submit_inbound(input("never admitted"))
                .is_err()
        );
        assert!(runtime.client().attach().is_err());
        assert!(replace_task(&f, 0).await.unwrap().is_err());
        assert!(f.load(0).await.unwrap().is_err());
        probe.after_delete_fence.release();
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert!(runtime.inspect_runtime().is_none());
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_joins_replacement_without_publishing_its_candidate() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let old = f.load(0).await.unwrap().unwrap();
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(old.conversation_id());
        probe.before_shutdown.arm();
        let replace = replace_task(&f, 0);
        probe.before_shutdown.entered().await;
        probe.after_delete_fence.arm();
        let delete = delete_task(&f, 0, revision);
        probe.after_delete_fence.entered().await;
        probe.before_shutdown.release();
        probe.after_delete_fence.release();
        assert!(replace.await.unwrap().is_err());
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert!(old.inspect_runtime().is_none());
        assert!(f.load(0).await.unwrap().is_err());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_retirement_failure_retains_writer_slot_and_session_fence() {
    bounded(async {
        let f = Fixture::new().await;
        let runtime = f.load(0).await.unwrap().unwrap();
        let revision = deletion_revision(&f, 0).await;
        runtime
            .inspect_runtime()
            .unwrap()
            .fail_residency_settlement();
        let result = f.manager.delete_session(&f.sessions[0].id, &revision).await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("residency test settlement failure")
        );
        let flight = {
            let registry = f.manager.registry.0.lock().unwrap();
            assert!(registry.retiring_sessions.contains(&f.sessions[0].id));
            let Some(Entry::Unloading { flight, .. }) =
                registry.entries.get(runtime.conversation_id())
            else {
                panic!("unproven writer slot")
            };
            flight.clone()
        };
        assert!(matches!(
            flight.wait().await,
            Outcome::RetirementUnproven(_)
        ));
        assert!(
            f.manager
                .sessions
                .read_session(&f.sessions[0].id)
                .await
                .is_ok()
        );
        assert_eq!(
            f.manager.residency(runtime.conversation_id()),
            ResidencyState::Unloading
        );
        assert!(runtime.inspect_runtime().is_some());
        assert!(f.load(0).await.unwrap().is_err());
        assert!(replace_task(&f, 0).await.unwrap().is_err());
        assert!(matches!(
            f.manager
                .recover_session_deletion(&f.sessions[0].id)
                .await
                .unwrap(),
            crate::local_runtime::session::deletion::SessionDeleteResult::Preview { .. }
        ));
        assert!(
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(&f.sessions[0].id)
        );
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(f.manager.diagnostics().unloading, 1);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_settles_an_active_native_attempt() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let runtime = f.load(0).await.unwrap().unwrap();
        runtime.client().submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        let revision = deletion_revision(&f, 0).await;
        let live = runtime.inspect_runtime().unwrap();
        let weak = live.weak_inner();
        let arrival = Arc::new(tokio::sync::Notify::new());
        let drained = Arc::new(tokio::sync::Notify::new());
        live.install_drain_signals(arrival.clone(), drained.clone());
        drop(live);
        let result = f
            .manager
            .delete_session(&f.sessions[0].id, &revision)
            .await
            .unwrap();
        // Native coordinator Running -> Draining, not a catalog-only shortcut.
        arrival.notified().await;
        drained.notified().await;
        assert!(matches!(result, SessionDeleteResult::Deleted { .. }));
        assert!(weak.upgrade().is_none());
        assert!(runtime.inspect_runtime().is_none());
        f.gates[0].release();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_reload_recovers_accepted_history_without_another_provider_request_or_result() {
    bounded(async {
        let f = Fixture::new().await;
        let a = f.load(0).await.unwrap().unwrap();
        let live = a.inspect_runtime().unwrap();
        live.submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        let done = live.settlement_signal().notified();
        f.gates[0].release();
        done.await;
        let history = live.historical_canonical_history().unwrap();
        let journal = events(&live);
        f.manager.unload(a.conversation_id()).await.unwrap();
        let resumed = f.load(0).await.unwrap().unwrap();
        let current = resumed.inspect_runtime().unwrap();
        assert_ne!(a.incarnation_id(), resumed.incarnation_id());
        assert_eq!(current.historical_canonical_history().unwrap(), history);
        assert_eq!(events(&current), journal);
        assert_eq!(f.provider.request_bodies().len(), 1);
        // Drain is the liveness proof that activation had no hidden accepted
        // work to execute; no elapsed-time or empty poll establishes this.
        f.manager.unload(resumed.conversation_id()).await.unwrap();
        assert_eq!(f.provider.request_bodies().len(), 1);
        assert_eq!(
            events(&current)
                .iter()
                .filter(|e| matches!(e, RuntimeEvent::AttemptCompleted { .. }))
                .count(),
            1
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn panicked_composition_task_cleans_flight_and_warm_load_never_reresolves() {
    bounded(async {
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        probe.before_compose.arm();
        probe.panic_once.store(true, Ordering::SeqCst);
        let first = f.load(0);
        probe.before_compose.entered().await;
        let second = f.load(0);
        probe.joined(2).await;
        probe.before_compose.release();
        let failure = first.await.unwrap().unwrap_err();
        assert_eq!(failure, second.await.unwrap().unwrap_err());
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        let loaded = f.load(0).await.unwrap().unwrap();
        std::fs::write(f.workspaces[0].join("rustx.toml"), "invalid = [").unwrap();
        // The durable allocation authority refuses a second process owner;
        // cloned manager handles share this owner's live registry.
        assert!(
            SessionRuntimeManager::new(
                f.manager.sessions.clone(),
                f.manager.configuration.clone(),
                f.manager.credentials.clone(),
                LocalRuntimeDependencies::default(),
                f.manager.policy(),
            )
            .is_err()
        );
        let other = f.manager.clone();
        let warm = other.load(&f.sessions[0].id, None).await.unwrap();
        assert!(Arc::ptr_eq(&loaded, &warm));
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 2);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_composition_failure_does_not_touch_running_b() {
    bounded(async {
        let f = Fixture::new().await;
        let b = f.load(1).await.unwrap().unwrap();
        let live_b = b.inspect_runtime().unwrap();
        live_b.submit_inbound(input("request-B")).unwrap();
        f.gates[1].wait_entered().await;
        let access = f
            .manager
            .sessions
            .acquire_session(&f.sessions[0].id, None)
            .await
            .unwrap();
        // Real recovery/composition failure, beyond successful configuration.
        std::fs::write(&access.database_path, b"not a SQLite database").unwrap();
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(
            f.manager.residency(&access.node.conversation_id),
            ResidencyState::Unloaded
        );
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        let done = live_b.settlement_signal().notified();
        f.gates[1].release();
        done.await;
        f.manager.unload(b.conversation_id()).await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_load_preserves_unknown_external_tool_outcome_without_replay() {
    bounded(async {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        use crate::events::types::{RuntimeEventEnvelope, EVENT_SCHEMA_VERSION};
        use crate::runtime::identity::{AttemptId, EventId, MessageId, ToolCallId, ToolId, TurnId};
        use crate::message::types::{MessageBlock, AssistantMessageBlock, AssistantContentBlock};
        use crate::tools::types::{ToolCall, ToolExecutionStatus};
        let f = Fixture::new().await;
        let access = f.manager.sessions.acquire_session(&f.sessions[0].id, None).await.unwrap();
        let id = access.node.conversation_id.clone();
        let attempt = AttemptId::for_conversation(&id, 0);
        let envelope = |key: &str, event| RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION, event_id: EventId::new(key), sequence: 0,
            conversation_id: id.clone(), attempt_id: Some(attempt.clone()), turn_id: Some(TurnId::new("0")),
            timestamp: chrono::DateTime::from_timestamp(1, 0).unwrap(), event,
        };
        {
            // Exact committed crash prefix through the same SQLite authority
            // used by existing durable recovery contracts; no manager recovery.
            let store = SqliteConversationStore::open(id.clone(), &access.database_path).unwrap().with_lifecycle(access.allocation.clone());
            store.append_event(envelope("attempt", RuntimeEvent::AttemptStarted { attempt_id: attempt.clone() })).unwrap();
            let assistant = MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("assistant"), content: vec![AssistantContentBlock::ToolCall(ToolCall {
                    id: ToolCallId::new("call"), tool_id: ToolId::new("tool-write"), name: "write".into(),
                    arguments: serde_json::json!({"path":"must-not-exist", "content":"never replay"}),
                })],
            });
            store.append_canonical_with_event(&assistant, envelope("assistant", RuntimeEvent::AssistantMessageCommitted { message_id: MessageId::new("assistant") })).unwrap();
            store.append_event(envelope("tool", RuntimeEvent::ToolExecutionStarted { tool_call_id: ToolCallId::new("call"), tool_id: ToolId::new("tool-write") })).unwrap();
        }
        drop(access);
        let loaded = f.load(0).await.unwrap().unwrap(); let live = loaded.inspect_runtime().unwrap();
        let history = live.historical_canonical_history().unwrap();
        assert_eq!(history.iter().filter(|m| matches!(m, MessageBlock::Tool(tool) if matches!(tool.result.status, ToolExecutionStatus::OutcomeUnknown { .. }))).count(), 1);
        f.manager.unload(&id).await.unwrap();
        let reload = f.load(0).await.unwrap().unwrap(); let current = reload.inspect_runtime().unwrap();
        f.manager.unload(&id).await.unwrap();
        assert_eq!(current.historical_canonical_history().unwrap(), history);
        assert_eq!(events(&current).iter().filter(|e| matches!(e, RuntimeEvent::ToolExecutionStarted { .. })).count(), 1);
        assert!(f.provider.request_bodies().is_empty());
        assert!(!f.workspaces[0].join("must-not-exist").exists());
        f.close().await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_delete_rejects_load_before_physical_cleanup_finishes() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        use crate::runtime::conversation_runtime::Gate;
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        let SessionDeleteResult::Preview { preview } =
            f.manager.sessions.delete_preview(&f.sessions[0].id).await
        else {
            panic!("unloaded")
        };
        let gate = Arc::new(Gate::default());
        let release = gate.arm_scoped();
        f.manager.sessions.install_delete_cleanup_gate(gate.clone());
        let sessions = f.manager.sessions.clone();
        let session = f.sessions[0].id.clone();
        let deletion = tokio::spawn(async move {
            sessions
                .delete_session(&session, &preview.target_revision)
                .await
        });
        gate_entered(&gate).await; // Durable delete committed; filesystem cleanup parked.
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 0);
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        let b = f.load(1).await.unwrap().unwrap();
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        drop(release);
        assert!(matches!(
            deletion.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unload_joining_replacement_drains_the_new_incarnation_before_success() {
    bounded(async {
        let f = Fixture::new().await;
        let a = f.load(0).await.unwrap().unwrap();
        let old = a.inspect_runtime().unwrap();
        let probe = f.manager.probe(a.conversation_id());
        probe.before_shutdown.arm();
        let replacement = replace_task(&f, 0);
        probe.before_shutdown.entered().await;
        let unload = unload_task(&f, a.conversation_id().clone());
        probe
            .unloads_joined
            .subscribe()
            .wait_for(|n| *n == 1)
            .await
            .unwrap();
        assert!(!unload.is_finished());
        probe.before_shutdown.release();
        let new = replacement.await.unwrap().unwrap();
        unload.await.unwrap().unwrap();
        assert_ne!(a.incarnation_id(), new.incarnation_id());
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Unloaded
        );
        assert!(new.inspect_runtime().is_none());
        assert!(old.submit_inbound(input("stale")).is_err());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_activation_never_holds_the_global_registry_lock() {
    bounded(async {
        use crate::runtime::conversation_runtime::Gate;
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        let gate = Arc::new(Gate::default());
        let release = gate.arm_scoped();
        *probe.activation.lock().unwrap() = Some(gate.clone());
        let a = f.load(0);
        gate_entered(&gate).await;
        assert_eq!(f.manager.residency(&id), ResidencyState::Loading);
        let another_a = f.load(0);
        probe.joined(2).await;
        assert!(!another_a.is_finished());
        let b = f.load(1).await.unwrap().unwrap();
        let live_b = b.inspect_runtime().unwrap();
        live_b.submit_inbound(input("request-B")).unwrap();
        f.gates[1].wait_entered().await;
        let done = live_b.settlement_signal().notified();
        f.gates[1].release();
        done.await;
        f.manager.unload(b.conversation_id()).await.unwrap();
        assert!(!a.is_finished());
        drop(release);
        let a = a.await.unwrap().unwrap();
        let another_a = another_a.await.unwrap().unwrap();
        assert!(Arc::ptr_eq(&a, &another_a));
        f.close().await;
    })
    .await;
}

// Uses the durable graph owner to create two valid lineages before residency.
async fn second_node(f: &Fixture) -> SessionSnapshot {
    use crate::durable::ConversationStore;
    use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock, UserSource};
    let a = &f.sessions[0];
    let access = f
        .manager
        .sessions
        .acquire_session(&a.id, None)
        .await
        .unwrap();
    let store = crate::durable::SqliteConversationStore::open(
        a.active_conversation_id.clone(),
        &access.database_path,
    )
    .unwrap();
    let boundary = crate::runtime::identity::MessageId::new("branch-boundary");
    store
        .append_canonical(&MessageBlock::User(UserMessageBlock {
            id: boundary.clone(),
            content: input("branch seed"),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }))
        .unwrap();
    let revision = store.load_head().unwrap().revision;
    let branch = f
        .manager
        .sessions
        .branch_session_node(&a.id, &a.active_node, revision, &boundary)
        .await
        .unwrap()
        .session;
    f.manager
        .sessions
        .set_current_node(&a.id, &a.active_node)
        .await
        .unwrap();
    branch
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_claim_excludes_other_nodes_through_loading_loaded_and_unloading() {
    bounded(async {
        let f = Fixture::new().await;
        let branch = second_node(&f).await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        let other = f.manager.probe(&branch.active_conversation_id);
        probe.before_compose.arm();
        let loading = f.load(0);
        probe.before_compose.entered().await;
        let expected = RuntimeManagerError::SessionAlreadyResident {
            session_id: branch.id.clone(),
            resident_conversation: id.clone(),
            requested_conversation: branch.active_conversation_id.clone(),
        };
        assert_eq!(
            f.manager
                .load(&branch.id, Some(&branch.active_node))
                .await
                .unwrap_err(),
            expected
        );
        probe.before_compose.release();
        let a = loading.await.unwrap().unwrap();
        let b = f.load(1).await.unwrap().unwrap();
        let live_b = b.inspect_runtime().unwrap();
        b.client().submit_inbound(input("request-B")).unwrap();
        f.gates[1].wait_entered().await;
        assert_eq!(
            f.manager
                .load(&branch.id, Some(&branch.active_node))
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(
            f.manager
                .replace(&branch.id, Some(&branch.active_node))
                .await
                .unwrap_err(),
            expected
        );
        assert!(f.manager.is_current(&id, a.incarnation_id()));
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        assert!(live_b.has_current_attempt());
        probe.before_shutdown.arm();
        let manager = f.manager.clone();
        let unloading_id = id.clone();
        let unloading = tokio::spawn(async move { manager.unload(&unloading_id).await });
        probe.before_shutdown.entered().await;
        assert_eq!(
            f.manager
                .load(&branch.id, Some(&branch.active_node))
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(other.compositions.load(Ordering::SeqCst), 0);
        probe.before_shutdown.release();
        unloading.await.unwrap().unwrap();
        let second = f
            .manager
            .load(&branch.id, Some(&branch.active_node))
            .await
            .unwrap();
        assert_eq!(other.compositions.load(Ordering::SeqCst), 1);
        assert!(
            f.manager
                .is_current(second.conversation_id(), second.incarnation_id())
        );
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        let done = live_b.settlement_signal().notified();
        f.gates[1].release();
        done.await;
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        f.manager.unload(second.conversation_id()).await.unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_client_and_identity_cannot_keep_unloaded_allocation_alive() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let identity = f.load(0).await.unwrap().unwrap();
        let client = identity.client();
        client.snapshot().unwrap();
        let weak = identity.inspect_runtime().unwrap().weak_inner();
        f.manager.unload(identity.conversation_id()).await.unwrap();
        assert_eq!(
            f.manager.residency(identity.conversation_id()),
            ResidencyState::Unloaded
        );
        assert!(weak.upgrade().is_none());
        let SessionDeleteResult::Preview { preview } =
            f.manager.sessions.delete_preview(&f.sessions[0].id).await
        else {
            panic!("stale handles must not retain allocation")
        };
        assert!(matches!(
            f.manager
                .sessions
                .delete_session(&f.sessions[0].id, &preview.target_revision)
                .await
                .unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        // Both the production client and identity remain alive across real deletion.
        assert_eq!(
            client.submit_inbound(input("stale")).unwrap_err(),
            RuntimeManagerError::StaleIncarnation
        );
        assert_eq!(
            identity.client().snapshot().unwrap_err(),
            RuntimeManagerError::StaleIncarnation
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_attachment_subscription_and_endpoint_do_not_own_residency() {
    bounded(async {
        use crate::runtime_client::host::EventDelivery;
        use crate::runtime_client::{RequestId, RuntimeClientError, RuntimeClientRequest};
        let f = Fixture::new().await;
        let identity = f.load(0).await.unwrap().unwrap();
        let client = identity.client();
        let attachment = client.attach().unwrap().0.attachment;
        let subscription = attachment.subscription().unwrap();
        let (endpoint, weak_host) = {
            let resident = identity.resident.upgrade().unwrap();
            let composition = resident.composition.lock().unwrap();
            let host = composition.as_ref().unwrap().host();
            (host.endpoint(), host.weak_inner())
        };
        let weak_runtime = identity.inspect_runtime().unwrap().weak_inner();
        f.manager.unload(identity.conversation_id()).await.unwrap();
        assert!(
            weak_host.upgrade().is_none(),
            "passive handles retain no host"
        );
        assert!(
            weak_runtime.upgrade().is_none(),
            "runtime resources released"
        );
        assert!(
            matches!(
                f.manager.sessions.delete_preview(&f.sessions[0].id).await,
                crate::local_runtime::session::deletion::SessionDeleteResult::Preview { .. }
            ),
            "native destructive preflight proves allocation access was released"
        );
        assert_eq!(subscription.next().await, EventDelivery::Closed);
        assert_eq!(
            attachment
                .handle_request(RuntimeClientRequest::SnapshotGet {
                    id: RequestId::new(1),
                })
                .error,
            Some(RuntimeClientError::NotAttached)
        );
        let replacement = f.load(0).await.unwrap().unwrap();
        assert_ne!(identity.incarnation_id(), replacement.incarnation_id());
        assert_eq!(
            client.validate(),
            Err(RuntimeManagerError::StaleIncarnation)
        );
        assert_eq!(subscription.try_next(), EventDelivery::Closed);
        assert_eq!(
            endpoint
                .handle_request(RuntimeClientRequest::Initialize {
                    id: RequestId::new(2),
                    protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
                })
                .error,
            Some(RuntimeClientError::NotAttached)
        );
        let _new_attachment = replacement.client().attach().unwrap().0.attachment;
        f.manager
            .unload(replacement.conversation_id())
            .await
            .unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacement_revokes_old_client_and_releases_old_composition() {
    bounded(async {
        let f = Fixture::new().await;
        let old = f.load(0).await.unwrap().unwrap();
        let client = old.client();
        let weak = old.inspect_runtime().unwrap().weak_inner();
        let new = f.manager.replace(&f.sessions[0].id, None).await.unwrap();
        assert_eq!(old.conversation_id(), new.conversation_id());
        assert_ne!(old.incarnation_id(), new.incarnation_id());
        assert!(weak.upgrade().is_none());
        assert_eq!(
            client.submit_inbound(input("stale")).unwrap_err(),
            RuntimeManagerError::StaleIncarnation
        );
        assert_eq!(
            client.snapshot().unwrap_err(),
            RuntimeManagerError::StaleIncarnation
        );
        new.client().snapshot().unwrap();
        assert!(
            f.manager
                .is_current(new.conversation_id(), new.incarnation_id())
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_claim_survives_replacement_handoff_and_clears_on_failed_loading() {
    bounded(async {
        let f = Fixture::new().await;
        let branch = second_node(&f).await;
        let a = f.load(0).await.unwrap().unwrap();
        let probe = f.manager.probe(a.conversation_id());
        probe.before_compose.arm();
        probe.panic_once.store(true, Ordering::SeqCst);
        let manager = f.manager.clone();
        let session = f.sessions[0].id.clone();
        let replacement = tokio::spawn(async move { manager.replace(&session, None).await });
        probe.before_compose.entered().await;
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loading
        );
        assert!(a.inspect_runtime().is_none());
        assert!(matches!(
            f.manager.load(&branch.id, Some(&branch.active_node)).await,
            Err(RuntimeManagerError::SessionAlreadyResident { .. })
        ));
        probe.before_compose.release();
        assert!(replacement.await.unwrap().is_err());
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Unloaded
        );
        let second = f
            .manager
            .load(&branch.id, Some(&branch.active_node))
            .await
            .unwrap();
        f.manager.unload(second.conversation_id()).await.unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_fence_before_load_prevents_allocation_acquisition() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(&id);
        probe.after_delete_fence.arm();
        let delete = delete_task(&f, 0, revision);
        probe.after_delete_fence.entered().await;
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 0);
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 0);
        assert!(f.load(1).await.unwrap().is_ok());
        probe.after_delete_fence.release();
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn load_claim_is_visible_to_delete_before_allocation_acquisition() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(&id);
        probe.before_allocation.arm();
        let load = f.load(0);
        probe.before_allocation.entered().await;
        assert_eq!(f.manager.residency(&id), ResidencyState::Loading);
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 0);
        let joined = f.load(0);
        probe.joined(2).await;
        let delete = delete_task(&f, 0, revision);
        // This watch fires only when delete sees and joins the registered flight.
        probe
            .unloads_joined
            .subscribe()
            .wait_for(|n| *n == 1)
            .await
            .unwrap();
        assert!(!delete.is_finished());
        assert!(
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(&f.sessions[0].id)
        );
        assert!(
            f.load(1)
                .await
                .unwrap()
                .unwrap()
                .client()
                .validate()
                .is_ok()
        );
        probe.before_allocation.release();
        assert!(load.await.unwrap().is_err());
        assert!(joined.await.unwrap().is_err());
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 1);
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        assert!(f.provider.request_bodies().is_empty());
        assert!(
            f.manager
                .sessions
                .read_session(&f.sessions[0].id)
                .await
                .is_err()
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_joining_failed_loading_receives_proven_writer_absence() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(&id);
        probe.before_compose.arm();
        probe.fail_compose_once.store(true, Ordering::SeqCst);
        let load = f.load(0);
        probe.before_compose.entered().await;
        let flight = {
            let registry = f.manager.registry.0.lock().unwrap();
            let Some(Entry::Loading(flight)) = registry.entries.get(&id) else {
                panic!("Loading")
            };
            flight.clone()
        };
        let delete = delete_task(&f, 0, revision);
        probe
            .unloads_joined
            .subscribe()
            .wait_for(|n| *n == 1)
            .await
            .unwrap();
        assert!(!delete.is_finished());
        probe.before_compose.release();
        assert!(load.await.unwrap().is_err());
        assert!(matches!(flight.wait().await, Outcome::WriterAbsent(Err(_))));
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        assert!(
            !f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(&f.sessions[0].id)
        );
        assert!(
            f.manager
                .sessions
                .read_session(&f.sessions[0].id)
                .await
                .is_err()
        );
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 1);
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_joins_replacement_failure_after_proven_writer_transfer() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let old = f.load(0).await.unwrap().unwrap();
        let id = old.conversation_id();
        let weak = old.inspect_runtime().unwrap().weak_inner();
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(id);
        probe.after_writer_transfer.arm();
        probe.fail_compose_once.store(true, Ordering::SeqCst);
        let replace = replace_task(&f, 0);
        probe.after_writer_transfer.entered().await;
        assert!(
            weak.upgrade().is_none(),
            "old native writer released before replacement acquisition"
        );
        assert!(old.inspect_runtime().is_none());
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 1);
        let flight = {
            let registry = f.manager.registry.0.lock().unwrap();
            let Some(Entry::Loading(flight)) = registry.entries.get(id) else {
                panic!("replacement Loading")
            };
            flight.clone()
        };
        let delete = delete_task(&f, 0, revision);
        probe
            .unloads_joined
            .subscribe()
            .wait_for(|n| *n == 1)
            .await
            .unwrap();
        assert!(!delete.is_finished());
        probe.after_writer_transfer.release();
        assert!(replace.await.unwrap().is_err());
        assert!(matches!(flight.wait().await, Outcome::WriterAbsent(Err(_))));
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert_eq!(f.manager.residency(id), ResidencyState::Unloaded);
        assert!(
            !f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(&f.sessions[0].id)
        );
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 2);
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 2);
        assert!(f.provider.request_bodies().is_empty());
        assert!(
            f.manager
                .sessions
                .read_session(&f.sessions[0].id)
                .await
                .is_err()
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovery_observes_live_session_without_releasing_active_delete_fence() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let session = &f.sessions[0].id;
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(&id);
        probe.after_delete_fence.arm();
        let delete = delete_task(&f, 0, revision.clone());
        probe.after_delete_fence.entered().await;
        let SessionDeleteResult::Preview { preview } =
            f.manager.recover_session_deletion(session).await.unwrap()
        else {
            panic!("still-live Session must return Preview")
        };
        assert_eq!(preview.target_revision, revision);
        assert!(
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(session)
        );
        assert!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .pending_deletion_ids()
                .is_empty()
        );
        assert!(!delete.is_finished());
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 0);
        probe.after_delete_fence.release();
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovery_settles_durability_uncertainty_without_runtime_reconstruction() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let old = f.load(0).await.unwrap().unwrap();
        let session = &f.sessions[0].id;
        let probe = f.manager.probe(old.conversation_id());
        let revision = deletion_revision(&f, 0).await;
        f.manager
            .sessions
            .catalog
            .lock()
            .await
            .arm_write_fault_after_rename();
        assert!(matches!(
            f.manager.delete_session(session, &revision).await.unwrap(),
            SessionDeleteResult::CommittedDurabilityUncertain { .. }
        ));
        assert!(old.inspect_runtime().is_none());
        assert!(
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(session)
        );
        assert!(f.load(0).await.unwrap().is_err());
        assert!(
            f.load(1)
                .await
                .unwrap()
                .unwrap()
                .client()
                .validate()
                .is_ok()
        );
        assert!(matches!(
            f.manager.recover_session_deletion(session).await.unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert!(
            !f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(session)
        );
        assert!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .pending_deletion_ids()
                .is_empty()
        );
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        // Repeated reconciliation confirms absence; it never recreates authority.
        assert!(matches!(
            f.manager.recover_session_deletion(session).await.unwrap(),
            SessionDeleteResult::NotFound { .. }
        ));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovery_with_unproven_catalog_durability_retains_delete_fence() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let session = &f.sessions[0].id;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        let revision = deletion_revision(&f, 0).await;
        f.manager
            .sessions
            .catalog
            .lock()
            .await
            .arm_write_fault_after_rename();
        assert!(matches!(
            f.manager.delete_session(session, &revision).await.unwrap(),
            SessionDeleteResult::CommittedDurabilityUncertain { .. }
        ));
        f.manager
            .sessions
            .catalog
            .lock()
            .await
            .arm_write_fault_before_rename();
        assert!(matches!(
            f.manager.recover_session_deletion(session).await.unwrap(),
            SessionDeleteResult::CommittedDurabilityUncertain { .. }
        ));
        assert!(
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(session)
        );
        assert_eq!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .pending_deletion_ids(),
            vec![session.clone()]
        );
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 0);
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 0);
        f.close().await;
    })
    .await;
}
