//! The child side of the FND-06 conformance harness.
//!
//! A child is a real runtime process: it composes the same
//! [`LocalConversationCore`] the `rustx` binary composes — the real
//! `ConversationRuntime`, the real durable `SQLite` authority, the real Tool
//! Plane, the real capability/resource loader over the parent's on-disk lab —
//! and drives it with a scripted provider adapter so every model turn is a
//! fixed sequence of canonical events.
//!
//! A child never exits on its own. Each scenario ends by announcing a note and
//! blocking in a control rendezvous, so the parent always owns the moment of
//! death. Between rendezvous the child can be frozen at any instrumented
//! durable boundary through the [`process_death`] gate the parent armed.
//!
//! [`process_death`]: crate::runtime::process_death

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::events::types::RuntimeEvent;
use crate::local_runtime::composition::{
    HeadlessConversationRuntime, LocalConversationCore, LocalRuntimeDependencies, LocalRuntimePaths,
};
use crate::local_runtime::config::CurrentRuntimeConfig;
use crate::local_runtime::session::SessionPersistentState;
use crate::message::content::TextBlock;
use crate::message::types::{ContentBlockIndex, UserContentBlock};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::types::ModelProtocol;
use crate::runtime::ApprovalDecision;
use crate::runtime::conversation_runtime::ConversationRuntime;
use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
use crate::runtime::interaction::InteractionResponse;
use crate::runtime::observation::{ConversationObservation, PendingObservations};
use crate::runtime::process_death;
use crate::scripted_suites::support::fake::{
    FakeModel, FakeStep, ScriptedCall, model_release, tool_call_events,
};
use crate::scripted_suites::support::model::{
    FixtureModel, ScriptedAdapterFactory, fixture_registry,
};
use crate::tools::types::ToolCallStart;

use super::ROOT_ENV;
use super::harness::{CONVERSATION, MODEL};

// ---------------------------------------------------------------------------
// Scenario names, shared with the parent
// ---------------------------------------------------------------------------

/// One inbound message answered by one plain streaming text turn.
pub(crate) const TEXT_TURN: &str = "text_turn";
/// [`TEXT_TURN`] composed with **no** observation bridge installed, so no
/// client-facing consumer exists at the moment of death.
pub(crate) const TEXT_TURN_NO_CLIENT: &str = "text_turn_no_client";
/// One turn whose whole payload fits under the coalescer's byte threshold, so
/// the publication terminal transaction carries the only frame.
pub(crate) const TERMINAL_ONLY_TURN: &str = "terminal_only_turn";
/// One streaming text turn whose terminal event leaves a tool-call proposal
/// structurally incomplete, so `assembler.finish()` rejects the turn after
/// frames were already released.
pub(crate) const STRUCTURAL_FAILURE: &str = "structural_failure";
/// One inbound message answered by a tool-calling turn, its real native Read
/// execution, and a continuation turn.
pub(crate) const TOOL_TURN: &str = "tool_turn";
/// [`TOOL_TURN`] with native Read requiring approval and a child that approves.
pub(crate) const TOOL_APPROVAL: &str = "tool_approval";
/// [`TOOL_TURN`] with native Read requiring approval and a child that never
/// answers, so the waiter is still pending at process death.
pub(crate) const TOOL_APPROVAL_PENDING: &str = "tool_approval_pending";
/// A settled tool execution, a continuation request in flight, and a second
/// inbound accepted during it.
pub(crate) const TOOL_CONTINUATION_INBOUND: &str = "tool_continuation_inbound";
/// One turn that starts a real detached background Bash execution.
pub(crate) const BACKGROUND_TOOL: &str = "background_tool";
/// One settled turn, an explicit manual compaction with a parked summary
/// request, and one more settled turn afterwards.
pub(crate) const COMPACTION: &str = "compaction";
/// One settled attempt, a rendezvous the parent edits resources in, and a
/// second attempt whose continuation issues a native Read of the edited Skill.
pub(crate) const LIVE_RESOURCE_EDIT: &str = "live_resource_edit";
/// One settled attempt, an explicit runtime reload at a quiescent boundary,
/// and one more attempt afterwards.
pub(crate) const RELOAD: &str = "reload";
/// A reload attempted while an attempt owns the session.
pub(crate) const RELOAD_BUSY: &str = "reload_busy";
/// A reopened conversation that admits one new request.
pub(crate) const COLD_RESUME: &str = "cold_resume";
/// A reopened conversation whose new attempt reads the Skill file again.
pub(crate) const COLD_RESUME_READ: &str = "cold_resume_read";
/// Composition only: the child reports whether the current resources produced
/// a runtime at all.
pub(crate) const COMPOSE_ONLY: &str = "compose_only";
/// Inbound accepted durably with no adoption.
pub(crate) const INBOUND_ONLY: &str = "inbound_only";
/// A streaming turn that is still open while a second inbound is accepted.
pub(crate) const STREAMING_INBOUND: &str = "streaming_inbound";

