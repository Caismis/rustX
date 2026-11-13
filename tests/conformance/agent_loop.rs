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
const CHUNKED_PROMPT: &str = "conformance: write the chunked compaction history";
const CHUNKED_SUMMARY: &str = "chunked compaction kept the user's goal";
const PRIMARY_REASONING: &str = "private-primary-reasoning-522";
const SUMMARY_REASONING: &str = "private-summary-reasoning-522";

/// The compaction window and reserve.
/// The emulator's scripted turn-one reply is about 200 KB; this context
/// window exercises compaction against the provider fixture.
const COMPACTION_CONTEXT_WINDOW: u64 = 56_000;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SummaryScenario {
    Standard,
    Chunked,
    CrossProvider,
}

/// How a driver's catalog and session are shaped.
struct Setup {
    /// The model the session selects.
    model: String,
    images: bool,
    /// The declared context window of every catalog model.
    context_window: u64,
    /// The explicit summary-model policy, when the session declares one.
    auxiliary: Option<String>,
    /// The Summary model and profile arrangement exercised by this fixture.
    summary_scenario: SummaryScenario,
    /// Explicit Skill resources for this isolated provider fixture.
    skill_paths: Vec<PathBuf>,
    responses_stateless: bool,
}

impl Setup {
    fn new(model: &str) -> Self {
        Self {
            model: model.to_owned(),
            images: false,
            context_window: 128_000,
            auxiliary: None,
            summary_scenario: SummaryScenario::Standard,
            skill_paths: Vec::new(),
            responses_stateless: false,
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
            std::fs::write(
                workspace.join("rustx.toml"),
                "approval_mode = \"full_access\"\n[agent]\nworkflows = [\"image_review\"]\n",
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
                thread_id: session.active_conversation_id.clone(),
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
                    ending @ (NotificationMethod::Closed { .. }
                    | NotificationMethod::ResyncRequired { .. }) => {
                        eprintln!("conformance attachment stopped: {ending:?}");
                        break;
                    }
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
                recover_unavailable_auxiliary: None,
                target: self.target.clone(),
                config: Box::new(config),
            },
        )
        .await
    }

