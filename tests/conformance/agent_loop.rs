//! Issue #47: Agent Loop conformance through the real provider boundary.
//!
//! Every test here composes the **real** local runtime — the real model
//! catalog, the resolved model binding, the production provider adapter, the
//! real HTTP client and streaming parser, the Agent Loop, the context
//! engine, the tool runtime, the capability plane, and the Runtime Client
//! projection — and points its catalog at an external scripted provider
//! process.
//!
//! ```text
//! this test driver
//!   |  fixed user/runtime actions
//!   v
//! SessionRuntimeManager + AppServerConnection  (real composition)
//!   |
//!   v
//! real provider adapter -> real HTTP + SSE -> fake-provider (Python)
//! ```
//!
//! Nothing in rustX is substituted. The only scripted participant is the
//! external provider, and its scenario asserts the requests rustX produced
//! as strictly as this file asserts the runtime state rustX reached.
//!
//! The string constants below mirror `test-support/fake-provider/src/
//! fake_provider/scenarios/conformance.py`. A drift in either direction
//! fails: the scenario rejects a request it did not expect, and this driver
//! rejects a runtime state it did not expect.

use crate::launch_fixture::LaunchFixture;
use rustx::durable::{CanonicalStore, JournalStore, ObservationStore, ProjectionStore};
use std::path::PathBuf;
use std::sync::Arc;

use crate::common::provider_emulator::ProviderEmulator;
use rustx::app_server::connection::AppServerConnection;
use rustx::app_server::host::AppServerHost;
use rustx::app_server::protocol::{
    APP_SERVER_PROTOCOL_VERSION, AttachmentTarget, ClientIdentity, InitializeParams,
    JsonRpcVersion, Method, MethodResult, NotificationMethod, PresentationCapabilities, Request,
    RequestId, Response, RpcError,
};
use rustx::credentials::CredentialSnapshot;
use rustx::durable::{ConversationStore, SqliteConversationStore};
use rustx::local_runtime::HostEnvironment;
use rustx::local_runtime::app_server_policy::AppServerPolicy;
use rustx::local_runtime::composition::{HeadlessConversationRuntime, LocalRuntimeDependencies};
use rustx::local_runtime::session::SessionPersistentState;
use rustx::local_runtime::session_controller::{SessionAccess, SessionController};
use rustx::local_runtime::session_runtime_manager::{
    ManagedRuntimeClient, RuntimeManagerError, RuntimeResidencyPolicy, SessionRuntimeManager,
};
use rustx::message::content::TextBlock;
use rustx::message::types::{MessageBlock, UserContentBlock};
use rustx::model::catalog::{MapCredentialEnvironment, ModelRef};
use rustx::model::session::{AuxiliaryModelPolicy, SessionModelConfig};
use rustx::observation::failure::RuntimeClientAttemptFailure;
use rustx::runtime::recovery::{AttemptRecoveryClass, ResumeDisposition};
use rustx::runtime_client::snapshot::RuntimeClientAttemptPhase;
use rustx::runtime_client::{RuntimeClientEvent, RuntimeClientOutcome};

// ---------------------------------------------------------------------------
// The scenario contract, mirrored from the Python scenarios
// ---------------------------------------------------------------------------

const CREDENTIAL_VARIABLE: &str = "RUSTX_CONFORMANCE_KEY";
const CREDENTIAL_VALUE: &str = "conformance-secret";

const CHAT_MODEL: &str = "chat-model";
const RESPONSES_MODEL: &str = "responses-model";
const ANTHROPIC_MODEL: &str = "anthropic-model";
const SUMMARY_MODEL: &str = "summary-model";
const SECOND_MODEL: &str = "second-model";

const TURN_ONE: &str = "conformance: turn one";
const TURN_TWO: &str = "conformance: turn two";
const READ_PROMPT: &str = "conformance: read the note";
const SKILL_PROMPT: &str = "conformance: follow the conformance skill";
const FIRST_ATTEMPT: &str = "conformance: first attempt";
const SECOND_ATTEMPT: &str = "conformance: second attempt";

const NOTE_PATH: &str = "note.txt";
const NOTE_MARKER: &str = "deterministic-note-payload-6d41";
const SKILL_NAME: &str = "conformance-skill";
const SKILL_DESCRIPTION: &str =
    "The deterministic workspace Skill of the issue 47 conformance harness.";
const SKILL_BODY_MARKER: &str = "skill-body-marker-a17c";
const COMPACTION_MARKER: &str = "compaction-filler-marker-93be";
const SUMMARY_TEXT: &str = "conformance summary: the assistant produced one long report.";

const TURN_THREE: &str = "conformance: turn three";
const FILLER_ONE_MARKER: &str = "compaction-filler-one-marker-51f0";
const FILLER_TWO_MARKER: &str = "compaction-filler-two-marker-b27d";
const SUMMARY_ONE_TEXT: &str = "conformance summary one: the assistant produced filler report one.";
const SUMMARY_TWO_TEXT: &str = "conformance summary two: the assistant produced filler report two.";

/// The compaction window and reserve.
///
/// The emulator's scripted turn-one reply is ~200 KB, which the frozen
/// `ceil(bytes / 4)` estimator values at roughly 51k tokens. Two bounds have
/// to hold at once, and both are structural, not statistical:
///
/// ```text
/// trigger:  window - reserve - output   <=  the turn-two request estimate
/// summary:  the selected span estimate  <=  window - reserve - summary output
/// ```
///
/// Both budgets carry the reserve, so both bounds are the same number here
/// (the primary and summary output budgets are both 1k), and that number has
/// to land between the selected span's estimate (~51k) and the whole
/// turn-two request estimate (~56k). The difference between the two is the
/// non-retirable request input — the Effective System Prompt, the tool
/// definitions, and the turn-two user message — so a 56k window with a 1.5k
/// reserve puts the shared limit at ~53.4k, roughly centred in that band
/// with ~2.4k of margin on each side. Compaction never splits a message to
/// get there.
const COMPACTION_CONTEXT_WINDOW: u64 = 56_000;
const COMPACTION_RESERVE_TOKENS: u64 = 1_536;

// ---------------------------------------------------------------------------
// The composed driver
// ---------------------------------------------------------------------------

/// One composed runtime plus the attachment and subscription a driver needs.
struct Driver {
    /// The temporary root, kept alive for the fixture lifetime.
    #[allow(dead_code)]
    root: tempfile::TempDir,
    manager: SessionRuntimeManager,
    client: ManagedRuntimeClient,
    connection: Arc<AppServerConnection>,
    target: AttachmentTarget,
    access: SessionAccess,
    store: Arc<SqliteConversationStore>,
    events: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<NotificationMethod>>,
    submitted_after: std::sync::atomic::AtomicU64,
    consumed_cursor: std::sync::atomic::AtomicU64,
    relay: tokio::task::JoinHandle<()>,
}

/// Retain complete wire records; semantic assertions borrow their original facts.
#[derive(Debug, Default)]
struct ObservedEvents {
    records: Vec<NotificationMethod>,
}
impl ObservedEvents {
    fn iter(&self) -> impl DoubleEndedIterator<Item = &RuntimeClientEvent> {
        self.records.iter().flat_map(|record| match record {
            NotificationMethod::Event { event, .. } => event.facts(),
            _ => &[],
        })
    }
    fn last(&self) -> Option<&RuntimeClientEvent> {
        self.iter().next_back()
    }
}

/// How a driver's catalog and session are shaped.
struct Setup {
    /// The model the session selects.
    model: String,
    instructions: Option<String>,
    images: bool,
    /// The declared context window of every catalog model.
    context_window: u64,
    /// The session's safety reserve.
    reserve_tokens: u64,
    /// Tokens of recent history kept literal.
    keep_recent_tokens: u64,
    /// The explicit summary-model policy, when the session declares one.
    summary_model: Option<String>,
    /// Explicit Skill resources for this isolated provider fixture.
    skill_paths: Vec<PathBuf>,
}

impl Setup {
    fn new(model: &str) -> Self {
        Self {
            model: model.to_owned(),
            instructions: None,
            images: false,
            context_window: 128_000,
            reserve_tokens: 1_024,
            keep_recent_tokens: 8_192,
            summary_model: None,
            skill_paths: Vec::new(),
        }
    }
}

impl Driver {
    /// Composes the real runtime against a running emulator.
    async fn start(emulator: &ProviderEmulator, setup: &Setup) -> Self {
        Self::start_in(tempfile::tempdir().expect("temp root"), emulator, setup).await
    }