/// A text payload larger than the coalescer's default byte threshold, so the
/// publication plane provably stages and releases frames *before* the stream
/// terminal — the "staged/released frames" precondition of the P/U/C cases.
const WIDE_TEXT: &str = "FND-06 streamed publication payload. ..........................................................................................................................................................................................................................................";

/// The already-discovered Skill file every resource scenario reads.
pub(crate) const SKILL_FILE: &str = ".agents/skills/alpha/SKILL.md";

// ---------------------------------------------------------------------------
// Control channel
// ---------------------------------------------------------------------------

/// Announces one fact to the owning parent without blocking.
fn note(text: &str) {
    process_death::send_line(&serde_json::json!({"kind": "note", "text": text}).to_string());
}

/// Announces one fact and blocks until the parent releases this child.
///
/// This is a control rendezvous: while it blocks, the child executes nothing,
/// so the parent's `SIGKILL` lands on a provably idle process.
fn rendezvous(text: &str) {
    note(text);
    let line = process_death::recv_line().expect("the parent released this child");
    assert!(
        line.contains("\"go\""),
        "unexpected FND-06 parent command {line}"
    );
}

/// Announces that the scenario finished and never returns.
///
/// The parent owns the exit: this child waits for its `SIGKILL`. If the parent
/// itself disappears the control socket reports end of file, and the child
/// ends rather than becoming an orphan.
fn idle() -> ! {
    note("idle");
    process_death::orphan_watchdog()
}

// ---------------------------------------------------------------------------
// Observation log
// ---------------------------------------------------------------------------

/// One recorded runtime fact of this child.
///
/// Deliberately narrow: a scenario waits only on an attempt terminal or on a
/// published interaction, and every other conformance fact is read from the
/// durable authority by the parent after the kill.
#[derive(Debug, Clone)]
enum Seen {
    Event(Box<RuntimeEvent>),
    InteractionPending,
}

/// The child's deterministic view of what the runtime has already done.
///
/// Waiting on this log is never polling: [`Log::wait_for`] parks on the
/// observation notification and re-reads the recorded prefix, so it advances
/// only when the runtime actually published a new fact.
struct Log {
    seen: Mutex<Vec<Seen>>,
    notify: tokio::sync::Notify,
}

impl Log {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            notify: tokio::sync::Notify::new(),
        })
    }

    fn push(&self, seen: Seen) {
        self.seen.lock().expect("log lock").push(seen);
        self.notify.notify_waiters();
    }

    fn snapshot(&self) -> Vec<Seen> {
        self.seen.lock().expect("log lock").clone()
    }

    /// Blocks until `predicate` holds over the recorded facts.
    async fn wait_for(&self, predicate: impl Fn(&[Seen]) -> bool) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if predicate(&self.snapshot()) {
                return;
            }
            notified.await;
        }
    }

    /// Blocks until `count` attempt terminal facts are recorded.
    async fn wait_settled(&self, count: usize) {
        self.wait_for(|seen| terminal_count(seen) >= count).await;
    }
}

fn terminal_count(seen: &[Seen]) -> usize {
    seen.iter()
        .filter(|entry| {
            matches!(entry, Seen::Event(event) if matches!(
                **event,
                RuntimeEvent::AttemptCompleted { .. }
                    | RuntimeEvent::AttemptCancelled { .. }
                    | RuntimeEvent::AttemptFailed { .. }
                    | RuntimeEvent::AttemptTimedOut { .. }
                    | RuntimeEvent::AttemptLimitExceeded { .. }
            ))
        })
        .count()
}

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