    async fn cancel_current_attempt(&self) -> Result<MethodResult, RpcError> {
        let MethodResult::Checkpoint { checkpoint } = rpc(
            &self.connection,
            Method::SessionCheckpoint {
                target: self.target.clone(),
                trace_records: vec![],
            },
        )
        .await?
        else {
            panic!("checkpoint")
        };
        let turn_id = checkpoint
            .snapshot
            .attempt
            .expect("active Attempt")
            .attempt_id
            .expect("exact Attempt");
        rpc(
            &self.connection,
            Method::TurnCancel {
                turn_id,
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
                instructions: None,
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
                Ok(other) => panic!(
                    "the observation stream ended early: {other:?}; native state: {:?}",
                    self.snapshot().map(|(snapshot, cursor)| (
                        cursor,
                        snapshot
                            .attempt
                            .map(|attempt| (attempt.attempt_id, attempt.phase))
                    ))
                ),
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
        if setup.summary_scenario == SummaryScenario::Chunked && id == SUMMARY_MODEL {
            model["context_window"] = 16_000.into();
            model["max_output_tokens"] = 4_096.into();
            model["capabilities"]["json_schema_output"] = true.into();
            model["default_profile"] = "thinking".into();
            model["profiles"] = serde_json::json!({
                "thinking": {"request_params": "{\"reasoning_effort\":\"medium\"}"}
            });
        }
        models.insert(format!("emulator/{id}"), model);
    }
    for (reference_provider, provider, id, protocol) in [
        (
            "emulator",
            "emulator-responses",
            RESPONSES_MODEL,
            "openai_responses",
        ),
        (
            "emulator-anthropic",
            "emulator-anthropic",
            ANTHROPIC_MODEL,
            "anthropic_messages",
        ),
    ] {
        let mut model = serde_json::json!({
            "provider": provider, "id": id,
            "context_window": window, "max_output_tokens": 1024,
            "capabilities": if setup.images && protocol == "anthropic_messages" { serde_json::json!({"input_modalities": ["text", "image"], "output_modalities": ["text"]}) } else { text_capabilities() },
        });
        if setup.summary_scenario == SummaryScenario::CrossProvider
            && protocol == "anthropic_messages"
        {
            model["context_window"] = 128_000.into();
            model["default_profile"] = "ordinary".into();
            model["profiles"] = serde_json::json!({
                "ordinary": {"request_params": "{\"temperature\":0.7}"},
                "precise": {"request_params": "{\"temperature\":0.2}"}
            });
        }
        models.insert(format!("{reference_provider}/{id}"), model);
    }
    if setup.responses_stateless {
        models
            .get_mut(&format!("emulator/{RESPONSES_MODEL}"))
            .unwrap()["compat"] = serde_json::json!({"responses_storage":"stateless"});
    }
    if setup.images {
        for (id, protocol) in [
            ("chat-image", "openai_chat_completions"),
            ("responses-image", "openai_responses"),
        ] {
            models.insert(format!("emulator/{id}"), serde_json::json!({
                "provider": if protocol == "openai_responses" { "emulator-responses" } else { "emulator" }, "id":id,
                "compat": if protocol == "openai_chat_completions" { serde_json::json!({"chat_reasoning_replay":"omit"}) } else { serde_json::json!({}) },
                "context_window":window, "max_output_tokens":1024,
                "capabilities":{"input_modalities":["text","image"],"output_modalities":["text"]}
            }));
        }
    }
    toml::to_string_pretty(&serde_json::json!({
        "providers": {
            "emulator": { "protocol": "openai_chat_completions", "base_url": emulator.openai_base_url(), "api_key": format!("${CREDENTIAL_VARIABLE}") },
            "emulator-responses": { "protocol": "openai_responses", "base_url": emulator.openai_base_url(), "api_key": format!("${CREDENTIAL_VARIABLE}") },
            "emulator-anthropic": { "protocol": "anthropic_messages", "base_url": emulator.base_url(), "api_key": format!("${CREDENTIAL_VARIABLE}") },
        },
        "models": models,
    })).unwrap()
}

fn chat_model(id: &str, window: u64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
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
    if let Some(summary) = &setup.auxiliary {
        model["auxiliary"] = serde_json::json!({"mode": "explicit", "model": summary});
        if setup.summary_scenario == SummaryScenario::CrossProvider {
            model["auxiliary"]["profile"] =
                serde_json::json!({"mode": "explicit", "id": "precise"});
        }
    }
    let document = serde_json::json!({
        "approval_mode": "full_access",
        "agent_id": "agent-issue47",
        "agent": {"model": model, "skills": "all", "tools": {"builtin": if setup.images { vec!["read", "write", "edit", "glob", "grep", "bash", "read_image"] } else { vec!["read", "write", "edit", "glob", "grep", "bash"] }}, "plugins": {"agent_status": {"enabled": true}, "todo": {"enabled": true}}},
    });
    toml::to_string_pretty(&document).unwrap()
}

/// The provider-request bodies the emulator recorded, as raw JSON text.
fn body_text(request: &serde_json::Value) -> String {
    serde_json::to_string(&request["body"]).expect("serialize the recorded body")
}

// ---------------------------------------------------------------------------
// Scenario A — a normal streamed turn, on every supported protocol
// ---------------------------------------------------------------------------

/// Changing protocols affects request serialization, never accepted history.
/// Exercise the reported Responses -> Chat Completions -> Responses sequence
/// through `ModelSet`, durable admission, preflight, tools and real HTTP/SSE.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn responses_chat_responses_switch_preserves_history_and_tool_results() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join(NOTE_PATH), NOTE_MARKER).unwrap();
    let Some(emulator) =
        ProviderEmulator::start_with_workspace("responses_chat_responses_switch", Some(&workspace))
            .await
    else {
        return;
    };
    let responses = format!("emulator/{RESPONSES_MODEL}");
    let driver = Driver::start_in(root, &emulator, &Setup::new(&responses)).await;

    for (model, prompt) in [
        (responses.as_str(), TURN_ONE),
        ("emulator/chat-model", TURN_TWO),
        (responses.as_str(), TURN_THREE),
    ] {
        let previous_history = driver.canonical_history();
        driver
            .model_set(SessionModelConfig::of(ModelRef::parse(model).unwrap()))
            .await
            .expect("select protocol for the next attempt");
        driver.submit(prompt);
        let (_, outcome) = driver.settle().await;
        assert!(
            matches!(outcome, RuntimeClientOutcome::Completed { .. }),
            "{model}: {outcome:?}"
        );
        assert!(
            driver.canonical_history().starts_with(&previous_history),
            "switching does not rewrite accepted reasoning or tool identities"
        );
    }

    let requests = emulator.requests().await;
    assert_eq!(
        requests.len(),
        4,
        "three user turns and one real tool continuation"
    );
    let input = requests[3]["body"]["input"].as_array().unwrap();
    let mut text = Vec::new();
    for item in input {
        if item["type"] == "message" && item["role"] == "assistant" {
            for part in item["content"].as_array().unwrap() {
                assert_eq!(part["type"], "output_text");
                text.push(part["text"].as_str().unwrap());
            }
        }
    }
    assert_eq!(
        text,
        [
            "Responses plan",
            "Responses answer",
            "Chat tool plan",
            "Chat final plan",
            "Chat answer"
        ]
    );
    assert!(
        !input.iter().any(|item| item["type"] == "reasoning"),
        "readable thoughts do not become fabricated native reasoning items"
    );
    assert_eq!(
        input
            .iter()
            .filter(|item| item["type"] == "function_call" && item["call_id"] == "call-switch-read")
            .count(),
        1
    );
    assert_eq!(
        input
            .iter()
            .filter(|item| item["type"] == "function_call_output"
                && item["call_id"] == "call-switch-read")
            .count(),
        1
    );
    assert_eq!(
        driver.committed_assistant_text(),
        "Responses answerChat answerBack on Responses"
    );
    emulator.finish().await;
}

/// Every directed protocol switch goes through real admission and tool execution.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_protocol_switch_keeps_native_owners_history_and_tools() {
    use rustx::message::types::AssistantContentBlock;
    use rustx::model::ModelProtocol;
    for stateless in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join(NOTE_PATH), NOTE_MARKER).unwrap();
        let Some(emulator) = ProviderEmulator::start_with_workspace(
            if stateless {
                "all_protocol_switches_stateless"
            } else {
                "all_protocol_switches"
            },
            Some(&workspace),
        )
        .await
        else {
            return;
        };
        let models = [
            ("emulator/responses-model", ModelProtocol::OpenAiResponses),
            ("emulator/chat-model", ModelProtocol::OpenAiChatCompletions),
            (
                "emulator-anthropic/anthropic-model",
                ModelProtocol::AnthropicMessages,
            ),
            ("emulator/responses-model", ModelProtocol::OpenAiResponses),
            (
                "emulator-anthropic/anthropic-model",
                ModelProtocol::AnthropicMessages,
            ),
            ("emulator/chat-model", ModelProtocol::OpenAiChatCompletions),
            ("emulator/responses-model", ModelProtocol::OpenAiResponses),
        ];
        let mut setup = Setup::new(models[0].0);
        setup.responses_stateless = stateless;
        let driver = Driver::start_in(root, &emulator, &setup).await;
        for (index, (model, protocol)) in models.into_iter().enumerate() {
            let before = driver.canonical_history();
            driver
                .model_set(SessionModelConfig::of(ModelRef::parse(model).unwrap()))
                .await
                .unwrap();
            driver.submit(&format!("switch prompt {index}"));
            let (_, outcome) = driver.settle().await;
            assert!(
                matches!(outcome, RuntimeClientOutcome::Completed { .. }),
                "{model}: {outcome:?}"
            );
            let after = driver.canonical_history();
            assert!(after.starts_with(&before));
            let owned = after[before.len()..]
                .iter()
                .filter_map(|message| match message {
                    MessageBlock::Assistant(a) => Some(a),
                    _ => None,
                })
                .flat_map(|a| &a.content)
                .filter_map(|block| match block {
                    AssistantContentBlock::Reasoning(r) => r.origin.as_ref(),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert!(!owned.is_empty());
            assert!(
                owned
                    .iter()
                    .all(|origin| origin.protocol == protocol && origin.model == model_id(model))
            );
        }
        let requests = emulator.requests().await;
        assert_eq!(requests.len(), 14);
        let history = driver.request_history();
        let snapshots = history.page(None, 32).unwrap().snapshots;
        assert_eq!(snapshots.len(), 14);
        for pair in snapshots.as_chunks::<2>().0 {
            assert!(
                pair[0].continuation.is_none(),
                "a changed Session model starts from full history"
            );
            let first = history.reconstruct(&pair[0].identity).unwrap();
            let next = history.reconstruct(&pair[1].identity).unwrap();
            assert_eq!(
                first.invocation.replay_origin(),
                next.invocation.replay_origin()
            );
            assert_eq!(
                pair[1].continuation.is_some(),
                first.protocol() != ModelProtocol::OpenAiChatCompletions
            );
        }
        assert_eq!(
            driver
                .canonical_history()
                .iter()
                .filter(|m| matches!(m, MessageBlock::Tool(_)))
                .count(),
            7
        );
        emulator.finish().await;
    }
}

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