    /// Composes the real runtime over a root a test has already populated
    /// (a Skill package must exist before the capability candidate is
    /// prepared, which happens inside composition).
    async fn start_in(root: tempfile::TempDir, emulator: &ProviderEmulator, setup: &Setup) -> Self {
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        if setup.images {
            std::fs::create_dir_all(workspace.join(".agents/agents")).unwrap();
            std::fs::create_dir_all(workspace.join(".agents/workflows")).unwrap();
            std::fs::write(
                workspace.join(".agents/agents/image_reviewer.toml"),
                "description = \"Text-only image history reviewer.\"\ninstructions = \"Review the delegated text.\"\n[model]\nmodel = \"emulator/chat-model\"\n[tools]\nbuiltin = []\n",
            ).unwrap();
            let workflow = serde_json::json!({
                "description": "Delegate a text review to an independently frozen child.",
                "block": {
                    "input": {"type":"object", "properties":{"task":{"type":"string"}}, "required":["task"], "additionalProperties":false},
                    "nodes": {"workflow_start":{"type":"start"},
                        "review": {"type":"agent", "profile":"image_reviewer", "task":[{"type":"literal","value":"Review the delegated text.\n\n"},{"type":"reference","path":["args","task"]}],
                            "output":{"type":"json_schema","schema":{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"],"additionalProperties":false}}},
                        "done": {"type":"return","outputs":[{"variable":"summary","value_selector":["review","summary"]}]}
                    },"edges": [{"from":"workflow_start","to":"review"},{"from":"review","to":"done"}],}
            });
            std::fs::write(
                workspace.join(".agents/workflows/image_review.yaml"),
                serde_yaml::to_string(&workflow).unwrap(),
            )
            .unwrap();
        }
        std::fs::write(
            root.path().join("rustx.toml"),
            format!("{}\n{}", session_json(setup), models_json(emulator, setup)),
        )
        .expect("rustx.toml");

        let paths = LaunchFixture {
            config: root.path().join("rustx.toml"),
            startup_session: rustx::local_runtime::StartupSession::Empty,
            workspace,
            runtime_root: root.path().join("private"),
        };
        let dependencies = LocalRuntimeDependencies {
            credentials: Some(Arc::new(MapCredentialEnvironment::new([(
                CREDENTIAL_VARIABLE.to_owned(),
                CREDENTIAL_VALUE.to_owned(),
            )]))),
            child_program: Some(PathBuf::from(env!("CARGO_BIN_EXE_rustx"))),
            ..LocalRuntimeDependencies::default()
        };
        let home = root.path().join(".test-host");
        std::fs::create_dir_all(&home).expect("isolated host");
        let environment =
            HostEnvironment::from_paths(paths.workspace.clone(), home).expect("host environment");
        let (configuration, input) = paths
            .request()
            .session_input(&environment)
            .expect("fixture Session input");
        let sessions = SessionController::open(&paths.runtime_root).expect("Session controller");
        let session = sessions
            .create_session(SessionPersistentState::from_input(&input))
            .await
            .expect("durable Session")
            .session;
        // These strict scripts exercise primary/Summary execution. Their saved
        // fixture label suppresses independent automatic Title inference.
        sessions
            .rename_session(&session.id, "Provider conformance fixture")
            .await
            .expect("fixture manual name");
        let manager = SessionRuntimeManager::new(
            sessions.clone(),
            configuration,
            CredentialSnapshot::new([(
                CREDENTIAL_VARIABLE.to_owned(),
                CREDENTIAL_VALUE.to_owned(),
            )]),
            dependencies,
            RuntimeResidencyPolicy {
                max_resident_runtimes: 8,
                idle_grace_ms: 300_000,
            },
        )
        .expect("Session runtime owner");
        let connection = Arc::new(AppServerConnection::new(AppServerHost::new(
            manager.clone(),
            AppServerPolicy::default(),
        )));
        rpc(
            &connection,
            Method::Initialize(InitializeParams {
                protocol_version: APP_SERVER_PROTOCOL_VERSION,
                client: ClientIdentity {
                    name: "conformance".into(),
                    version: "1".into(),
                },
                presentation: PresentationCapabilities::default(),
            }),
        )
        .await
        .expect("initialize App Server");
        let attached = rpc(
            &connection,
            Method::SessionAttach {
                session_id: session.id.clone(),
                node_id: None,
            },
        )
        .await
        .expect("attach durable Session");
        let MethodResult::Attached { target, .. } = attached else {
            panic!("attach returns its bounded identity receipt");
        };
        let checkpoint = rpc(
            &connection,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: Vec::new(),
            },
        )
        .await
        .expect("bounded native checkpoint");
        let MethodResult::Checkpoint { checkpoint } = checkpoint else {
            panic!("checkpoint response");
        };
        let cursor = checkpoint.cursor;
        rpc(
            &connection,
            Method::SessionSubscribe {
                target: target.clone(),
                after_cursor: cursor,
            },
        )
        .await
        .expect("subscribe");
        let runtime = manager
            .load(&session.id, None)
            .await
            .expect("resident runtime");
        let client = runtime.client();
        let access = sessions
            .acquire_session(&session.id, None)
            .await
            .expect("read access");
        let store = Arc::new(
            SqliteConversationStore::open_existing(
                access.node.conversation_id.clone(),
                &access.database_path,
            )
            .expect("durable read handle"),
        );
        let (send, events) = tokio::sync::mpsc::unbounded_channel();
        let notifications = connection.clone();
        let relay = tokio::spawn(async move {
            loop {
                match notifications.next_notification().await.notification {
                    record @ NotificationMethod::Event { .. } => {
                        if send.send(record).is_err() {
                            return;
                        }
                    }
                    NotificationMethod::Closed { .. }
                    | NotificationMethod::ResyncRequired { .. } => break,
                    _ => {}
                }
            }
        });
        Self {
            root,
            manager,
            client,
            connection,
            target,
            access,
            store,
            events: tokio::sync::Mutex::new(events),
            submitted_after: std::sync::atomic::AtomicU64::new(0),
            consumed_cursor: std::sync::atomic::AtomicU64::new(cursor.get()),
            relay,
        }
    }

    fn snapshot(
        &self,
    ) -> Result<
        (
            rustx::runtime_client::snapshot::RuntimeClientSnapshot,
            rustx::runtime_client::RuntimeClientCursor,
        ),
        RuntimeManagerError,
    > {
        self.client
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
    }

    /// Explicit original-history oracle, independent of the checkpoint's body cache.
    /// Every selected immutable body belongs to one captured native Ledger prefix.
    fn canonical_history(&self) -> Vec<MessageBlock> {
        let cut = self
            .store
            .observation_read_cut()
            .expect("native history cut");
        let mut offset = 0;
        let mut messages = Vec::new();
        loop {
            let page = self
                .store
                .message_header_page(&cut, Some(offset), 32)
                .expect("bounded canonical history page");
            assert_eq!(page.cut, cut);
            assert_eq!(page.offset, offset);
            assert_eq!(page.total, cut.ledger_through);
            assert!(page.headers.len() <= 32);
            let ids: Vec<_> = page
                .headers
                .iter()
                .enumerate()
                .map(|(index, header)| {
                    assert_eq!(
                        header.ledger_position,
                        Some(offset + u64::try_from(index).expect("bounded header ordinal") + 1)
                    );
                    header
                        .identity
                        .clone()
                        .expect("small original fixture identity")
                })
                .collect();
            let originals = self
                .store
                .load_messages(&ids)
                .expect("selected original bodies");
            assert_eq!(originals.len(), ids.len());
            for (original, identity) in originals.iter().zip(&ids) {
                assert_eq!(original.id(), identity);
            }
            messages.extend(originals);
            offset += u64::try_from(page.headers.len()).expect("bounded page count");
            assert_eq!(page.next_offset, (offset < page.total).then_some(offset));
            if page.next_offset.is_none() {
                assert_eq!(u64::try_from(messages.len()).unwrap(), cut.ledger_through);
                return messages;
            }
            assert!(!page.headers.is_empty(), "native continuation advances");
        }
    }

    fn request_history(&self) -> rustx::runtime::request_history::RequestHistory {
        rustx::runtime::request_history::RequestHistory::new(self.store.clone())
    }

    fn reconstruct_request(
        &self,
        identity: &rustx::model::RequestIdentity,
    ) -> Result<rustx::model::ModelRequest, rustx::runtime::request_history::RequestHistoryError>
    {
        self.request_history().reconstruct(identity)
    }

    fn artifacts(&self) -> rustx::tools::artifacts::ArtifactStore {
        rustx::tools::artifacts::ArtifactStore::new(
            self.access.node.conversation_id.clone(),
            self.access.database_path.parent().unwrap(),
        )
        .expect("artifact read handle")
    }

    async fn model_set(&self, config: SessionModelConfig) -> Result<MethodResult, RpcError> {
        rpc(
            &self.connection,
            Method::ModelSet {
                target: self.target.clone(),
                config: Box::new(config),
            },
        )
        .await
    }

    async fn cancel_current_attempt(&self) -> Result<MethodResult, RpcError> {
        rpc(
            &self.connection,
            Method::TurnCancel {
                target: self.target.clone(),
            },
        )
        .await
    }

    async fn compact_context(&self) -> Result<MethodResult, RpcError> {
        rpc(
            &self.connection,
            Method::CompactContext {
                target: self.target.clone(),
                request_id: "conformance-compact".to_owned().try_into().unwrap(),
            },
        )
        .await
    }

    async fn shutdown(&self) -> Result<(), RuntimeManagerError> {
        self.connection.close();
        self.relay.abort();
        self.manager.unload(&self.target.conversation_id).await
    }

    /// Submits one fixed user turn.
    fn submit(&self, text: &str) {
        self.submitted_after.store(
            self.store
                .presentation_frontier()
                .expect("original pre-submit J cut"),
            std::sync::atomic::Ordering::SeqCst,
        );
        self.client
            .submit_inbound(vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })])
            .expect("inbound accepted");
    }

    /// Consumes the observation stream up to and including the attempt's one
    /// terminal settlement.
    ///
    /// The settlement uniqueness invariant is asserted here rather than
    /// assumed: a second `AttemptSettled` fails, and the terminal event must
    /// be the last event of the attempt.
    fn submitted_attempt(&self) -> Option<(u64, rustx::runtime::identity::AttemptId)> {
        use rustx::durable::presentation::{FactQuery, FactScope};
        let through = self
            .store
            .presentation_frontier()
            .expect("original attempt Journal frontier");
        // The original start is immutable J evidence. Pending adoption may
        // expire an independent transcript cut while this attempt executes.
        let starts = self
            .store
            .read_presentation_events(&FactQuery {
                scope: FactScope::All,
                kinds: vec!["attempt_started"],
                before: None,
                after: self
                    .submitted_after
                    .load(std::sync::atomic::Ordering::SeqCst),
                ascending: true,
                through,
                limit: 1,
            })
            .expect("bounded original Attempt start");
        starts.into_iter().next().map(|start| {
            let rustx::events::types::RuntimeEvent::AttemptStarted { attempt_id } = start.event
            else {
                panic!("original indexed AttemptStarted kind")
            };
            assert_eq!(start.attempt_id.as_ref(), Some(&attempt_id));
            assert_eq!(start.conversation_id, self.target.conversation_id);
            assert!(start.sequence <= through);
            (start.sequence, attempt_id)
        })
    }

    fn assert_original_terminal(&self, start: u64, owner: &rustx::runtime::identity::AttemptId) {
        use rustx::durable::presentation::{FactQuery, FactScope};
        use rustx::events::types::RuntimeEvent as E;
        let through = self
            .store
            .presentation_frontier()
            .expect("native terminal Journal frontier");
        let query = FactQuery {
            scope: FactScope::Attempt(owner.clone()),
            kinds: vec![
                "attempt_completed",
                "attempt_failed",
                "attempt_cancelled",
                "attempt_timed_out",
                "attempt_limit_exceeded",
            ],
            before: None,
            after: start,
            ascending: false,
            through,
            limit: 2,
        };
        let terminal = self
            .store
            .read_presentation_events(&query)
            .expect("original terminal uniqueness");
        assert_eq!(
            terminal.len(),
            1,
            "one native terminal, regardless of checkpoint repaint"
        );
        let (E::AttemptCompleted { attempt_id, .. }
        | E::AttemptFailed { attempt_id, .. }
        | E::AttemptCancelled { attempt_id, .. }
        | E::AttemptTimedOut { attempt_id }
        | E::AttemptLimitExceeded { attempt_id, .. }) = &terminal[0].event
        else {
            panic!("original indexed terminal kind")
        };
        assert_eq!(attempt_id, owner);
        assert_eq!(terminal[0].attempt_id.as_ref(), Some(owner));
        assert_eq!(terminal[0].conversation_id, self.target.conversation_id);
        assert!(start < terminal[0].sequence && terminal[0].sequence <= through);
        // This independent oracle reads every original J kind through the
        // same captured immutable frontier, including nonterminal owner facts.
        let mut after = start;
        let mut last_owned = None;
        let mut original_terminals = Vec::new();
        while after < through {
            let page = self
                .store
                .read_events(Some(after), 32)
                .expect("original Journal page");
            assert!(
                !page.events.is_empty(),
                "captured original J prefix remains readable"
            );
            assert_eq!(
                page.next_sequence,
                page.events.last().map(|event| event.sequence)
            );
            for envelope in page.events {
                assert!(envelope.sequence > after, "original J advances");
                after = envelope.sequence;
                if after > through {
                    break;
                }
                assert_eq!(envelope.conversation_id, self.target.conversation_id);
                if envelope.attempt_id.as_ref() == Some(owner) {
                    last_owned = Some(envelope.sequence);
                    if matches!(
                        envelope.event,
                        E::AttemptCompleted { .. }
                            | E::AttemptFailed { .. }
                            | E::AttemptCancelled { .. }
                            | E::AttemptTimedOut { .. }
                            | E::AttemptLimitExceeded { .. }
                    ) {
                        original_terminals.push(envelope.sequence);
                    }
                }
            }
        }
        assert_eq!(
            original_terminals,
            [terminal[0].sequence],
            "one exact original native terminal"
        );
        assert_eq!(
            last_owned,
            Some(terminal[0].sequence),
            "terminal is the last original owner fact"
        );
    }

    async fn settle(&self) -> (ObservedEvents, RuntimeClientOutcome) {
        self.settle_observing(|_| {}).await
    }

    async fn settle_observing(
        &self,
        mut observe: impl FnMut(&RuntimeClientEvent),
    ) -> (ObservedEvents, RuntimeClientOutcome) {
        let mut observed = ObservedEvents::default();
        let mut events = self.events.lock().await;
        let mut owner = None;
        loop {
            let record = match tokio::time::timeout(
                std::time::Duration::from_secs(30),
                events.recv(),
            )
            .await
            {
                Ok(Some(record)) => record,
                Ok(other) => panic!("the observation stream ended early: {other:?}"),
                Err(elapsed) => panic!(
                    "the attempt never settled ({elapsed}); {} complete records, cursor={}, owner={owner:?}",
                    observed.records.len(),
                    self.consumed_cursor
                        .load(std::sync::atomic::Ordering::SeqCst)
                ),
            };
            let NotificationMethod::Event { cursor, event, .. } = &record else {
                unreachable!()
            };
            for fact in event.facts() {
                observe(fact);
            }
            let previous = self
                .consumed_cursor
                .swap(cursor.get(), std::sync::atomic::Ordering::SeqCst);
            assert_eq!(
                cursor.get(),
                previous + 1,
                "complete native records stay contiguous"
            );
            if let RuntimeClientEvent::ProjectionTransition { metadata, .. } = event.as_ref() {
                assert_eq!(metadata.read_cut.revision, cursor.get());
            }
            if owner.is_none() {
                owner = self.submitted_attempt();
            }
            let outcome = owner.as_ref().and_then(|(start, expected)| {
                event.facts().iter().find_map(|fact| match fact {
                    RuntimeClientEvent::AttemptSettled {
                        attempt_id,
                        outcome,
                    } if attempt_id == expected => Some(outcome.clone()),
                    RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. }
                        if checkpoint.read_cut.through >= *start =>
                    {
                        let attempt = checkpoint.snapshot.attempt.as_ref()?;
                        if attempt.attempt_id.as_ref() != Some(expected) {
                            return None;
                        }
                        if let RuntimeClientAttemptPhase::Settled { outcome } = &attempt.phase {
                            Some(outcome.clone())
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
            });
            observed.records.push(record);
            if let Some(outcome) = outcome {
                let (start, attempt_id) = owner.as_ref().unwrap();
                self.assert_original_terminal(*start, attempt_id);
                return (observed, outcome);
            }
        }
    }

    /// The committed Assistant text of the conversation, concatenated.
    fn committed_assistant_text(&self) -> String {
        let (snapshot, _) = self.snapshot().expect("snapshot");
        snapshot
            .messages
            .iter()
            .filter_map(|message| match message {
                MessageBlock::Assistant(assistant) => Some(assistant),
                _ => None,
            })
            .flat_map(|assistant| assistant.content.iter())
            .filter_map(|block| match block {
                rustx::message::types::AssistantContentBlock::Text(text) => {
                    Some(text.text.as_str())
                }
                _ => None,
            })
            .collect()
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.connection.close();
        self.relay.abort();
    }
}

async fn rpc(connection: &AppServerConnection, call: Method) -> Result<MethodResult, RpcError> {
    match connection
        .handle_request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(1),
            call,
        })
        .await
    {
        Response::Success(success) => Ok(success.result),
        Response::Failure(failure) => Err(failure.error),
    }
}