/// The explicit startup paths of one child, derived from the parent's lab.
fn lab_paths(root: &Path) -> LocalRuntimePaths {
    LocalRuntimePaths {
        models: root.join("models.json"),
        config: root.join("rustx.json"),
        skill_paths: Vec::new(),
        no_skills: false,
        no_builtin_tools: false,
        no_tools: false,
        tools: None,
        exclude_tools: Vec::new(),
        workspace: root.join("workspace"),
        runtime_root: root.join("private"),
    }
}

/// One composed child runtime.
struct Child {
    headless: HeadlessConversationRuntime,
    model: Arc<FakeModel>,
    log: Arc<Log>,
}

impl Child {
    fn runtime(&self) -> &ConversationRuntime {
        self.headless.runtime()
    }

    /// Composes the real local runtime over the parent's lab and activates it.
    ///
    /// `approve` decides what this child does with a native interaction: an
    /// approving child answers the request it observes, a non-approving child
    /// leaves the waiter pending forever. `client` decides whether an
    /// observation bridge — the seam a Runtime Client attaches through — exists
    /// at all, which is how the "no client present at crash time" case is
    /// built.
    async fn compose(
        root: &Path,
        scripts: Vec<Vec<FakeStep>>,
        approve: bool,
        client: bool,
    ) -> Result<Self, String> {
        let paths = lab_paths(root);
        let config_bytes = std::fs::read(&paths.config).map_err(|error| error.to_string())?;
        let runtime_config = CurrentRuntimeConfig::from_json_slice(&config_bytes)
            .map_err(|error| format!("{error:?}"))?;
        let model = Arc::new(FakeModel::new(scripts));
        let adapter: Arc<dyn crate::model::ModelAdapter> = model.clone();
        let registry = fixture_registry(
            &[
                FixtureModel::text(MODEL, ModelProtocol::OpenAiChatCompletions)
                    .with_context_window(1_000_000)
                    .with_max_output_tokens(4096),
            ],
            &ScriptedAdapterFactory::new(adapter),
        );
        let artifacts_root = paths.artifacts_root();
        let core = LocalConversationCore::compose_from_config(
            &paths,
            &LocalRuntimeDependencies::default(),
            registry,
            runtime_config.clone(),
            SessionPersistentState {
                model: runtime_config.model.clone(),
            },
            ConversationId::new(CONVERSATION),
            artifacts_root,
        )
        .await
        .map_err(|error| format!("{error:?}"))?;

        let log = Log::new();
        if client {
            let observations = Arc::new(PendingObservations::new());
            core.runtime()
                .install_observation_bridge(Arc::clone(&observations))
                .map_err(|error| format!("{error:?}"))?;
            // Native interactions are published only to a capable attachment.
            // A conformance child *is* that attachment: it observes the request
            // through the same bridge a Runtime Client uses and answers through
            // the same narrow runtime seam.
            core.runtime().set_interaction_provider_available(true);
            let runtime = core.runtime().clone();
            spawn_observer(&observations, Arc::clone(&log), runtime, approve);
        }
        let headless = core.into_headless();
        Ok(Self {
            headless,
            model,
            log,
        })
    }

    /// Composes and panics on failure: every scenario except [`COMPOSE_ONLY`]
    /// requires a runtime.
    async fn require(
        root: &Path,
        scripts: Vec<Vec<FakeStep>>,
        approve: bool,
        client: bool,
    ) -> Self {
        match Self::compose(root, scripts, approve, client).await {
            Ok(child) => child,
            Err(error) => panic!("the FND-06 child could not compose its runtime: {error}"),
        }
    }

    fn submit(&self, body: &str) {
        self.runtime()
            .submit_inbound(user_text(body))
            .expect("accept the inbound message");
    }

    /// Blocks until the scripted model parked mid-stream.
    async fn wait_model_parked(&self) {
        let mut parked = self.model.parked();
        parked
            .wait_for(|parked| *parked)
            .await
            .expect("the scripted model park watch stays open");
    }
}