/// Built-in guidance and global and workspace AGENTS.md cross
/// every provider boundary in the same order and survive frozen reconstruction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn default_system_prompt_layers_project_instructions_on_every_protocol() {
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
        std::fs::write(workspace.join("extra.md"), "Supplemental guidance").unwrap();
        let driver = Driver::start_in(root, &emulator, &Setup::new(&model)).await;
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
        let project_instructions = format!(
            "\n\nProject-specific instructions and guidelines:\n\n<project_instructions path=\"{}\">\n{global}\n</project_instructions>\n\n<project_instructions path=\"{}\">\n{project}\n</project_instructions>",
            global_directory
                .canonicalize()
                .unwrap()
                .join("AGENTS.md")
                .display(),
            workspace
                .canonicalize()
                .unwrap()
                .join("AGENTS.md")
                .display()
        );
        assert!(prompt.contains(&project_instructions));
        // PermissionReminder is the last system-section lane when eligible, so
        // the prompt may continue past project instructions on first request.
        let after_project = prompt.split(&project_instructions).nth(1).unwrap_or("");
        assert!(
            after_project.is_empty() || after_project.starts_with("\n\nExecution permission mode:"),
            "unexpected system prompt tail after project instructions: {after_project:?}"
        );
        let snapshots = driver.request_history().page(None, 32).unwrap().snapshots;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].effective_system_prompt, prompt);
        std::fs::write(global_directory.join("AGENTS.md"), "Later global edit").unwrap();
        std::fs::write(workspace.join("AGENTS.md"), "Later workspace edit").unwrap();
        let reconstructed = driver.reconstruct_request(&snapshots[0].identity).unwrap();
        assert_eq!(reconstructed.effective_system_prompt, prompt);
        assert!(driver.canonical_history().iter().all(|message| {
            let json = serde_json::to_string(message).unwrap();
            ![global, project].iter().any(|text| json.contains(text))
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
        first.contains(&published),
        "the provider receives the absolute path of the Skill instruction file"
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
    for (scenario, auxiliary, expected_auxiliary) in [
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
            auxiliary,
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
            serde_json::json!(expected_auxiliary),
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

/// A selected Anthropic Profile supplies a locally validated Prompt-schema
/// summary for an `OpenAI` primary request through both real provider adapters.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compaction_uses_an_explicit_cross_provider_summary_profile() {
    use rustx::events::types::RuntimeEvent as E;
    use rustx::model::auxiliary::AuxiliaryInputEvidence;
    use rustx::model::session::AuxiliaryModelView;
    use rustx::model::structured::StructuredOutputStrategy;

    let Some(emulator) = ProviderEmulator::start("compaction_cross_provider_summary").await else {
        return;
    };
    let setup = Setup {
        context_window: COMPACTION_CONTEXT_WINDOW,
        auxiliary: Some(format!("emulator-anthropic/{ANTHROPIC_MODEL}")),
        summary_scenario: SummaryScenario::CrossProvider,
        ..Setup::new(&format!("emulator/{CHAT_MODEL}"))
    };
    let driver = Driver::start(&emulator, &setup).await;
    driver.submit(TURN_ONE);
    let (_, first) = driver.settle().await;
    assert!(
        matches!(first, RuntimeClientOutcome::Completed { .. }),
        "{first:?}"
    );
    driver.submit(TURN_TWO);
    let (_, second) = driver.settle().await;
    assert!(
        matches!(second, RuntimeClientOutcome::Completed { .. }),
        "{second:?}"
    );

    let (snapshot, _) = driver.snapshot().unwrap();
    assert_eq!(snapshot.context.compaction_count, 1);
    let frozen = snapshot.attempt.as_ref().unwrap().model.as_ref().unwrap();
    assert_eq!(
        frozen.primary.model.to_string(),
        format!("emulator/{CHAT_MODEL}")
    );
    let AuxiliaryModelView::Explicit { effective: summary } = &frozen.summary else {
        panic!("the attempt must retain its explicit Summary binding")
    };
    assert_eq!(
        summary.model.to_string(),
        format!("emulator-anthropic/{ANTHROPIC_MODEL}")
    );
    assert_eq!(
        summary.protocol,
        rustx::model::ModelProtocol::AnthropicMessages
    );
    assert_eq!(summary.profile.as_ref().unwrap().as_str(), "precise");
    assert_eq!(
        summary.request_params["temperature"],
        serde_json::json!(0.2)
    );

    let requests = emulator.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["protocol"], "openai_chat_completions");
    assert_eq!(requests[1]["protocol"], "anthropic_messages");
    assert_eq!(requests[1]["model"], ANTHROPIC_MODEL);
    assert_eq!(requests[1]["body"]["temperature"], serde_json::json!(0.2));
    assert!(requests[1]["body"].get("output_config").is_none());
    assert!(body_text(&requests[1]).contains(COMPACTION_MARKER));
    assert_eq!(requests[2]["protocol"], "openai_chat_completions");
    assert!(body_text(&requests[2]).contains(SUMMARY_TEXT));
    assert!(!body_text(&requests[2]).contains(COMPACTION_MARKER));

    let journal = driver.store.read_events(None, 256).unwrap().events;
    assert_eq!(
        journal
            .iter()
            .filter(|e| matches!(e.event, E::CompactionCompleted { .. }))
            .count(),
        1
    );
    assert!(journal.iter().any(|e| matches!(
        &e.event,
        E::AuxiliaryCallStarted {
            input: AuxiliaryInputEvidence::Compaction {
                output_strategy: StructuredOutputStrategy::Prompt,
                ..
            },
            ..
        }
    )));
    assert!(snapshot.context.projection_head.is_some());
    emulator.finish().await;
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
        auxiliary: Some(format!("emulator/{SUMMARY_MODEL}")),
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

/// A smaller real summary model processes more than twelve normal fragments
/// after an explicit native-format rejection. The operation keeps the admitted
/// model/Profile, accounts for every physical request and activates once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn chunked_compaction_falls_back_once_across_real_provider_requests() {
    use rustx::context::tokens::{DefaultTokenEstimator, TokenEstimator};
    use rustx::durable::AuxiliaryStore;
    use rustx::events::types::RuntimeEvent as E;
    use rustx::model::ModelRequestOwner;
    use rustx::model::auxiliary::{
        AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryInputEvidence,
        AuxiliaryRequestTerminal, AuxiliaryUsageCompleteness,
    };
    use rustx::model::projected::ProjectedMessage;
    use rustx::model::structured::{
        PreparedResponseSchema, ResponseSchema, StructuredOutputStrategy,
    };
    use sha2::{Digest, Sha256};
    use std::collections::BTreeSet;
    use std::fmt::Write as _;

    let Some(emulator) = ProviderEmulator::start("chunked_compaction").await else {
        return;
    };
    let setup = Setup {
        context_window: 256_000,
        auxiliary: Some(format!("emulator/{SUMMARY_MODEL}")),
        summary_scenario: SummaryScenario::Chunked,
        ..Setup::new(&format!("emulator/{CHAT_MODEL}"))
    };
    let driver = Driver::start(&emulator, &setup).await;
    // Make the first source itself larger than the Summary allowance. This
    // forces exact prefix pricing rather than leaving whole-message slack
    // that could hide the added Prompt schema cost after the Native failure.
    let mut source_padding = String::with_capacity(782 * 64);
    for index in 0..782 {
        write!(
            source_padding,
            "{:x}",
            Sha256::digest(format!("chunked-source:{index}").as_bytes())
        )
        .expect("fixture string formatting");
    }
    let prompt = format!(
        "{CHUNKED_PROMPT}\nHistorical fixture payload:\n{}",
        &source_padding[..50_000]
    );
    driver.submit(&prompt);

    // This barrier is reached on the Prompt retry after the Native failure.
    // A Session update now cannot alter any remaining call in this operation.
    tokio::select! {
        _ = emulator.await_gate("first-prompt-summary") => {}
        (_, early_outcome) = driver.settle_observing(|event| {
            if let RuntimeClientEvent::InteractionPending { interaction } = event {
                panic!("unexpected interaction while waiting for Summary: {interaction:?}");
            }
        }) => {
            let requests = emulator.requests().await;
            panic!(
                "Attempt settled before the first Prompt summary: {early_outcome:?}; \
                 provider requests: {}, last model: {:?}; provider diagnostics: {}",
                requests.len(), requests.last().map(|r| &r["model"]), emulator.diagnostics(),
            );
        }
    }
    driver
        .model_set(SessionModelConfig::of(
            ModelRef::parse(&format!("emulator/{SECOND_MODEL}")).unwrap(),
        ))
        .await
        .expect("a later Session model selection commits");
    emulator.release_gate("first-prompt-summary").await;
    let (_, outcome) = driver.settle().await;
    assert!(
        matches!(outcome, RuntimeClientOutcome::Completed { .. }),
        "{outcome:?}"
    );

    let requests = emulator.requests().await;
    let primary: Vec<_> = requests
        .iter()
        .filter(|r| r["model"] == CHAT_MODEL)
        .collect();
    let summary: Vec<_> = requests
        .iter()
        .filter(|r| r["model"] == SUMMARY_MODEL)
        .collect();
    assert!(primary.len() >= 15 && primary.len() <= 64);
    assert!(
        summary.len() > 13,
        "one rejected Native and more than twelve accepted calls"
    );
    assert_eq!(requests.len(), primary.len() + summary.len());
    assert_eq!(summary[0]["body"]["response_format"]["type"], "json_schema");
    for request in &summary {
        assert_eq!(request["body"]["reasoning_effort"], "medium");
        assert!(!body_text(request).contains(PRIMARY_REASONING));
        assert!(!body_text(request).contains(SUMMARY_REASONING));
    }
    for request in &summary[1..] {
        assert!(
            request["body"].get("response_format").is_none(),
            "fallback is retained"
        );
    }
    let frozen_schema = ResponseSchema::new(
        summary[0]["body"]["response_format"]["json_schema"]["name"]
            .as_str()
            .unwrap()
            .to_owned(),
        summary[0]["body"]["response_format"]["json_schema"]["schema"].clone(),
        64 * 1024,
    )
    .unwrap();
    let prompt_instruction = frozen_schema.prompt_instruction();
    // Reconstruct the provider-neutral Summary input from the observed wire
    // body and use the production estimator over the complete frozen request.
    let text_parts = |content: &serde_json::Value| -> Vec<String> {
        match content {
            serde_json::Value::String(text) => vec![text.clone()],
            serde_json::Value::Array(parts) => parts
                .iter()
                .map(|part| {
                    assert_eq!(
                        part["type"], "text",
                        "Summary wire input must contain only text"
                    );
                    part["text"]
                        .as_str()
                        .expect("typed text content")
                        .to_owned()
                })
                .collect(),
            _ => panic!("unexpected Summary wire content: {content}"),
        }
    };
    let estimate = |body: &serde_json::Value, force_prompt: bool| {
        let mut request = crate::common::simple_request(
            rustx::model::ModelProtocol::OpenAiChatCompletions,
            SUMMARY_MODEL,
            "",
        );
        let messages = body["messages"].as_array().unwrap();
        let users: Vec<_> = messages
            .iter()
            .filter(|message| message["role"] == "user")
            .collect();
        let systems: Vec<_> = messages
            .iter()
            .filter(|message| message["role"] == "system")
            .collect();
        assert_eq!(users.len(), 1, "one exact Summary user envelope");
        assert!(systems.len() <= 1, "one effective system prompt");
        assert_eq!(
            messages.len(),
            users.len() + systems.len(),
            "no omitted Summary wire messages"
        );
        let mut input = rustx::context::summarizer::SummaryRequest::default()
            .model_input()
            .messages;
        let rustx::model::ModelInputMessage::Derived(message) = &mut input[0] else {
            unreachable!()
        };
        let ProjectedMessage::User { content, .. } = &mut message.message else {
            unreachable!()
        };
        *content = text_parts(&users[0]["content"])
            .into_iter()
            .map(|text| UserContentBlock::Text(TextBlock { text }))
            .collect();
        request.messages = input.into();
        request.effective_system_prompt = if force_prompt {
            prompt_instruction.clone()
        } else {
            systems.first().map_or_else(String::new, |message| {
                text_parts(&message["content"]).concat()
            })
        };
        request.response_schema = Some(PreparedResponseSchema {
            schema: frozen_schema.clone(),
            strategy: if !force_prompt && body.get("response_format").is_some() {
                StructuredOutputStrategy::Native
            } else {
                StructuredOutputStrategy::Prompt
            },
        });
        DefaultTokenEstimator.estimate_frozen_request(&request)
    };
    let output = summary[0]["body"]
        .get("max_completion_tokens")
        .or_else(|| summary[0]["body"].get("max_tokens"))
        .unwrap()
        .as_u64()
        .unwrap();
    let input_limit = 16_000 - 800 - output;
    let native_price = estimate(&summary[0]["body"], false);
    let native_as_prompt_price = estimate(&summary[0]["body"], true);
    let first_prompt_price = estimate(&summary[1]["body"], false);
    assert!(
        native_price <= input_limit,
        "Native={native_price}, input_limit={input_limit}"
    );
    assert!(
        native_as_prompt_price > input_limit,
        "the unchanged Native source would overflow after adding the Prompt schema: Native={native_price}, Native as Prompt={native_as_prompt_price}, first Prompt={first_prompt_price}, input_limit={input_limit}, output={output}"
    );
    for request in &summary[1..] {
        let system = request["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "system")
            .unwrap();
        assert_eq!(
            text_parts(&system["content"]).concat(),
            prompt_instruction,
            "the frozen schema is unchanged"
        );
        assert!(
            estimate(&request["body"], false) <= input_limit,
            "every complete Prompt request is independently repriced"
        );
    }
    let final_primary = primary.last().unwrap();
    let final_body = body_text(final_primary);
    assert!(final_body.contains(CHUNKED_SUMMARY));
    assert!(!final_body.contains("chunked-call-001"));
    assert!(!final_body.contains(PRIMARY_REASONING));
    assert!(!final_body.contains(SUMMARY_REASONING));
    let messages = final_primary["body"]["messages"].as_array().unwrap();
    let call_ids: Vec<_> = messages
        .iter()
        .filter_map(|m| m["tool_calls"].as_array())
        .flatten()
        .map(|call| call["id"].as_str().unwrap())
        .collect();
    let result_ids: Vec<_> = messages
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["tool_call_id"].as_str().unwrap())
        .collect();
    let calls: BTreeSet<_> = call_ids.iter().copied().collect();
    let results: BTreeSet<_> = result_ids.iter().copied().collect();
    assert!(!calls.is_empty(), "a recent original tool pair remains");
    assert_eq!(calls.len(), call_ids.len(), "retained call IDs are unique");
    assert_eq!(
        results.len(),
        result_ids.len(),
        "retained result IDs are unique"
    );
    assert_eq!(calls, results, "every retained call has exactly its result");

    let canonical = driver.canonical_history();
    let sizes: Vec<_> = canonical
        .iter()
        .map(|m| serde_json::to_vec(m).unwrap().len())
        .collect();
    assert!(sizes.iter().all(|size| *size < 1024 * 1024));
    assert!(sizes.iter().sum::<usize>() < 16 * 1024 * 1024);
    assert!(canonical.iter().any(|m| {
        serde_json::to_string(m)
            .unwrap()
            .contains("chunked-call-001")
    }));

    // Read the immutable native journal in bounded pages, not the live client
    // projection. These events carry the accepted ranges and real Call IDs.
    let mut journal = Vec::new();
    let mut after = None;
    loop {
        let page = driver.store.read_events(after, 128).unwrap();
        if page.events.is_empty() {
            break;
        }
        after = page.next_sequence;
        journal.extend(page.events);
    }
    let completions: Vec<_> = journal
        .iter()
        .filter_map(|e| match &e.event {
            E::CompactionCompleted { operation_id, .. } => Some(operation_id),
            _ => None,
        })
        .collect();
    assert_eq!(completions.len(), 1);
    let operation = completions[0];
    let accepted: Vec<_> = journal
        .iter()
        .filter_map(|e| match &e.event {
            E::CompactionChunkAccepted {
                operation_id,
                accepted_fragments,
                source_ranges,
                generation,
            } if operation_id == operation => {
                Some((*accepted_fragments, source_ranges, generation))
            }
            _ => None,
        })
        .collect();
    assert!(accepted.len() > 12);
    assert_eq!(accepted.len() + 1, summary.len());
    let starts: Vec<_> = journal
        .iter()
        .filter_map(|e| match &e.event {
            E::AuxiliaryCallStarted {
                call_id,
                business_source: AuxiliaryBusinessSource::Compaction { operation_id, .. },
                ..
            } if operation_id == operation => Some(call_id),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), summary.len());
    assert_eq!(
        starts.iter().copied().collect::<BTreeSet<_>>().len(),
        starts.len()
    );
    let physical_starts = journal
        .iter()
        .filter(|e| {
            matches!(
                &e.event,
                E::ModelRequestStarted {
                    owner: ModelRequestOwner::Auxiliary { .. },
                    ..
                }
            )
        })
        .count();
    assert_eq!(physical_starts, summary.len());
    assert_eq!(
        journal
            .iter()
            .filter(|e| matches!(
                &e.event,
                E::ModelRequestStarted {
                    owner: ModelRequestOwner::Agent { .. },
                    ..
                }
            ))
            .count(),
        primary.len()
    );
    assert!(
        journal
            .iter()
            .filter_map(|e| match &e.event {
                E::ModelRequestCompleted {
                    owner: ModelRequestOwner::Agent { .. },
                    usage,
                    ..
                } => Some(usage),
                _ => None,
            })
            .all(Option::is_none),
        "summary usage never becomes primary usage"
    );

    let cut = driver.store.auxiliary_audit_cut().unwrap();
    let mut last_range: Option<(u32, u32)> = None;
    let mut previous_accepted = None;
    let mut frozen_invocation = None;
    let mut rejected_ranges = None;
    let mut captured_cut = None;
    for (index, call_id) in starts.iter().enumerate() {
        let call = driver
            .store
            .load_auxiliary_call(call_id, cut)
            .unwrap()
            .unwrap();
        let requests = driver
            .store
            .read_auxiliary_call_requests(call_id, cut, None, 2)
            .unwrap();
        assert_eq!(
            requests.len(),
            1,
            "each fragment has one real physical request"
        );
        assert_eq!(requests[0].model, SUMMARY_MODEL);
        if let Some(frozen) = &frozen_invocation {
            assert_eq!(
                &call.invocation_fingerprint, frozen,
                "the Model, Profile and resource revision stay frozen"
            );
        } else {
            frozen_invocation = Some(call.invocation_fingerprint.clone());
        }
        let evidence = requests[0].request_evidence.as_ref().unwrap();
        let AuxiliaryBusinessSource::Compaction {
            operation_id,
            preceding_call,
        } = &call.business_source
        else {
            panic!("summary Call has a compaction business owner")
        };
        assert_eq!(operation_id, operation);
        assert_eq!(
            preceding_call.as_ref().map(|r| &r.call_id),
            index.checked_sub(1).map(|i| starts[i])
        );
        let AuxiliaryInputEvidence::Compaction {
            cut,
            source_ranges,
            previous_summary,
            output_strategy,
        } = &call.input
        else {
            panic!("summary Call has native compaction provenance")
        };
        if let Some(captured) = &captured_cut {
            assert_eq!(
                cut, captured,
                "the captured historical source never changes"
            );
        } else {
            captured_cut = Some(cut.clone());
        }
        assert_eq!(
            previous_summary.as_ref().map(|r| &r.call_id),
            previous_accepted
        );
        if index == 0 {
            assert_eq!(*output_strategy, StructuredOutputStrategy::Native);
            assert_eq!(call.outcome, Some(AuxiliaryCallOutcome::Failed));
            assert_eq!(
                requests[0].local_terminal,
                Some(AuxiliaryRequestTerminal::Failed)
            );
            assert_eq!(
                requests[0].normalized_failure_kind,
                Some(rustx::model::ModelErrorKind::NativeFormatUnsupported)
            );
            assert_eq!(
                evidence.observed_provider_terminal,
                Some(
                    rustx::model::auxiliary::AuxiliaryObservedProviderTerminal::Failed {
                        kind: rustx::model::ModelErrorKind::NativeFormatUnsupported,
                    }
                )
            );
            rejected_ranges = Some(source_ranges.clone());
            assert!(previous_summary.is_none());
            assert!(evidence.usage.is_none());
            assert_eq!(
                evidence.usage_completeness,
                AuxiliaryUsageCompleteness::Unreported
            );
            continue;
        }
        assert!(
            matches!(output_strategy, StructuredOutputStrategy::PromptFallback { operation_id } if operation_id == operation)
        );
        if index == 1 {
            let rejected = rejected_ranges.as_ref().unwrap();
            let first = &source_ranges[0];
            assert_eq!(
                (first.projection_index, first.byte_start),
                (rejected[0].projection_index, rejected[0].byte_start),
                "the rejected Native request never advances the source cursor"
            );
            assert_ne!(
                source_ranges, rejected,
                "the complete Prompt request is replanned"
            );
        }
        assert_eq!(call.outcome, Some(AuxiliaryCallOutcome::Completed));
        let (ordinal, accepted_ranges, generation) = accepted[index - 1];
        assert_eq!(ordinal as usize, index);
        assert_eq!(source_ranges, accepted_ranges);
        let generation = generation.as_ref().expect("accepted native generation");
        assert_eq!(&generation.call_id, *call_id);
        assert!(!source_ranges.is_empty());
        for range in source_ranges {
            if let Some((prior_index, prior_end)) = last_range {
                if range.projection_index == prior_index {
                    assert_eq!(range.byte_start, prior_end);
                } else {
                    assert_eq!(range.projection_index, prior_index + 1);
                    assert_eq!(range.byte_start, 0);
                }
            } else {
                assert_eq!((range.projection_index, range.byte_start), (0, 0));
            }
            assert!(range.byte_end >= range.byte_start);
            last_range = Some((range.projection_index, range.byte_end));
        }
        previous_accepted = Some(*call_id);
        assert_eq!(
            evidence.usage_completeness,
            AuxiliaryUsageCompleteness::Partial
        );
        let usage = evidence.usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.total_tokens, None);
        assert_eq!(usage.output_tokens, Some(49));
        assert_eq!(usage.details.as_ref().unwrap().reasoning_tokens, Some(13));
    }
    let (snapshot, _) = driver.snapshot().unwrap();
    let head = snapshot.context.projection_head.as_ref().unwrap();
    assert_eq!(head.activation_id.0, 1, "one installed activation");
    assert_eq!(
        driver
            .store
            .projection_timeline(rustx::context::checkpoint::ProjectionActivationId(0), 64,)
            .unwrap()
            .len(),
        1
    );
    // Independently enumerate canonical source records at the captured cut.
    // Every retired occurrence must be covered through its complete rendering.
    let mut at_cut = Vec::new();
    let mut after = rustx::context::resources::LedgerFrontier(0);
    loop {
        let page = driver
            .store
            .compaction_message_page(
                captured_cut.as_ref().unwrap(),
                after,
                None,
                rustx::context::resources::ResourceLimits::default(),
            )
            .unwrap();
        if page.rows.is_empty() {
            break;
        }
        after = rustx::context::resources::LedgerFrontier(page.rows.last().unwrap().0);
        at_cut.extend(page.rows.into_iter().map(|(_, message)| message));
    }
    let retired_count = at_cut.len() - head.original_items as usize;
    let expected_lengths: Vec<_> = at_cut
        .into_iter()
        .take(retired_count)
        .map(|message| {
            u32::try_from(
                rustx::context::summarizer::SummaryRequest {
                    retired: vec![message.into()],
                    ..Default::default()
                }
                .render_transcript()
                .len(),
            )
            .expect("bounded fixture source rendering")
        })
        .collect();
    let mut endpoints = vec![0; expected_lengths.len()];
    for (_, ranges, _) in &accepted {
        for range in *ranges {
            let index = range.projection_index as usize;
            assert_eq!(range.byte_start, endpoints[index]);
            assert!(range.byte_end <= expected_lengths[index]);
            endpoints[index] = range.byte_end;
        }
    }
    assert_eq!(
        endpoints, expected_lengths,
        "every retired source byte is covered exactly once"
    );
    assert_eq!(snapshot.context.compaction_count, 1);
    let occupancy = snapshot
        .context
        .occupancy
        .as_ref()
        .expect("primary occupancy");
    assert!(occupancy.estimated);
    assert_eq!(occupancy.context_window_tokens, 256_000);
    assert!(
        occupancy.input_tokens > 49,
        "summary output usage is not primary occupancy"
    );
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
    for (scenario, auxiliary, expected_auxiliary) in [
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
            auxiliary,
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
            serde_json::json!(expected_auxiliary),
            "{scenario}: summary #1 used the configured summary model"
        );
        assert_eq!(
            requests[3]["model"],
            serde_json::json!(expected_auxiliary),
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
        auxiliary: AuxiliaryModelPolicy::default(),
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
    // A Busy refusal admits nothing, so retry until the native owner returns.
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            match driver.compact_context().await {
                Err(error)
                    if error
                        .message
                        .contains("requires an idle conversation runtime") =>
                {
                    tokio::task::yield_now().await;
                }
                result => break result.unwrap(),
            }
        }
    })
    .await
    .expect("native idle owners must return after the delegated Attempt settles");
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
        setup.auxiliary = Some("emulator/summary-model".into());
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
    // The selected old prefix must be larger than the deterministic rendered
    // JSON summary, so this overflow recovery can make actual progress while
    // retaining the image call/result groups whose coverage is under test.
    driver.submit(&format!(
        "Read sixteen images, then one more. {}",
        "Retain the image review context and its source. ".repeat(40)
    ));
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
async fn invalidated_stored_reasoning_replays_readable_history_without_stale_coverage() {
    let Some((driver, emulator)) = image_coverage_driver(
        "image_coverage_stored_reasoning",
        "emulator/responses-image",
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
    assert_eq!(requests.len(), 3);
    let body = &requests[2]["body"];
    assert_eq!(
        body.get("previous_response_id"),
        None,
        "changed coverage retires the stored pointer"
    );
    let input = body["input"].as_array().unwrap();
    assert!(!input.iter().any(|item| item["type"] == "reasoning"));
    assert!(input.iter().any(|item| item["role"] == "assistant"
        && item["status"] == "completed"
        && item["content"][0]
            == serde_json::json!({"type":"output_text", "text":"stored response reasoning", "annotations": []})));
    let snapshots = driver.request_history().page(None, 32).unwrap().snapshots;
    assert_eq!(snapshots.len(), 3);
    assert!(snapshots[1].continuation.is_some());
    assert!(snapshots[2].continuation.is_none());
    assert_eq!(
        driver.committed_assistant_text(),
        "stored reasoning history continued"
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
        3,
        "the fresh request commits one final Assistant output"
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