/// The catalog: one OpenAI-family provider and one Anthropic provider, both
/// pointed at the emulator. The credential is resolved by the runtime from
/// the environment exactly as in production.
fn models_json(emulator: &ProviderEmulator, setup: &Setup) -> String {
    let window = setup.context_window;
    let mut models = serde_json::Map::new();
    for id in [CHAT_MODEL, SECOND_MODEL, SUMMARY_MODEL] {
        let mut model = chat_model(id, window);
        model["provider"] = "emulator".into();
        models.insert(format!("emulator/{id}"), model);
    }
    for (provider, id, protocol) in [
        ("emulator", RESPONSES_MODEL, "openai_responses"),
        ("emulator-anthropic", ANTHROPIC_MODEL, "anthropic_messages"),
    ] {
        models.insert(
            format!("{provider}/{id}"),
            serde_json::json!({
                "provider": provider, "id": id, "protocol": protocol,
                "context_window": window, "max_output_tokens": 1024,
                "capabilities": if setup.images && protocol == "anthropic_messages" { serde_json::json!({"input_modalities": ["text", "image"], "output_modalities": ["text"]}) } else { text_capabilities() },
            }),
        );
    }
    if setup.images {
        for (id, protocol) in [
            ("chat-image", "openai_chat_completions"),
            ("responses-image", "openai_responses"),
        ] {
            models.insert(format!("emulator/{id}"), serde_json::json!({
                "provider":"emulator", "id":id, "protocol":protocol,
                "compat": if protocol == "openai_chat_completions" { serde_json::json!({"chat_reasoning_replay":"omit"}) } else { serde_json::json!({}) },
                "context_window":window, "max_output_tokens":1024,
                "capabilities":{"input_modalities":["text","image"],"output_modalities":["text"]}
            }));
        }
    }
    toml::to_string_pretty(&serde_json::json!({
        "providers": {
            "emulator": { "base_url": emulator.openai_base_url(), "api_key": format!("${CREDENTIAL_VARIABLE}") },
            "emulator-anthropic": { "base_url": emulator.base_url(), "api_key": format!("${CREDENTIAL_VARIABLE}") },
        },
        "models": models,
    })).unwrap()
}

fn chat_model(id: &str, window: u64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "protocol": "openai_chat_completions",
        "context_window": window,
        "max_output_tokens": 1024,
        "capabilities": text_capabilities(),
        "compat": {"chat_reasoning_replay": "omit"},
    })
}

fn text_capabilities() -> serde_json::Value {
    serde_json::json!({
    "input_modalities": ["text"],
    "output_modalities": ["text"],
    })
}

fn session_json(setup: &Setup) -> String {
    let mut model = serde_json::json!({"model": setup.model});
    if let Some(summary) = &setup.summary_model {
        model["summary_model"] = serde_json::json!({"mode": "explicit", "model": summary});
    }
    let mut document = serde_json::json!({
        "agent_id": "agent-issue47",
        "agent": {"model": model, "skills": "all", "workflows": if setup.images { vec!["image_review"] } else { vec![] }, "tools": {"builtin": if setup.images { vec!["read", "write", "edit", "glob", "grep", "bash", "read_image"] } else { vec!["read", "write", "edit", "glob", "grep", "bash"] }}, "plugins": {"agent_status": {"enabled": true}, "todo": {"enabled": true}}},
        "context": {
            "reserve_tokens": setup.reserve_tokens,
            "keep_recent_tokens": setup.keep_recent_tokens,
        },
    });
    if let Some(instructions) = &setup.instructions {
        document["agent"]["instructions"] = serde_json::json!(instructions);
    }
    toml::to_string_pretty(&document).unwrap()
}

/// The provider-request bodies the emulator recorded, as raw JSON text.
fn body_text(request: &serde_json::Value) -> String {
    serde_json::to_string(&request["body"]).expect("serialize the recorded body")
}

// ---------------------------------------------------------------------------
// Scenario A — a normal streamed turn, on every supported protocol
// ---------------------------------------------------------------------------

/// One fixed user input produces exactly one provider request, one canonical
/// agent commit, and exactly one successful terminal settlement — over each
/// of the three real provider protocols.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_normal_streamed_turn_settles_once_on_every_protocol() {
    for (scenario, model) in [
        (
            "openai_chat_streamed_turn",
            format!("emulator/{CHAT_MODEL}"),
        ),
        (
            "openai_responses_streamed_turn",
            format!("emulator/{RESPONSES_MODEL}"),
        ),
        (
            "anthropic_streamed_turn",
            format!("emulator-anthropic/{ANTHROPIC_MODEL}"),
        ),
    ] {
        let Some(emulator) = ProviderEmulator::start(scenario).await else {
            return;
        };
        let driver = Driver::start(&emulator, &Setup::new(&model)).await;

        driver.submit(TURN_ONE);
        let (events, outcome) = driver.settle().await;

        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: {outcome:?}"
        );
        assert_eq!(
            driver.committed_assistant_text(),
            "Hello world",
            "{scenario}: the streamed deltas commit as one canonical message"
        );
        // The scripted response leads with the protocol's full normal
        // reasoning lifecycle, so this also proves the real stream parser
        // accepts it rather than only the text subset.
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match event {
                    RuntimeClientEvent::BlockContentAdvanced {
                        kind,
                        inline_delta: Some(delta),
                        ..
                    } if kind == "reasoning" => Some(delta.as_str()),
                    _ => None,
                })
                .collect::<String>(),
            "the conformance plan",
            "{scenario}: the reasoning block reached the runtime"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RuntimeClientEvent::AttemptSettled { .. }))
                .count(),
            1,
            "{scenario}: exactly one terminal settlement"
        );
        assert!(
            matches!(
                events.last(),
                Some(RuntimeClientEvent::AttemptSettled { .. })
            ),
            "{scenario}: the terminal event is last"
        );

        let requests = emulator.requests().await;
        assert_eq!(
            requests.len(),
            1,
            "{scenario}: exactly one provider request"
        );
        assert_eq!(requests[0]["model"], serde_json::json!(model_id(&model)));
        assert!(
            system_text(&requests[0])
                .starts_with("You are an expert coding assistant operating inside rustX."),
            "{scenario}: the default System Prompt needs no authored instructions"
        );
        assert_eq!(
            requests[0]["credentialHeaders"]
                .as_array()
                .expect("credential headers")
                .len(),
            1,
            "{scenario}: the runtime authenticated the request itself"
        );
        assert!(
            !body_text(&requests[0]).contains(CREDENTIAL_VALUE),
            "{scenario}: a credential never reaches the request body"
        );
        assert!(
            body_text(&requests[0]).contains("<system-reminder>"),
            "{scenario}: Agent Status reached the provider as canonical context"
        );
        emulator.finish().await;
    }
}

/// Built-in guidance, Agent instructions, global, workspace and explicit files cross
/// every provider boundary in the same order and survive frozen reconstruction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn default_system_prompt_layers_agent_and_project_instructions_on_every_protocol() {
    for (scenario, model) in [
        (
            "openai_chat_streamed_turn",
            format!("emulator/{CHAT_MODEL}"),
        ),
        (
            "openai_responses_streamed_turn",
            format!("emulator/{RESPONSES_MODEL}"),
        ),
        (
            "anthropic_streamed_turn",
            format!("emulator-anthropic/{ANTHROPIC_MODEL}"),
        ),
    ] {
        let Some(emulator) = ProviderEmulator::start(scenario).await else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let global_directory = root.path().join(".test-host/rustx/.agents");
        std::fs::create_dir_all(&global_directory).unwrap();
        let global = "Global guidance: show the validation evidence.";
        std::fs::write(
            global_directory.join("AGENTS.md"),
            format!("\u{feff}{global}"),
        )
        .unwrap();
        let project = "Project guidance: preserve the public API.";
        std::fs::write(workspace.join("AGENTS.md"), format!("\u{feff}{project}")).unwrap();
        let explicit = "Explicit guidance: prefer focused changes.";
        std::fs::write(workspace.join("extra.md"), format!("\u{feff}{explicit}")).unwrap();
        std::fs::write(
            workspace.join("rustx.toml"),
            "[agent.agents_md]\nfiles = ['extra.md']\n",
        )
        .unwrap();
        let instructions = "Agent guidance: explain the evidence for changes.";
        let setup = Setup {
            instructions: Some(instructions.to_owned()),
            ..Setup::new(&model)
        };
        let driver = Driver::start_in(root, &emulator, &setup).await;
        driver.submit(TURN_ONE);
        let (_, outcome) = driver.settle().await;
        assert!(matches!(outcome, RuntimeClientOutcome::Completed { .. }));

        let requests = emulator.requests().await;
        assert_eq!(requests.len(), 1);
        let prompt = system_text(&requests[0]);
        assert!(prompt.starts_with("You are an expert coding assistant operating inside rustX."));
        assert!(prompt.contains(&format!(
            "Current working directory: {}",
            workspace.canonicalize().unwrap().display()
        )));
        assert!(prompt.ends_with(&format!(
            "\n\n{instructions}\n\nProject-specific instructions and guidelines:\n\n<project_instructions path=\"{}\">\n{global}\n</project_instructions>\n\n<project_instructions path=\"{}\">\n{project}\n</project_instructions>\n\n<project_instructions path=\"{}\">\n{explicit}\n</project_instructions>",
            global_directory.canonicalize().unwrap().join("AGENTS.md").display(),
            workspace.canonicalize().unwrap().join("AGENTS.md").display(),
            workspace.canonicalize().unwrap().join("extra.md").display()
        )));
        let snapshots = driver.request_history().page(None, 32).unwrap().snapshots;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].effective_system_prompt, prompt);
        std::fs::write(global_directory.join("AGENTS.md"), "Later global edit").unwrap();
        std::fs::write(workspace.join("AGENTS.md"), "Later workspace edit").unwrap();
        std::fs::write(workspace.join("extra.md"), "Later explicit edit").unwrap();
        let reconstructed = driver.reconstruct_request(&snapshots[0].identity).unwrap();
        assert_eq!(reconstructed.effective_system_prompt, prompt);
        assert!(driver.canonical_history().iter().all(|message| {
            let json = serde_json::to_string(message).unwrap();
            ![instructions, global, project, explicit]
                .iter()
                .any(|text| json.contains(text))
        }));
        emulator.finish().await;
    }
}