/// Folds the runtime observation stream into the child's log, answering
/// native interactions when this child is an approving child.
fn spawn_observer(
    observations: &Arc<PendingObservations>,
    log: Arc<Log>,
    runtime: ConversationRuntime,
    approve: bool,
) {
    let observations = Arc::clone(observations);
    tokio::spawn(async move {
        loop {
            observations.wait().await;
            for observation in observations.drain() {
                match observation {
                    ConversationObservation::Event { event, .. } => {
                        log.push(Seen::Event(Box::new(event)));
                    }
                    ConversationObservation::InteractionPending { request, .. } => {
                        let id = request.id.clone();
                        log.push(Seen::InteractionPending);
                        if approve {
                            runtime
                                .respond_interaction(
                                    &id,
                                    InteractionResponse::Approval {
                                        decision: ApprovalDecision::Allow,
                                    },
                                )
                                .expect("answer the pending approval");
                        }
                    }
                    _ => {}
                }
            }
            if observations.is_closed() {
                return;
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Scripted turns
// ---------------------------------------------------------------------------

fn started() -> FakeStep {
    FakeStep::Emit(ModelEvent::Started)
}

fn text(chunk: &str) -> FakeStep {
    FakeStep::Emit(ModelEvent::TextDelta {
        block_index: ContentBlockIndex::new(0),
        text: chunk.to_owned(),
    })
}

fn done(reason: ModelFinishReason) -> FakeStep {
    FakeStep::Emit(ModelEvent::Completed {
        finish_reason: reason,
        usage: None,
    })
}

fn read_call(id: &'static str, file: &str) -> ScriptedCall {
    ScriptedCall {
        id,
        tool_id: "tool-read",
        name: "read",
        arguments: serde_json::json!({"path": file}),
    }
}

fn call_steps(index: u32, call: &ScriptedCall) -> Vec<FakeStep> {
    tool_call_events(index, call)
        .into_iter()
        .map(FakeStep::Emit)
        .collect()
}

/// One plain streaming text turn whose payload provably crosses the
/// publication byte threshold before the terminal event.
fn wide_text_turn() -> Vec<FakeStep> {
    vec![
        started(),
        text(WIDE_TEXT),
        text(" tail"),
        done(ModelFinishReason::Stop),
    ]
}

/// One short turn: nothing crosses the byte threshold, so the terminal
/// transaction carries the only frame.
fn short_text_turn() -> Vec<FakeStep> {
    vec![started(), text("ok"), done(ModelFinishReason::Stop)]
}

/// A turn that releases frames and then leaves a tool-call proposal
/// structurally incomplete at the terminal event.
fn structurally_incomplete_turn() -> Vec<FakeStep> {
    vec![
        started(),
        text(WIDE_TEXT),
        FakeStep::Emit(ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(1),
            call: ToolCallStart {
                id: ToolCallId::new("call-partial"),
                tool_id: ToolId::new("tool-read"),
                name: "read".to_owned(),
            },
        }),
        FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(1),
            call_id: ToolCallId::new("call-partial"),
            arguments_delta: "{\"file_p".to_owned(),
        }),
        done(ModelFinishReason::Stop),
    ]
}

/// A turn that proposes one complete tool call after releasing frames.
fn calling_turn(call: &ScriptedCall) -> Vec<FakeStep> {
    let mut steps = vec![started(), text(WIDE_TEXT)];
    steps.extend(call_steps(1, call));
    steps.push(done(ModelFinishReason::ToolCalls));
    steps
}

fn user_text(body: &str) -> Vec<UserContentBlock> {
    vec![UserContentBlock::Text(TextBlock {
        text: body.to_owned(),
    })]
}

/// Renders one runtime result for the parent's assertions.
fn describe<T: std::fmt::Debug, E: std::fmt::Debug>(result: &Result<T, E>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(error) => format!("{error:?}"),
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Runs one child scenario. Never returns: the parent owns the exit.
pub(crate) fn run(scenario: &str) -> ! {
    let root = PathBuf::from(std::env::var_os(ROOT_ENV).expect("a FND-06 lab root"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the child tokio runtime");
    runtime.block_on(async move { scenario_body(&root, scenario).await });
    idle()
}

#[allow(clippy::too_many_lines)] // one linear script per scenario, by design
async fn scenario_body(root: &Path, scenario: &str) {
    match scenario {
        TEXT_TURN | TEXT_TURN_NO_CLIENT | TERMINAL_ONLY_TURN | STRUCTURAL_FAILURE => {
            let script = match scenario {
                TERMINAL_ONLY_TURN => short_text_turn(),
                STRUCTURAL_FAILURE => structurally_incomplete_turn(),
                _ => wide_text_turn(),
            };
            let client = scenario != TEXT_TURN_NO_CLIENT;
            let child = Child::require(root, vec![script], false, client).await;
            child.submit("go");
            if client {
                child.log.wait_settled(1).await;
                note("settled");
            } else {
                // With no observation bridge there is no client-facing consumer
                // to wait on: this child is always killed at an armed durable
                // boundary instead.
                note("submitted");
            }
        }
        TOOL_TURN | TOOL_APPROVAL | TOOL_APPROVAL_PENDING => {
            let call = read_call("call-read-1", "note.txt");
            let child = Child::require(
                root,
                vec![
                    calling_turn(&call),
                    vec![started(), text("continued"), done(ModelFinishReason::Stop)],
                ],
                scenario == TOOL_APPROVAL,
                true,
            )
            .await;
            child.submit("read the note");
            if scenario == TOOL_APPROVAL_PENDING {
                // The waiter is process-owned state that never settles here:
                // the parent kills this child while the request is pending.
                child
                    .log
                    .wait_for(|seen| {
                        seen.iter()
                            .any(|entry| matches!(entry, Seen::InteractionPending))
                    })
                    .await;
                note("interaction-pending");
            } else {
                child.log.wait_settled(1).await;
                note("settled");
            }
        }
        TOOL_CONTINUATION_INBOUND => {
            let (release, receiver) = model_release();
            let call = read_call("call-read-continuation", "note.txt");
            let child = Child::require(
                root,
                vec![
                    calling_turn(&call),
                    vec![
                        started(),
                        text("continuing"),
                        FakeStep::ParkUntilReleased(receiver),
                        done(ModelFinishReason::Stop),
                    ],
                ],
                false,
                true,
            )
            .await;
            child.submit("read the note");
            // The tool already produced its canonical result and the
            // continuation request is provably in flight.
            child.wait_model_parked().await;
            child.submit("while the continuation runs");
            rendezvous("continuation-in-flight");
            release.send_replace(true);
            child.log.wait_settled(2).await;
            note("settled");
        }
        BACKGROUND_TOOL => {
            let call = ScriptedCall {
                id: "call-bash-1",
                tool_id: "tool-bash",
                name: "bash",
                arguments: serde_json::json!({
                    "command": "sleep 300",
                    "execution_mode": "background"
                }),
            };
            let child = Child::require(
                root,
                vec![
                    calling_turn(&call),
                    vec![started(), text("started"), done(ModelFinishReason::Stop)],
                ],
                false,
                true,
            )
            .await;
            child.submit("start the background command");
            child.log.wait_settled(1).await;
            note("settled");
        }
        COMPACTION => {
            let (release, receiver) = model_release();
            let child = Child::require(
                root,
                vec![
                    wide_text_turn(),
                    // The compaction summary side request: parked so the parent
                    // can kill this child while the side request is in flight.
                    vec![
                        FakeStep::ParkUntilReleased(receiver),
                        started(),
                        text("compact summary"),
                        done(ModelFinishReason::Stop),
                    ],
                    vec![
                        started(),
                        text("after compaction"),
                        done(ModelFinishReason::Stop),
                    ],
                ],
                false,
                true,
            )
            .await;
            child.submit("go");
            child.log.wait_settled(1).await;
            rendezvous("settled");
            let runtime = child.runtime().clone();
            let compaction = tokio::spawn(async move { runtime.compact_context().await });
            child.wait_model_parked().await;
            rendezvous("summary-in-flight");
            release.send_replace(true);
            let outcome = compaction.await.expect("the compaction task joins");
            note(&format!("compaction:{}", describe(&outcome)));
            child.submit("after compaction");
            child.log.wait_settled(2).await;
            note("post-compaction-settled");
        }
        LIVE_RESOURCE_EDIT => {
            let call = read_call("call-read-live", SKILL_FILE);
            let child = Child::require(
                root,
                vec![
                    wide_text_turn(),
                    calling_turn(&call),
                    vec![started(), text("continued"), done(ModelFinishReason::Stop)],
                ],
                false,
                true,
            )
            .await;
            child.submit("go");
            child.log.wait_settled(1).await;
            // The runtime is idle and R1 is durably recorded in the first
            // request snapshot: the parent now edits every resource on disk.
            rendezvous("first-attempt-settled");
            child.submit("read the skill");
            child.log.wait_settled(2).await;
            note("settled");
        }
        RELOAD => {
            let child = Child::require(
                root,
                vec![
                    wide_text_turn(),
                    vec![
                        started(),
                        text("after reload"),
                        done(ModelFinishReason::Stop),
                    ],
                ],
                false,
                true,
            )
            .await;
            child.submit("go");
            child.log.wait_settled(1).await;
            rendezvous("settled");
            let reloaded = child.runtime().reload_resources().await;
            note(&format!("reload:{}", describe(&reloaded)));
            // Whether the reload published R2 or kept R1, the next admitted
            // attempt records which generation it actually used.
            child.submit("after reload");
            child.log.wait_settled(2).await;
            note("reload-done");
        }
        RELOAD_BUSY => {
            let (release, receiver) = model_release();
            let child = Child::require(
                root,
                vec![vec![
                    started(),
                    text(WIDE_TEXT),
                    FakeStep::ParkUntilReleased(receiver),
                    done(ModelFinishReason::Stop),
                ]],
                false,
                true,
            )
            .await;
            child.submit("go");
            child.wait_model_parked().await;
            let reloaded = child.runtime().reload_resources().await;
            note(&format!("reload:{}", describe(&reloaded)));
            release.send_replace(true);
            child.log.wait_settled(1).await;
            note("settled");
        }
        COLD_RESUME | COLD_RESUME_READ => {
            let first = if scenario == COLD_RESUME_READ {
                calling_turn(&read_call("call-read-cold", SKILL_FILE))
            } else {
                wide_text_turn()
            };
            let child = Child::require(
                root,
                vec![
                    first,
                    vec![started(), text("resumed"), done(ModelFinishReason::Stop)],
                ],
                false,
                true,
            )
            .await;
            note(&format!(
                "recovery:{:?}",
                child.runtime().recovery().resume()
            ));
            child.submit("after reopen");
            child.log.wait_settled(1).await;
            note("settled");
        }
        COMPOSE_ONLY => {
            let composed = Child::compose(root, vec![wide_text_turn()], false, true).await;
            match composed {
                Ok(_) => note("compose:ok"),
                Err(error) => note(&format!("compose:err:{error}")),
            }
        }
        INBOUND_ONLY => {
            // Composition alone. The runtime is never activated, so accepted
            // inbound can never be adopted by this process.
            let paths = lab_paths(root);
            let config_bytes = std::fs::read(&paths.config).expect("read the lab runtime config");
            let runtime_config =
                CurrentRuntimeConfig::from_json_slice(&config_bytes).expect("valid runtime config");
            let adapter: Arc<dyn crate::model::ModelAdapter> = Arc::new(FakeModel::new(Vec::new()));
            let registry = fixture_registry(
                &[FixtureModel::text(
                    MODEL,
                    ModelProtocol::OpenAiChatCompletions,
                )],
                &ScriptedAdapterFactory::new(adapter),
            );
            let artifacts_root = paths.artifacts_root();
            let core = LocalConversationCore::compose_from_config(
                &paths,
                &LocalRuntimeDependencies::default(),
                registry,
                runtime_config.clone(),
                SessionPersistentState {
                    model: runtime_config.model.clone(),
                },
                ConversationId::new(CONVERSATION),
                artifacts_root,
            )
            .await
            .expect("compose the FND-06 child runtime");
            let accepted = core
                .tool_runtime()
                .durable_store()
                .accept_inbound(crate::durable::InboundDraft {
                    message_id: None,
                    source: crate::message::types::UserSource::Human,
                    kind: crate::message::types::InboundKind::Message,
                    content: user_text("never adopted"),
                    timestamp: chrono::Utc::now(),
                    correlation: None,
                })
                .expect("accept the inbound message");
            note(&format!("accepted:{}", accepted.message_id));
        }
        STREAMING_INBOUND => {
            let (release, receiver) = model_release();
            let child = Child::require(
                root,
                vec![
                    vec![
                        started(),
                        text(WIDE_TEXT),
                        FakeStep::ParkUntilReleased(receiver),
                        text(" tail"),
                        done(ModelFinishReason::Stop),
                    ],
                    vec![started(), text("second"), done(ModelFinishReason::Stop)],
                ],
                false,
                true,
            )
            .await;
            child.submit("go");
            // The first stream is provably open: the model is parked mid-stream
            // and the publication plane already released frames.
            child.wait_model_parked().await;
            child.submit("while streaming");
            rendezvous("second-inbound-accepted");
            release.send_replace(true);
            // The second inbound is adopted at the running attempt's next safe
            // boundary, so this stays one attempt with two model turns.
            child.log.wait_settled(1).await;
            note("settled");
        }
        other => panic!("unknown FND-06 scenario {other}"),
    }
}