fn system_text(request: &serde_json::Value) -> String {
    let body = &request["body"];
    if let Some(instructions) = body["instructions"].as_str() {
        instructions.to_owned()
    } else if let Some(system) = body["system"].as_array() {
        system
            .iter()
            .map(|block| block["text"].as_str().unwrap())
            .collect()
    } else {
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "system")
            .map(|message| message["content"].as_str().unwrap())
            .collect()
    }
}

fn model_id(reference: &str) -> &str {
    reference.split_once('/').expect("provider/model").1
}

// ---------------------------------------------------------------------------
// Scenario B — provider tool call -> real tool -> continuation
// ---------------------------------------------------------------------------

/// The provider requests a tool; rustX executes the **real** native Read
/// through `ConversationToolRuntime`; the continuation request carries the
/// real tool result. The emulator never touches the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tool_call_runs_the_real_tool_and_continues() {
    // The workspace exists before the emulator starts: the scripted tool
    // call carries the fixture's absolute path so the emulator can substitute
    // the concrete root at scenario build time.
    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(workspace.join(NOTE_PATH), format!("{NOTE_MARKER}\n")).expect("workspace note");
    let Some(emulator) =
        ProviderEmulator::start_with_workspace("tool_call_continuation", Some(&workspace)).await
    else {
        return;
    };
    let driver = Driver::start_in(
        root,
        &emulator,
        &Setup::new(&format!("emulator/{CHAT_MODEL}")),
    )
    .await;

    driver.submit(READ_PROMPT);
    let (events, outcome) = driver.settle().await;

    assert!(matches!(outcome, RuntimeClientOutcome::Completed { .. }));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RuntimeClientEvent::ToolExecutionSettled { .. })),
        "the real tool plane executed the call: {events:?}"
    );
    assert!(driver.committed_assistant_text().contains(NOTE_MARKER));

    let requests = emulator.requests().await;
    assert_eq!(
        requests.len(),
        2,
        "one turn plus one tool continuation, and no extra model turn"
    );
    assert!(
        !body_text(&requests[0]).contains(NOTE_MARKER),
        "the first request cannot already carry a tool result"
    );
    assert!(
        body_text(&requests[1]).contains(NOTE_MARKER),
        "the continuation carries the result rustX's own Read produced"
    );
    emulator.finish().await;
}

// ---------------------------------------------------------------------------
// Scenario C — a real workspace Skill
// ---------------------------------------------------------------------------

/// rustX discovers a real workspace Skill, renders its compact catalog through
/// the normal Effective System Prompt path, and executes the real Read of the
/// Skill's own `SKILL.md`. Python implements no part of that.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_skill_reaches_the_provider_and_is_read_by_the_real_tool() {
    // The Skill must exist before composition: the capability candidate is
    // prepared and committed during `compose`. The workspace also exists
    // before the emulator starts so the capability snapshot can publish the
    // host location the scripted Read call uses.
    let root = tempfile::tempdir().expect("temp root");
    let workspace = root.path().join("workspace");
    let package = workspace.join(".agents/skills").join(SKILL_NAME);
    std::fs::create_dir_all(&package).expect("skill package");
    std::fs::write(
        package.join("SKILL.md"),
        format!(
            "---\nname: {SKILL_NAME}\ndescription: {SKILL_DESCRIPTION}\n---\n\n{SKILL_BODY_MARKER}\n"
        ),
    )
    .expect("SKILL.md");
    let Some(emulator) =
        ProviderEmulator::start_with_workspace("skill_read_turn", Some(&workspace)).await
    else {
        return;
    };
    let mut setup = Setup::new(&format!("emulator/{CHAT_MODEL}"));
    setup.skill_paths = vec![package];
    let driver = Driver::start_in(root, &emulator, &setup).await;

    let (snapshot, _) = driver.snapshot().expect("snapshot");
    assert_eq!(
        snapshot
            .capabilities
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec![SKILL_NAME],
        "rustX discovered the workspace Skill"
    );

    driver.submit(SKILL_PROMPT);
    let (_, outcome) = driver.settle().await;
    assert!(matches!(outcome, RuntimeClientOutcome::Completed { .. }));

    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 2);
    let first = body_text(&requests[0]);
    assert!(
        first.contains(SKILL_NAME) && first.contains(SKILL_DESCRIPTION),
        "the Skill catalog reached the provider through the unified system-prompt path"
    );
    let published = snapshot.capabilities.skills[0].location.clone();
    assert!(
        published.ends_with(&format!(".agents/skills/{SKILL_NAME}/SKILL.md")),
        "the catalog publishes the package's host SKILL.md path: {published}"
    );
    assert!(
        first.contains(
            std::path::Path::new(&published)
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_str()
                .unwrap()
        ) && !first.contains(&published),
        "the provider receives the absolute Skill root and package name, without path enumeration"
    );
    assert!(
        !first.contains(SKILL_BODY_MARKER),
        "the initial request contains routing metadata, not the full SKILL.md body"
    );
    let wire_messages = requests[0]["body"]["messages"]
        .as_array()
        .expect("Chat Completions messages");
    assert!(
        wire_messages.iter().any(|message| {
            message["role"] == "system"
                && message["content"].to_string().contains("## Skills")
                && message["content"].to_string().contains(SKILL_NAME)
        }),
        "the provider saw Skill guidance in the translated system prompt"
    );
    assert!(
        !wire_messages.iter().any(|message| {
            message["role"] == "user" && message["content"].to_string().contains("## Skills")
        }),
        "the provider received a canonical User Skill catalog message"
    );
    assert!(
        body_text(&requests[1]).contains(SKILL_BODY_MARKER),
        "the continuation carries the SKILL.md the real Read returned"
    );
    emulator.finish().await;
}

// ---------------------------------------------------------------------------
// Scenario D — a deterministic provider failure
// ---------------------------------------------------------------------------

/// A permanent provider HTTP failure settles the attempt exactly once as a
/// normalized model failure, with exactly one provider request: no retry and
/// no extra model turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_failure_settles_once_without_a_retry() {
    let Some(emulator) = ProviderEmulator::start("provider_http_error").await else {
        return;
    };
    let driver = Driver::start(&emulator, &Setup::new(&format!("emulator/{CHAT_MODEL}"))).await;

    driver.submit(TURN_ONE);
    let (events, outcome) = driver.settle().await;

    let RuntimeClientOutcome::Failed {
        error,
        error_deferred,
        error_source,
    } = &outcome
    else {
        panic!("a provider failure settles as Failed: {outcome:?}");
    };
    assert!(
        !error_deferred,
        "the finite normalized provider failure is complete"
    );
    assert!(
        error_source.is_none(),
        "a complete failure needs no deferred source"
    );
    let RuntimeClientAttemptFailure::Model { message, .. } = error else {
        panic!("the failure is projected as a model failure: {error:?}");
    };
    assert!(
        !message.is_empty(),
        "the normalized model failure retains its diagnostic"
    );
    assert!(
        !message.contains(CREDENTIAL_VALUE),
        "the projected failure never carries a credential"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeClientEvent::AttemptSettled { .. }))
            .count(),
        1
    );
    assert!(driver.committed_assistant_text().is_empty());
    assert_eq!(
        emulator.requests().await.len(),
        1,
        "a failing provider response is not retried into a second request"
    );
    emulator.finish().await;
}

// ---------------------------------------------------------------------------
// Scenario E — an interrupted stream, ordered by a gate rather than a sleep
// ---------------------------------------------------------------------------

/// Cancellation lands provably after the first streamed delta and before
/// anything the gate holds back.
///
/// The linearization point is the emulator's `gate_reached` observation, not
/// a timer: the driver waits for it, cancels, and then waits for the
/// provider to observe the client disconnect. Nothing sleeps.
///
/// This asserts one terminal cancellation settlement, no extra provider
/// request, and a physically closed provider connection.
///
/// Issue #12 (M9b) durable-start facts: the emulator gate is only reachable
/// **after** the model-turn start commit (the provider is never invoked
/// before it), so at the gate the request is durably started — exactly one
/// `ModelRequestStarted` and exactly one started Request Snapshot — and the
/// later cancellation settles that started request without ever
/// reclassifying it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_at_a_provider_gate_settles_once_and_closes_the_stream() {
    let Some(emulator) = ProviderEmulator::start("gated_stream_cancellation").await else {
        return;
    };
    let driver = Driver::start(&emulator, &Setup::new(&format!("emulator/{CHAT_MODEL}"))).await;

    driver.submit(TURN_ONE);

    // The provider has flushed "partial" and is suspended. Everything after
    // this point provably happens after that flush.
    emulator.await_gate("before-remaining-text").await;

    // M9b: the provider is only invoked after the durable model-turn start
    // commit, so with the request in flight the start fact already exists:
    // exactly one started Request Snapshot, bound to exactly one
    // `ModelRequestStarted` event.
    let snapshots = driver
        .request_history()
        .page(None, 32)
        .expect("request snapshot page")
        .snapshots;
    assert_eq!(
        snapshots.len(),
        1,
        "exactly one started Request Snapshot exists before cancellation"
    );
    let store = rustx::durable::SqliteConversationStore::open_existing(
        driver.access.node.conversation_id.clone(),
        &driver.access.database_path,
    )
    .expect("open the durable store for event assertions");
    let started_facts = store
        .read_events(None, 128)
        .expect("event journal")
        .events
        .iter()
        .filter(|envelope| {
            matches!(
                envelope.event,
                rustx::events::types::RuntimeEvent::ModelRequestStarted { .. }
            )
        })
        .count();
    assert_eq!(
        started_facts, 1,
        "exactly one request-start fact exists before cancellation"
    );

    let accepted = driver
        .cancel_current_attempt()
        .await
        .expect("the in-flight attempt is cancellable");
    assert!(matches!(
        accepted,
        MethodResult::CancellationAccepted { .. }
    ));

    // The runtime really dropped the provider connection; this is observed,
    // not assumed.
    emulator.await_client_disconnect().await;
    emulator.release_gate("before-remaining-text").await;

    let (events, outcome) = driver.settle().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Cancelled { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeClientEvent::AttemptSettled { .. }))
            .count(),
        1
    );
    assert!(
        !driver
            .committed_assistant_text()
            .contains("remainder that cancellation must prevent"),
        "content the gate held back can never be committed"
    );
    assert_eq!(
        emulator.requests().await.len(),
        1,
        "cancellation never starts another model turn"
    );
    emulator.finish().await;
}

// ---------------------------------------------------------------------------
// Scenario F — compaction through the real provider boundary
// ---------------------------------------------------------------------------

/// Compaction invokes the real provider and the rewritten Conversation
/// Surface reaches the next primary request, on both summary-model policies.
///
/// This is the composed, external proof of the M7.5 semantics:
///
/// ```text
/// real summary provider invocation
///   → canonical User(Runtime / CompactionSummary) Message Ledger commit
///   → Conversation Surface rewrite
///   → the next real provider request carries the summary
///   → the retired original filler is absent from the active provider context
///     while remaining a committed ledger fact
/// ```
///
/// The trigger is structural, not statistical: the catalog window, the
/// reserve, the output budget, and the emulator's scripted reply size fix
/// the threshold crossing exactly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
// The composed compaction proof is one scenario observed end to end; keeping
// it together preserves the request ordering that is the whole point.
#[allow(clippy::too_many_lines)]
async fn compaction_invokes_the_real_provider_on_both_summary_policies() {
    for (scenario, summary_model, expected_summary_model) in [
        ("compaction_session_summary", None, CHAT_MODEL),
        (
            "compaction_explicit_summary",
            Some(format!("emulator/{SUMMARY_MODEL}")),
            SUMMARY_MODEL,
        ),
    ] {
        let Some(emulator) = ProviderEmulator::start(scenario).await else {
            return;
        };
        let setup = Setup {
            context_window: COMPACTION_CONTEXT_WINDOW,
            reserve_tokens: COMPACTION_RESERVE_TOKENS,
            keep_recent_tokens: 256,
            summary_model,
            ..Setup::new(&format!("emulator/{CHAT_MODEL}"))
        };
        let driver = Driver::start(&emulator, &setup).await;

        driver.submit(TURN_ONE);
        let (_, outcome) = driver.settle().await;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: the filling turn completes: {outcome:?}"
        );

        driver.submit(TURN_TWO);
        let (events, outcome) = driver.settle().await;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: {outcome:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, RuntimeClientEvent::ContextCompacted { .. })),
            "{scenario}: the runtime committed the canonical compaction: {events:?}"
        );

        let (snapshot, _) = driver.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.context.compaction_count, 1,
            "{scenario}: exactly one compaction"
        );
        let latest = snapshot
            .context
            .latest_compaction
            .as_ref()
            .expect("the committed compaction metadata");
        assert_eq!(
            latest.generation, 1,
            "{scenario}: one compaction generation"
        );

        let canonical = driver.canonical_history();
        let head = snapshot
            .context
            .projection_head
            .as_ref()
            .expect("durable current projection");
        assert_eq!(head.checkpoint_id, latest.checkpoint_id);
        assert_eq!(head.activation_id, latest.activation_id);
        assert!(head.destination_installation.is_none());
        assert!(head.summary_preview.is_some());
        // Compaction never rewrote the ledger: the retired original is still
        // there, byte for byte.
        assert!(
            canonical.iter().any(|message| matches!(
                message,
                MessageBlock::Assistant(assistant)
                    if serde_json::to_string(assistant)
                        .expect("serialize the retired Assistant message")
                        .contains(COMPACTION_MARKER)
            )),
            "{scenario}: the retired original stays an immutable ledger fact"
        );

        let requests = emulator.requests().await;
        assert_eq!(
            requests.len(),
            3,
            "{scenario}: turn one, the summary invocation, and the compacted turn two"
        );
        assert_eq!(
            requests[1]["model"],
            serde_json::json!(expected_summary_model),
            "{scenario}: the summary used the configured summary model"
        );
        assert!(
            body_text(&requests[1]).contains(COMPACTION_MARKER),
            "{scenario}: the summary request carried the retired history"
        );
        let third = body_text(&requests[2]);
        assert!(
            third.contains(SUMMARY_TEXT) && !third.contains(COMPACTION_MARKER),
            "{scenario}: the rewritten surface replaced the retired history"
        );
        emulator.finish().await;
    }
}

/// A transient failure crosses the real HTTP adapter, auxiliary retry writer,
/// new physical request and normal checkpoint commit with byte-identical input.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn summary_retry_reuses_frozen_wire_input_and_commits_one_checkpoint() {
    let Some(emulator) = ProviderEmulator::start("compaction_transient_summary").await else {
        return;
    };
    let setup = Setup {
        context_window: COMPACTION_CONTEXT_WINDOW,
        reserve_tokens: COMPACTION_RESERVE_TOKENS,
        keep_recent_tokens: 256,
        summary_model: Some(format!("emulator/{SUMMARY_MODEL}")),
        ..Setup::new(&format!("emulator/{CHAT_MODEL}"))
    };
    let driver = Driver::start(&emulator, &setup).await;
    driver.submit(TURN_ONE);
    assert!(matches!(
        driver.settle().await.1,
        RuntimeClientOutcome::Completed { .. }
    ));
    driver.submit(TURN_TWO);
    assert!(matches!(
        driver.settle().await.1,
        RuntimeClientOutcome::Completed { .. }
    ));
    assert_eq!(driver.snapshot().unwrap().0.context.compaction_count, 1);
    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests[1]["body"], requests[2]["body"],
        "retry sends the frozen invocation and selected input exactly"
    );
    let journal = driver.store.read_events(None, 256).unwrap().events;
    let retry = journal
        .iter()
        .find(|event| {
            matches!(
                event.event,
                rustx::events::types::RuntimeEvent::AuxiliaryRetryScheduled { .. }
            )
        })
        .unwrap();
    let rustx::events::types::RuntimeEvent::AuxiliaryRetryScheduled {
        call_id,
        request_id,
        next_ordinal,
        delay_ms,
        ..
    } = &retry.event
    else {
        unreachable!()
    };
    assert_eq!((*next_ordinal, *delay_ms), (1, 2_000));
    assert!(journal.iter().any(|event| event.sequence < retry.sequence && matches!(&event.event, rustx::events::types::RuntimeEvent::ModelRequestFailed { request_id: failed, .. } if failed == request_id)));
    assert_eq!(journal.iter().filter(|event| matches!(&event.event, rustx::events::types::RuntimeEvent::ModelRequestStarted { owner: rustx::model::ModelRequestOwner::Auxiliary { call_id: actual, .. }, .. } if actual == call_id)).count(), 2);
    emulator.finish().await;
}

// ---------------------------------------------------------------------------
// Scenario F2 — repeated compaction through the real provider boundary
// ---------------------------------------------------------------------------

/// Two committed compactions through the real provider boundary, on both
/// summary-model policies (Issue #27).
///
/// Where Scenario F proves one compaction, this scenario proves the
/// composition: the second compaction operates on the already-compacted
/// surface, its summary input retires the still-active first summary, and
/// retired bytes never reach the provider again — asserted externally on the
/// recorded wire bodies and internally on the canonical ledger, the frozen
/// request snapshots, and their historical reconstructions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
// The composed compaction proof is one scenario observed end to end; keeping
// it together preserves the request ordering that is the whole point.
#[allow(clippy::too_many_lines)]
async fn repeated_compaction_invokes_the_real_provider_on_both_summary_policies() {
    for (scenario, summary_model, expected_summary_model) in [
        ("compaction_twice_session_summary", None, CHAT_MODEL),
        (
            "compaction_twice_explicit_summary",
            Some(format!("emulator/{SUMMARY_MODEL}")),
            SUMMARY_MODEL,
        ),
    ] {
        let Some(emulator) = ProviderEmulator::start(scenario).await else {
            return;
        };
        let setup = Setup {
            context_window: COMPACTION_CONTEXT_WINDOW,
            reserve_tokens: COMPACTION_RESERVE_TOKENS,
            keep_recent_tokens: 256,
            summary_model,
            ..Setup::new(&format!("emulator/{CHAT_MODEL}"))
        };
        let driver = Driver::start(&emulator, &setup).await;

        driver.submit(TURN_ONE);
        let (_, outcome) = driver.settle().await;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: the filling turn completes: {outcome:?}"
        );

        driver.submit(TURN_TWO);
        let (events_two, outcome) = driver.settle().await;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: {outcome:?}"
        );
        assert!(
            events_two
                .iter()
                .any(|event| matches!(event, RuntimeClientEvent::ContextCompacted { .. })),
            "{scenario}: compaction #1 committed: {events_two:?}"
        );

        driver.submit(TURN_THREE);
        let (events_three, outcome) = driver.settle().await;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: {outcome:?}"
        );

        // Exactly two committed compactions, in generation order.
        let generations: Vec<u64> = events_two
            .iter()
            .chain(events_three.iter())
            .filter_map(|event| match event {
                RuntimeClientEvent::ContextCompacted { context, .. } => context
                    .latest_compaction
                    .as_ref()
                    .map(|view| view.generation),
                _ => None,
            })
            .collect();
        assert_eq!(
            generations,
            [1, 2],
            "{scenario}: two compaction generations in order"
        );

        let (snapshot, _) = driver.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.context.compaction_count, 2,
            "{scenario}: exactly two compactions"
        );
        assert_eq!(
            snapshot
                .context
                .latest_compaction
                .as_ref()
                .expect("the committed compaction metadata")
                .generation,
            2
        );

        let head = snapshot
            .context
            .projection_head
            .as_ref()
            .expect("second projection");
        assert_eq!(head.activation_id.0, 2);
        assert_eq!(head.derived_items, 1, "one effective Derived Summary");
        let ledger =
            serde_json::to_string(&driver.canonical_history()).expect("serialize the ledger");
        assert!(
            ledger.contains(FILLER_ONE_MARKER) && ledger.contains(FILLER_TWO_MARKER),
            "{scenario}: the ledger retains both retired fillers verbatim"
        );
        assert!(
            !ledger.contains(SUMMARY_ONE_TEXT) && !ledger.contains(SUMMARY_TWO_TEXT),
            "summary context is independently durable, never a canonical execution message"
        );
        let projections = driver
            .store
            .projection_timeline(rustx::context::checkpoint::ProjectionActivationId(0), 64)
            .unwrap();
        let projected = serde_json::to_string(&projections).unwrap();
        assert!(projected.contains(SUMMARY_ONE_TEXT) && projected.contains(SUMMARY_TWO_TEXT));

        // The wire: five requests, each exactly what the scenario asserts.
        let requests = emulator.requests().await;
        assert_eq!(
            requests.len(),
            5,
            "{scenario}: three turns plus two summaries"
        );
        assert_eq!(
            requests[1]["model"],
            serde_json::json!(expected_summary_model),
            "{scenario}: summary #1 used the configured summary model"
        );
        assert_eq!(
            requests[3]["model"],
            serde_json::json!(expected_summary_model),
            "{scenario}: summary #2 used the configured summary model"
        );
        let bodies: Vec<String> = requests.iter().map(body_text).collect();
        assert!(bodies[0].contains(TURN_ONE));
        assert!(bodies[1].contains(FILLER_ONE_MARKER) && !bodies[1].contains(FILLER_TWO_MARKER));
        assert!(
            bodies[2].contains(SUMMARY_ONE_TEXT)
                && bodies[2].contains(TURN_TWO)
                && !bodies[2].contains(FILLER_ONE_MARKER),
            "{scenario}: the first rewritten surface replaced the retired history"
        );
        assert!(
            bodies[3].contains(SUMMARY_ONE_TEXT)
                && bodies[3].contains(FILLER_TWO_MARKER)
                && !bodies[3].contains(FILLER_ONE_MARKER),
            "{scenario}: the second compaction's span is the already-compacted surface"
        );
        assert!(
            bodies[4].contains(SUMMARY_TWO_TEXT)
                && bodies[4].contains(TURN_THREE)
                && !bodies[4].contains(FILLER_ONE_MARKER)
                && !bodies[4].contains(FILLER_TWO_MARKER)
                && !bodies[4].contains(SUMMARY_ONE_TEXT),
            "{scenario}: the second rewritten surface carries exactly the second summary"
        );

        // The frozen request snapshots reconstruct the three primary
        // requests through the real provider path, after both compactions.
        await_history_len(&driver, 3).await;
        let history = driver.request_history();
        let snapshots = crate::common::request_snapshots(&history);
        assert_eq!(
            snapshots.len(),
            3,
            "{scenario}: three frozen primary requests"
        );
        let reconstructed: Vec<String> = snapshots
            .iter()
            .map(|snapshot| {
                let request = driver
                    .reconstruct_request(&snapshot.identity)
                    .expect("settled historical request reconstructs");
                serde_json::to_string(&request.messages).expect("serialize reconstructed messages")
            })
            .collect();
        assert!(
            reconstructed[0].contains(TURN_ONE) && !reconstructed[0].contains(FILLER_ONE_MARKER),
            "{scenario}: R1 reconstructs the pre-filler surface (the filler is its response)"
        );
        assert!(
            reconstructed[1].contains(SUMMARY_ONE_TEXT)
                && reconstructed[1].contains(TURN_TWO)
                && !reconstructed[1].contains(FILLER_ONE_MARKER),
            "{scenario}: R2 reconstructs the first rewritten surface"
        );
        assert!(
            reconstructed[2].contains(SUMMARY_TWO_TEXT)
                && reconstructed[2].contains(TURN_THREE)
                && !reconstructed[2].contains(FILLER_ONE_MARKER)
                && !reconstructed[2].contains(SUMMARY_ONE_TEXT),
            "{scenario}: R3 reconstructs the second rewritten surface"
        );

        emulator.finish().await;
    }
}

/// Waits for the post-settlement transfer of frozen request facts to the
/// runtime-owned append-only history.
async fn await_history_len(driver: &Driver, expected: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if crate::common::request_snapshots(&driver.request_history()).len() == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request history transfer must settle");
}

// ---------------------------------------------------------------------------
// Scenario G — the immutable attempt model, observed from outside rustX
// ---------------------------------------------------------------------------

/// A session-model update that linearizes after admission never changes the
/// running attempt's model, and the next attempt uses the new one.
///
/// The proof is external: the emulator asserts the `model` field of both
/// requests, and the gate — not a sleep — establishes that the update
/// happened while the first attempt was mid-stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_attempt_keeps_its_frozen_model_while_the_session_moves_on() {
    let Some(emulator) = ProviderEmulator::start("frozen_attempt_model").await else {
        return;
    };
    let driver = Driver::start(&emulator, &Setup::new(&format!("emulator/{CHAT_MODEL}"))).await;

    driver.submit(FIRST_ATTEMPT);
    emulator.await_gate("session-model-updated").await;

    // The first attempt is provably in flight and has already been billed
    // its provider request.
    let next_model = SessionModelConfig {
        title_model: AuxiliaryModelPolicy::Session,
        summary_model: AuxiliaryModelPolicy::Session,
        ..SessionModelConfig::of(
            ModelRef::parse(&format!("emulator/{SECOND_MODEL}"))
                .expect("the second model reference parses"),
        )
    };
    driver
        .model_set(next_model)
        .await
        .expect("live selection commits");

    emulator.release_gate("session-model-updated").await;
    let (_, outcome) = driver.settle().await;
    assert!(matches!(outcome, RuntimeClientOutcome::Completed { .. }));

    let (snapshot, _) = driver.snapshot().expect("snapshot");
    let attempt = snapshot.attempt.as_ref().expect("the settled attempt");
    assert!(matches!(
        attempt.phase,
        RuntimeClientAttemptPhase::Settled { .. }
    ));
    assert_eq!(
        attempt.model.as_ref().unwrap().primary.model.to_string(),
        format!("emulator/{CHAT_MODEL}"),
        "the settled attempt reports the model it froze"
    );
    assert_eq!(
        snapshot
            .model
            .as_ref()
            .unwrap()
            .configured
            .model
            .to_string(),
        format!("emulator/{SECOND_MODEL}"),
        "the session moved on"
    );

    driver.submit(SECOND_ATTEMPT);
    let (_, outcome) = driver.settle().await;
    assert!(matches!(outcome, RuntimeClientOutcome::Completed { .. }));

    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["model"], serde_json::json!(CHAT_MODEL));
    assert_eq!(requests[1]["model"], serde_json::json!(SECOND_MODEL));
    emulator.finish().await;
}

// ---------------------------------------------------------------------------
// Scenario H — durable startup recovery against the real provider (Issue #12)
// ---------------------------------------------------------------------------

/// A crash after the request-start commit resends nothing, proven at the real
/// provider boundary.
///
/// The first runtime is composed inside its **own** Tokio runtime on a
/// dedicated thread. Dropping that Tokio runtime drops every runtime-owned
/// task at its current await point and closes the provider connection, which
/// is the closest honest model of process death available without spawning a
/// second binary — and, unlike a `kill`, it is exact: the driver knows the
/// provider had already received request #1 because it waited on the
/// provider-side gate, and it knows the client really vanished because it
/// waits on the provider's disconnect observation. Nothing sleeps.
///
/// ```text
/// runtime #1 -> request-start durable commit -> provider observes request #1
///            -> provider suspends at the gate
///            -> the Tokio runtime hosting runtime #1 is dropped   (crash)
/// same SQLite conversation reopened
///            -> runtime #2 recovers
/// ```
///
/// The scenario declares exactly one step, so a resent request would be an
/// unexpected provider request and would fail the run rather than pass
/// quietly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One crash, one recovery, one composed proof.
async fn a_crash_after_the_request_start_commit_never_resends_the_request() {
    let Some(emulator) = ProviderEmulator::start("restart_after_request_start").await else {
        return;
    };
    // The root outlives both runtimes: instance #2 reopens the exact same
    // durable conversation instance #1 died in.
    let root = tempfile::tempdir().expect("temp root");
    let setup = Setup::new(&format!("emulator/{CHAT_MODEL}"));
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(
        root.path().join("rustx.toml"),
        format!(
            "{}\n{}",
            session_json(&setup),
            models_json(&emulator, &setup)
        ),
    )
    .expect("rustx.toml");
    let paths = LaunchFixture {
        config: root.path().join("rustx.toml"),
        startup_session: rustx::local_runtime::StartupSession::Empty,
        workspace,
        runtime_root: root.path().join("private"),
    };

    // ---- runtime instance #1, in its own execution runtime ----
    let (crash_tx, crash_rx) = tokio::sync::oneshot::channel::<()>();
    let first_paths = paths.clone();
    let first = std::thread::spawn(move || {
        let execution = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("the first runtime's execution runtime");
        execution.block_on(async move {
            let runtime =
                HeadlessConversationRuntime::compose(&(first_paths).resolve(), &dependencies())
                    .await
                    .expect("the first runtime composes");
            runtime
                .runtime()
                .submit_inbound(vec![UserContentBlock::Text(TextBlock {
                    text: TURN_ONE.to_owned(),
                })])
                .expect("inbound accepted");
            // Park until the driver has observed the provider gate. The
            // attempt is in flight, parked on the provider stream.
            let _ = crash_rx.await;
        });
        // Dropping the execution runtime drops every runtime-owned task —
        // the attempt, the admission worker, the HTTP stream — at its await
        // point, and releases the durable store handle.
        drop(execution);
    });

    // The provider has received request #1 and flushed its first delta.
    // Everything after this point provably happens after that.
    emulator.await_gate("before-remaining-text").await;
    let requests_before_crash = emulator.requests().await;
    assert_eq!(requests_before_crash.len(), 1, "exactly one request so far");

    // Crash.
    crash_tx
        .send(())
        .expect("the first runtime is still parked");
    first.join().expect("the first runtime's thread exits");
    // The connection really went away; this is observed, not assumed.
    emulator.await_client_disconnect().await;
    emulator.release_gate("before-remaining-text").await;

    // ---- runtime instance #2 over the same durable conversation ----
    let catalog = rustx::local_runtime::SessionCatalog::open_existing(&paths.runtime_root)
        .unwrap()
        .unwrap();
    let mut resume_paths = paths.clone();
    resume_paths.startup_session = rustx::local_runtime::StartupSession::Select {
        session: catalog.persisted_session_ids()[0].clone(),
        node: None,
    };
    let recovered = HeadlessConversationRuntime::compose(
        &resume_paths.resolve(),
        &LocalRuntimeDependencies {
            startup_session: resume_paths.startup_session.clone(),
            ..dependencies()
        },
    )
    .await
    .expect("the second runtime recovers the durable conversation");
    let report = recovered.runtime().recovery();

    // The crash boundary is the drop of the first execution runtime. The
    // durable journal decides the honest classification, and both landings
    // preserve the invariant under test: recovery resends nothing.
    //
    //   Landing A (the intended crash): the drop aborted the attempt while
    //   it was parked on the gated stream, so the journal holds
    //   `ModelRequestStarted` with no outcome and no terminal — the attempt
    //   classifies as an indeterminate external outcome, the started
    //   request is named explicitly, and continuation is blocked.
    //
    //   Landing B (platform-dependent drop semantics, observed on macOS):
    //   the runtime drop lets the woken attempt settle its terminal as the
    //   connection closes, so the journal honestly records the terminal —
    //   the classification is the absorbing `AlreadyTerminal`. The exact
    //   indeterminate classification is covered deterministically by the
    //   store-prefix test (`started_request_with_unknown_outcome_is_indeterminate_and_never_resent`)
    //   and the runtime test (`class_c_restart_issues_no_provider_request_of_its_own`);
    //   what this scenario adds is the real-provider no-resend proof, which
    //   must hold under both landings.
    match report.attempt_class() {
        AttemptRecoveryClass::IndeterminateExternalOutcome {
            attempt_id,
            model_request,
            ..
        } => {
            let request_id = model_request
                .clone()
                .expect("the started request is named explicitly");
            assert_eq!(
                report.resume(),
                ResumeDisposition::BlockedIndeterminate,
                "an unknown provider outcome never authorizes a continuation"
            );
            assert_eq!(
                report.reconciliation().attempt_terminal.as_ref(),
                Some(attempt_id),
                "the interrupted attempt is settled exactly once by recovery"
            );

            // The historical provider-neutral request reconstructs exactly,
            // from frozen durable facts alone — and reconstructability is
            // still not permission to replay. The identity comes from the
            // retained snapshot itself, never from a guessed turn ordinal.
            let history = recovered.runtime().request_history();
            let page = history.page(None, 8).expect("the retained snapshot page");
            let snapshot = page
                .snapshots
                .iter()
                .find(|snapshot| snapshot.request_id == request_id)
                .expect("the started request retained its immutable snapshot");
            assert_eq!(
                snapshot.identity.attempt_id, *attempt_id,
                "the snapshot belongs to the interrupted attempt"
            );
            let historical = history
                .reconstruct(&snapshot.identity)
                .expect("the historical request reconstructs");
            assert_eq!(
                historical.invocation.model, CHAT_MODEL,
                "the historical request keeps its frozen model, not the current one"
            );
            assert!(
                historical.messages.iter().any(
                    |block| matches!(block.as_canonical(), Some(MessageBlock::User(user))
                    if user.content.iter().any(|content| matches!(
                        content,
                        UserContentBlock::Text(text) if text.text == TURN_ONE
                    )))
                ),
                "the reconstruction is the exact historical request"
            );
        }
        AttemptRecoveryClass::AlreadyTerminal => {
            // The dropped runtime settled the attempt before the process
            // died; the durable journal is honest and absorbing.
            assert_eq!(
                report.resume(),
                ResumeDisposition::PendingInboundOnly,
                "an already-terminal attempt authorizes no continuation"
            );
            assert!(
                report.reconciliation().is_empty(),
                "a terminal attempt needs no recovery fact"
            );
            let history = recovered.runtime().request_history();
            let page = history.page(None, 8).expect("the retained snapshot page");
            assert_eq!(
                page.snapshots.len(),
                1,
                "the started request retained its immutable snapshot"
            );
        }
        other => {
            panic!(
                "a committed request start must classify as indeterminate (or, when the \
                 dropped runtime settled it first, as already terminal): {other:?}"
            );
        }
    }

    // The whole point: the provider saw the request exactly once, under
    // either crash landing. The scenario declares exactly one step, so any
    // resend would have been an unexpected provider request and would have
    // failed the run rather than pass quietly.
    assert_eq!(
        emulator.requests().await.len(),
        1,
        "recovery performed zero automatic provider resend"
    );
    emulator.finish().await;
}

/// The composition dependencies shared by both runtime instances above.
fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Some(Arc::new(MapCredentialEnvironment::new([(
            CREDENTIAL_VARIABLE.to_owned(),
            CREDENTIAL_VALUE.to_owned(),
        )]))),
        ..LocalRuntimeDependencies::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn image_tool_round_trip_and_text_model_switch() {
    use base64::Engine;
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut bytes = Vec::new();
    {
        let encoder = png::Encoder::new(&mut bytes, 1, 1);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[128]).unwrap();
    }
    std::fs::write(workspace.join("sample.png"), &bytes).unwrap();
    let Some(emulator) =
        ProviderEmulator::start_with_workspace("image_tool_round_trip", Some(&workspace)).await
    else {
        return;
    };
    let mut setup = Setup::new(&format!("emulator-anthropic/{ANTHROPIC_MODEL}"));
    setup.images = true;
    setup.keep_recent_tokens = 0;
    let driver = Driver::start_in(root, &emulator, &setup).await;
    driver.submit("read the image");
    emulator.await_gate("image-admitted").await;
    let (snapshot, _) = driver.snapshot().unwrap();
    assert_eq!(
        snapshot
            .attempt
            .as_ref()
            .unwrap()
            .execution_settings
            .as_ref()
            .unwrap()
            .read_image_active,
        Some(true)
    );
    assert!(
        snapshot
            .capabilities
            .tools
            .iter()
            .any(|tool| tool.id.as_str() == "tool-read-image")
    );
    let text_model =
        SessionModelConfig::of(ModelRef::parse(&format!("emulator/{CHAT_MODEL}")).unwrap());
    assert!(
        driver.model_set(text_model.clone()).await.is_ok(),
        "Session selection changes while admitted image capabilities remain frozen"
    );
    emulator.release_gate("image-admitted").await;
    let (_, outcome) = driver.settle().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}"
    );
    std::fs::remove_file(workspace.join("sample.png")).unwrap();
    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 2);
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    assert!(body_text(&requests[1]).contains(&data));
    assert!(body_text(&requests[1]).contains("image/png"));
    let canonical_image = driver.canonical_history().into_iter()
        .find(|message| matches!(message, MessageBlock::Tool(tool) if tool.tool_call_id.as_str() == "call-image"))
        .expect("native image result committed canonically");
    assert!(
        serde_json::to_string(&canonical_image)
            .unwrap()
            .contains("\"type\":\"image\"")
    );
    driver.submit("continue as text");
    emulator.await_gate("text-admitted").await;
    let (snapshot, _) = driver.snapshot().unwrap();
    assert_eq!(
        snapshot
            .attempt
            .as_ref()
            .unwrap()
            .execution_settings
            .as_ref()
            .unwrap()
            .read_image_active,
        Some(false)
    );
    assert!(
        !snapshot
            .capabilities
            .tools
            .iter()
            .any(|tool| tool.id.as_str() == "tool-read-image")
    );
    let image_model = SessionModelConfig::of(
        ModelRef::parse(&format!("emulator-anthropic/{ANTHROPIC_MODEL}")).unwrap(),
    );
    assert!(
        driver.model_set(image_model.clone()).await.is_ok(),
        "Session selection changes while the text Attempt stays frozen"
    );
    emulator.release_gate("text-admitted").await;
    let (_, outcome) = driver.settle().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 3);
    assert!(!body_text(&requests[2]).contains(&data));
    assert!(body_text(&requests[2]).contains("Image artifact"));
    let messages = requests[2]["body"]["messages"].as_array().unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["role"] == "tool" && message["tool_call_id"] == "call-image")
            .count(),
        1
    );
    assert_eq!(
        messages
            .iter()
            .filter_map(|message| message["tool_calls"].as_array())
            .flatten()
            .filter(|call| call["id"] == "call-image")
            .count(),
        1
    );
    assert!(
        driver.canonical_history().contains(&canonical_image),
        "text projection preserves the exact canonical result"
    );
    assert!(
        !requests[2]["body"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "read_image")
    );
    driver.model_set(image_model).await.unwrap();
    driver.submit("inspect the retained image");
    let (_, outcome) = driver.settle().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 4);
    assert!(
        body_text(&requests[3]).contains(&data),
        "canonical image survives text projection and source deletion"
    );
    // The real child process owns a different conversation and a text-only
    // invocation. Parent image history must not leak across that boundary.
    driver.model_set(text_model).await.unwrap();
    driver.submit("delegate the retained image context as text");
    let (_, outcome) = driver.settle().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}: {}",
        emulator.diagnostics()
    );
    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 7);
    for request in &requests[4..] {
        let body = body_text(request);
        assert!(!body.contains(&data));
        assert!(!body.contains("image/png"));
        assert!(!body.contains("\"type\":\"image\""));
        assert!(!body.contains("image_url"));
    }
    assert!(body_text(&requests[4]).contains("call-image"));
    assert!(body_text(&requests[6]).contains("call-image"));
    assert!(body_text(&requests[5]).contains("workflow_output"));
    assert!(
        !body_text(&requests[5]).contains("call-image"),
        "child does not inherit parent history"
    );
    assert!(!body_text(&requests[5]).contains("read_image"));
    assert!(driver.canonical_history().contains(&canonical_image));

    // Manual compaction invokes the production summary request builder and
    // adapter over the image-bearing canonical span, not a helper-only mock.
    // Attempt settlement is logical; the coordinator may still own its tail.
    // This fixture submits no further work before maintenance, so wait for the
    // native idle-owner observation rather than racing that ownership return.
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let (snapshot, _) = driver.snapshot().unwrap();
            if matches!(
                snapshot.configuration_adoption_eligibility,
                rustx::runtime::adoption::AdoptionEligibility::Eligible
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("native idle owners must return after the delegated Attempt settles");
    driver.compact_context().await.unwrap();
    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 8);
    let summary = body_text(&requests[7]);
    assert!(summary.contains("retired-conversation"));
    assert!(summary.contains("artifact_1"));
    assert!(!summary.contains(&data));
    assert!(!summary.contains("image/png"));
    assert!(!summary.contains("\"type\":\"image\""));
    assert!(!summary.contains("image_url"));
    assert!(
        driver.canonical_history().contains(&canonical_image),
        "compaction preserves the canonical ledger"
    );
    assert_eq!(
        driver
            .artifacts()
            .read_bounded(&rustx::runtime::identity::ArtifactId::new("artifact_1"))
            .unwrap(),
        bytes
    );
    await_history_len(&driver, 6).await;
    driver.shutdown().await.unwrap();
    let reopened = rustx::durable::SqliteConversationStore::open_existing(
        driver.access.node.conversation_id.clone(),
        &driver.access.database_path,
    )
    .unwrap();
    let history = rustx::runtime::request_history::RequestHistory::new(Arc::new(reopened));
    let snapshots = crate::common::request_snapshots(&history);
    assert_eq!(snapshots.len(), 6);
    for (index, snapshot) in snapshots.iter().enumerate() {
        let request = history.reconstruct(&snapshot.identity).unwrap();
        let serialized = serde_json::to_string(&request).unwrap();
        assert!(
            !serialized.contains(&data),
            "durable evidence contains no encoded payload"
        );
        if index == 2 || index >= 4 {
            assert!(serialized.contains("Image artifact"));
            assert!(!serialized.contains("\"type\":\"image\""));
            assert!(
                serialized.contains("call-image"),
                "historical call/result correlation survives reconstruction"
            );
        } else if index > 0 {
            assert!(
                serialized.contains("\"type\":\"image\""),
                "canonical image reference survives reopening SQLite"
            );
        }
    }
    emulator.finish().await;
}

#[tokio::test]
async fn cross_turn_image_projection_reconciles_native_coverage_before_frozen_retry() {
    for (scenario, model) in [
        (
            "image_coverage_growth",
            "emulator-anthropic/anthropic-model",
        ),
        (
            "image_coverage_growth_responses",
            "emulator/responses-image",
        ),
    ] {
        let Some((driver, emulator)) = image_coverage_driver(scenario, model).await else {
            return;
        };
        driver.submit("Read sixteen images, then one more.");
        let outcome = driver.settle().await.1;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{scenario}: {outcome:?}"
        );
        let requests = emulator.requests().await;
        assert_eq!(
            requests.len(),
            4,
            "internal validation starts no extra request"
        );
        assert_eq!(
            requests[2]["body"], requests[3]["body"],
            "transient retry retains transformed input and continuation decision"
        );
        assert!(
            requests[2]["body"].get("previous_response_id").is_none(),
            "stale stored coverage was cleared"
        );
        // Accepted-output uniqueness belongs to the complete original Ledger,
        // independently of the bounded checkpoint body cache.
        let canonical = driver.canonical_history();
        assert_eq!(
            canonical
                .iter()
                .filter(|m| matches!(m, MessageBlock::Assistant(_)))
                .count(),
            3,
            "transient failure commits no duplicate accepted output"
        );
        assert_eq!(
            canonical
                .iter()
                .filter(|m| matches!(m, MessageBlock::Tool(_)))
                .count(),
            17
        );
        for m in &canonical {
            if let MessageBlock::Tool(t) = m {
                assert!(
                    t.result
                        .content
                        .iter()
                        .any(|b| matches!(b, rustx::tools::types::ToolResultContent::Image(_)))
                );
            }
        }
        let history = driver.request_history();
        let snapshots = history.page(None, 32).unwrap().snapshots;
        assert_eq!(snapshots.len(), 4);
        assert_eq!(
            snapshots[2].semantic_input_fingerprint,
            snapshots[3].semantic_input_fingerprint
        );
        let third = history.reconstruct(&snapshots[2].identity).unwrap();
        let fourth = history.reconstruct(&snapshots[3].identity).unwrap();
        assert_eq!(
            third, fourth,
            "reconstruction never repeats image selection"
        );
        emulator.finish().await;
    }
}

async fn image_coverage_driver(scenario: &str, model: &str) -> Option<(Driver, ProviderEmulator)> {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut bytes = Vec::new();
    png::Encoder::new(&mut bytes, 1, 1)
        .write_header()
        .unwrap()
        .write_image_data(&[128])
        .unwrap();
    std::fs::write(workspace.join("sample.png"), &bytes).unwrap();
    let emulator = ProviderEmulator::start_with_workspace(scenario, Some(&workspace)).await?;
    let mut setup = Setup::new(model);
    setup.images = true;
    if scenario == "image_coverage_overflow_retry" {
        setup.keep_recent_tokens = setup.context_window;
        setup.summary_model = Some("emulator/summary-model".into());
    }
    let driver = Driver::start_in(root, &emulator, &setup).await;
    Some((driver, emulator))
}

#[tokio::test]
async fn retired_native_coverage_does_not_anchor_overflow_compaction_or_frozen_retry() {
    let Some((driver, emulator)) = image_coverage_driver(
        "image_coverage_overflow_retry",
        "emulator-anthropic/anthropic-model",
    )
    .await
    else {
        return;
    };
    driver.submit("Read sixteen images, then one more.");
    let outcome = driver.settle().await.1;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let requests = emulator.requests().await;
    assert_eq!(
        requests.len(),
        6,
        "five actual primary requests and one summary"
    );
    assert_eq!(
        requests[4]["body"], requests[5]["body"],
        "transient retry retains the post-compaction frozen input"
    );
    assert!(
        !requests[3]["body"]
            .to_string()
            .contains("seventeenth-image"),
        "summary selection did not receive the retired continuation anchor"
    );
    // Count immutable accepted output, not the bounded checkpoint head.
    let canonical = driver.canonical_history();
    assert_eq!(
        canonical
            .iter()
            .filter(|m| matches!(m, MessageBlock::Assistant(_)))
            .count(),
        3
    );
    assert_eq!(
        canonical
            .iter()
            .filter(|m| matches!(m, MessageBlock::Tool(_)))
            .count(),
        17
    );
    let history = driver.request_history();
    let snapshots = history.page(None, 32).unwrap().snapshots;
    assert_eq!(
        snapshots.len(),
        5,
        "summary creates no actual primary request snapshot"
    );
    assert!(
        snapshots[1].continuation.is_some(),
        "unchanged coverage remains active"
    );
    assert!(
        snapshots[2..].iter().all(|s| s.continuation.is_none()),
        "reconciliation and Summary retire continuation ownership"
    );
    assert_ne!(
        snapshots[2].semantic_input_fingerprint,
        snapshots[3].semantic_input_fingerprint
    );
    assert_eq!(
        snapshots[3].semantic_input_fingerprint,
        snapshots[4].semantic_input_fingerprint
    );
    assert_ne!(snapshots[3].request_id, snapshots[4].request_id);
    assert_ne!(
        snapshots[3].provisional_message_id,
        snapshots[4].provisional_message_id
    );
    assert_eq!(
        history.reconstruct(&snapshots[3].identity).unwrap(),
        history.reconstruct(&snapshots[4].identity).unwrap()
    );
    let introduced: Vec<_> = snapshots
        .iter()
        .flat_map(|s| &s.request_context_ids)
        .collect();
    let unique: std::collections::BTreeSet<_> = introduced.iter().copied().collect();
    assert!(
        !unique.is_empty(),
        "native Agent Context participates in this flow"
    );
    assert_eq!(introduced.len(), unique.len(), "Context is introduced once");
    for snapshot in &snapshots {
        let reconstructed = history.reconstruct(&snapshot.identity).unwrap();
        for id in &unique {
            assert!(
                reconstructed
                    .messages
                    .iter()
                    .filter(|m| m.canonical_id() == Some(*id))
                    .count()
                    <= 1,
                "Context is never duplicated through Tail and staged input"
            );
        }
    }
    assert!(
        snapshots[3..]
            .iter()
            .all(|s| s.request_context_ids.is_empty())
    );
    emulator.finish().await;
}

#[tokio::test]
async fn invalidated_stored_reasoning_fails_compatibility_before_any_new_request_start() {
    let Some((driver, emulator)) =
        image_coverage_driver("image_coverage_missing_replay", "emulator/responses-image").await
    else {
        return;
    };
    driver.submit("Read sixteen images, then one more.");
    let outcome = driver.settle().await.1;
    let RuntimeClientOutcome::Failed { error, .. } = outcome else {
        panic!("missing native replay must fail")
    };
    assert!(
        format!("{error:?}").contains("native model input compatibility"),
        "{error:?}"
    );
    assert_eq!(
        emulator.requests().await.len(),
        2,
        "failed preflight dispatches no third request"
    );
    assert_eq!(
        driver
            .request_history()
            .page(None, 32)
            .unwrap()
            .snapshots
            .len(),
        2,
        "failed preflight commits no third request start"
    );
    let canonical = driver.canonical_history();
    assert_eq!(
        canonical
            .iter()
            .filter(|m| matches!(m, MessageBlock::Tool(_)))
            .count(),
        17
    );
    assert_eq!(
        canonical
            .iter()
            .filter(|m| matches!(m, MessageBlock::Assistant(_)))
            .count(),
        2,
        "compatibility failure invents no Assistant output"
    );
    emulator.finish().await;
}

#[tokio::test]
async fn seventeen_parallel_image_reads_leave_a_valid_provider_continuation() {
    image_budget_continuation(
        "image_budget_continuation",
        &format!("emulator-anthropic/{ANTHROPIC_MODEL}"),
        None,
    )
    .await;
}

#[tokio::test]
async fn chat_parallel_images_switch_and_reconstruct() {
    image_budget_continuation(
        "image_budget_chat",
        "emulator/chat-image",
        Some("emulator/chat-model"),
    )
    .await;
}

#[tokio::test]
async fn responses_parallel_images_switch_and_reconstruct() {
    image_budget_continuation(
        "image_budget_responses",
        "emulator/responses-image",
        Some("emulator/responses-model"),
    )
    .await;
}

async fn image_budget_continuation(scenario: &str, image_model: &str, text_model: Option<&str>) {
    use base64::Engine;
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut bytes = Vec::new();
    png::Encoder::new(&mut bytes, 1, 1)
        .write_header()
        .unwrap()
        .write_image_data(&[128])
        .unwrap();
    std::fs::write(workspace.join("sample.png"), &bytes).unwrap();
    let Some(emulator) = ProviderEmulator::start_with_workspace(scenario, Some(&workspace)).await
    else {
        return;
    };
    let mut setup = Setup::new(image_model);
    setup.images = true;
    let driver = Driver::start_in(root, &emulator, &setup).await;
    driver.submit("Read the same PNG seventeen times in parallel.");
    emulator.await_gate("image-budget-admitted").await;
    if let Some(text_model) = text_model {
        assert!(
            driver
                .model_set(SessionModelConfig::of(ModelRef::parse(text_model).unwrap()))
                .await
                .is_ok(),
            "live selection commits while the active Attempt catalog remains frozen"
        );
    }
    emulator.release_gate("image-budget-admitted").await;
    let (_, outcome) = driver.settle().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 2);
    let body = body_text(&requests[1]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    assert_eq!(body.matches(&encoded).count(), 16);
    assert!(body.contains("Image artifact"));
    if let Some(text_model) = text_model {
        let body = &requests[1]["body"];
        let (items, tool_tag, id_key) = if text_model.contains("responses") {
            (
                body["input"].as_array().unwrap(),
                "function_call_output",
                "call_id",
            )
        } else {
            (body["messages"].as_array().unwrap(), "tool", "tool_call_id")
        };
        // Stored Responses sends only the tail after its continuation boundary.
        // Chat retains the initial User plus request-only runtime context.
        let expected_users = if body.get("previous_response_id").is_some() {
            0
        } else {
            requests[0]["body"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["role"] == "user")
                .count()
        };
        assert_eq!(
            items.iter().filter(|item| item["role"] == "user").count(),
            expected_users,
            "image results add no synthetic User turn"
        );
        assert!(
            items
                .iter()
                .filter(|item| item["role"] == "user")
                .all(|item| !item.to_string().contains(&encoded)),
            "Tool images never move to User content"
        );
        for i in 0..17 {
            let tool = items
                .iter()
                .find(|item| {
                    item[id_key] == format!("parallel-image-{i}")
                        && (item["role"] == tool_tag || item["type"] == tool_tag)
                })
                .expect("each real native result retains its call identity");
            assert!(
                tool.to_string().contains(&encoded) || tool.to_string().contains("Image artifact")
            );
        }
    }

    let canonical = driver.canonical_history();
    let mut ids = std::collections::BTreeSet::new();
    for message in &canonical {
        if let rustx::message::types::MessageBlock::Tool(tool) = message {
            for block in &tool.result.content {
                if let rustx::tools::types::ToolResultContent::Image(image) = block {
                    ids.insert(image.artifact_id.clone());
                }
            }
        }
    }
    assert_eq!(
        ids.len(),
        17,
        "request projection never edits successful canonical results"
    );
    // Remove the original filesystem source before either later model request.
    std::fs::write(workspace.join("sample.png"), b"changed").unwrap();
    std::fs::remove_file(workspace.join("sample.png")).unwrap();
    if let Some(text_model) = text_model {
        driver
            .model_set(SessionModelConfig::of(ModelRef::parse(text_model).unwrap()))
            .await
            .unwrap();
        driver.submit("continue as text");
        let outcome = driver.settle().await.1;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{outcome:?}"
        );
        driver
            .model_set(SessionModelConfig::of(
                ModelRef::parse(image_model).unwrap(),
            ))
            .await
            .unwrap();
        driver.submit("restore images");
        let outcome = driver.settle().await.1;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{outcome:?}"
        );
        let requests = emulator.requests().await;
        assert_eq!(requests.len(), 4);
        assert!(!body_text(&requests[2]).contains(&encoded));
        assert_eq!(body_text(&requests[3]).matches(&encoded).count(), 16);
        // Original successful canonical Tool results remain immutable.
        let after = driver.canonical_history();
        for message in &canonical {
            assert!(after.contains(message));
        }
    }
    await_history_len(&driver, if text_model.is_some() { 4 } else { 2 }).await;
    driver.shutdown().await.unwrap();
    let reopened = rustx::durable::SqliteConversationStore::open_existing(
        driver.access.node.conversation_id.clone(),
        &driver.access.database_path,
    )
    .unwrap();
    let history = rustx::runtime::request_history::RequestHistory::new(Arc::new(reopened));
    let snapshots = crate::common::request_snapshots(&history);
    let reconstructed = history
        .reconstruct(&snapshots.last().unwrap().identity)
        .unwrap();
    let reconstructed_json = serde_json::to_string(&reconstructed.messages).unwrap();
    assert_eq!(reconstructed_json.matches("\"type\":\"image\"").count(), 16);
    assert!(reconstructed_json.contains("Image artifact"));
    assert!(
        !reconstructed.images.is_empty(),
        "exact frozen request retains the selected image bytes"
    );
    assert_eq!(reconstructed.images, snapshots.last().unwrap().images);
    assert!(
        snapshots
            .last()
            .unwrap()
            .input
            .iter()
            .any(|item| matches!(item, rustx::model::projected::FrozenModelInput::Derived(_)))
    );

    let protocol = reconstructed.protocol();
    let server = crate::common::FixtureServer::start(move |_, _| match protocol {
        rustx::model::ModelProtocol::OpenAiChatCompletions => {
            crate::common::sse_fixture("openai_chat", "plain_text.sse")
        }
        rustx::model::ModelProtocol::OpenAiResponses => {
            crate::common::sse_fixture("openai_responses", "plain_text.sse")
        }
        rustx::model::ModelProtocol::AnthropicMessages => {
            crate::common::sse_fixture("anthropic", "text.sse")
        }
    })
    .await;
    let adapter: Box<dyn rustx::model::ModelAdapter> = match protocol {
        rustx::model::ModelProtocol::OpenAiChatCompletions => {
            Box::new(rustx::model::OpenAiChatCompletionsAdapter::new(
                rustx::model::OpenAiAdapterConfig::new("k", server.url("/v1")),
            ))
        }
        rustx::model::ModelProtocol::OpenAiResponses => {
            Box::new(rustx::model::OpenAiResponsesAdapter::new(
                rustx::model::OpenAiAdapterConfig::new("k", server.url("/v1")),
            ))
        }
        rustx::model::ModelProtocol::AnthropicMessages => {
            Box::new(rustx::model::AnthropicMessagesAdapter::new(
                rustx::model::AnthropicAdapterConfig::new("k", server.url("")),
            ))
        }
    };
    let events = crate::common::collect_events(adapter.as_ref(), reconstructed).await;
    assert!(
        matches!(
            events.last(),
            Some(rustx::model::ModelEvent::Completed { .. })
        ),
        "{events:?}"
    );
    let replay: serde_json::Value = serde_json::from_str(&server.request_body(0)).unwrap();
    assert_eq!(
        replay,
        emulator.requests().await.last().unwrap()["body"],
        "SQLite reconstruction must produce the same provider request"
    );
    drop(driver);
    emulator.finish().await;
}

#[cfg(unix)]
mod runtime_benchmark;
