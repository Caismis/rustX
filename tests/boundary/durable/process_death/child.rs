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

use super::super::super::support::fake::{
    FakeModel, FakeStep, ScriptedCall, model_release, tool_call_events,
};
use super::super::super::support::model::{FixtureModel, ScriptedAdapterFactory, fixture_registry};
use crate::conversation::SurfaceRevision;
use crate::events::types::RuntimeEvent;
use crate::local_runtime::composition::{
    HeadlessConversationRuntime, LocalConversationCore, LocalRuntimeDependencies, LocalRuntimePaths,
};
use crate::local_runtime::config::CurrentRuntimeConfig;
use crate::local_runtime::session::{SessionCatalog, SessionPersistentState};
use crate::local_runtime::supervisor::LocalSessionSupervisor;
use crate::message::content::TextBlock;
use crate::message::types::{ContentBlockIndex, UserContentBlock};
use crate::model::error::{ModelError, ModelErrorKind, ModelRetryDisposition};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::types::ModelProtocol;
use crate::runtime::ApprovalDecision;
use crate::runtime::conversation_runtime::ConversationRuntime;
use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
use crate::runtime::interaction::InteractionResponse;
use crate::runtime::observation::{ConversationObservation, PendingObservations};
use crate::runtime::process_death;
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
/// One turn that durably owns a real subagent child process (Issue #60).
pub(crate) const SUBAGENT_TOOL: &str = "subagent_tool";
/// [`SUBAGENT_TOOL`] whose owned child answers with its terminal result, so
/// the terminal candidate is known and the publication transaction runs.
pub(crate) const SUBAGENT_SETTLED: &str = "subagent_settled";
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
/// A reload attempted while a **compaction** owns the session.
pub(crate) const RELOAD_BUSY_COMPACTION: &str = "reload_busy_compaction";
/// A reload attempted while a pending **interaction** owns the session.
pub(crate) const RELOAD_BUSY_INTERACTION: &str = "reload_busy_interaction";
/// A reopened conversation that admits one new request.
pub(crate) const COLD_RESUME: &str = "cold_resume";
/// One unresolved publication whose terminal attempt transition can be
/// killed and retried through startup recovery.
pub(crate) const UNRESOLVED_CARRYOVER: &str = "unresolved_carryover";
/// Two partial transient retries followed by a canonical success, killed at
/// the final attempt-terminal boundary so recovery must not select either
/// older retry audit as carryover.
pub(crate) const INTERNAL_RETRY_SUCCESS: &str = "internal_retry_success";
/// A reopened conversation whose new attempt reads the Skill file again.
pub(crate) const COLD_RESUME_READ: &str = "cold_resume_read";
/// Composition only: the child reports whether the current resources produced
/// a runtime at all.
pub(crate) const COMPOSE_ONLY: &str = "compose_only";
/// Inbound accepted durably with no adoption.
pub(crate) const INBOUND_ONLY: &str = "inbound_only";
/// One settled attempt, then a **second** ordinary inbound message, so a kill
/// can land inside the adoption/attempt-start window of a later turn of an
/// ordinary multi-turn conversation.
pub(crate) const SECOND_TURN: &str = "second_turn";
/// A streaming turn that is still open while a second inbound is accepted.
pub(crate) const STREAMING_INBOUND: &str = "streaming_inbound";
/// A reload attempted while a **foreground Tool execution** is running.
pub(crate) const RELOAD_BUSY_TOOL: &str = "reload_busy_tool";
/// A catalog-owning child that answers two turns, durably owns a background
/// execution and a subagent child, and then cuts a new lineage at the second
/// user message with `/fork`.
pub(crate) const SESSION_FORK: &str = "session_fork";
/// [`SESSION_FORK`] cut with `/branch` — a new node inside the same Session
/// rather than a new Session — so the cut rule is proven for both operations.
pub(crate) const SESSION_BRANCH: &str = "session_branch";
/// A catalog-owning child that reopens whatever lineage the catalog says is
/// active and answers one turn on it.
pub(crate) const SESSION_RESUME: &str = "session_resume";
/// One turn that starts a detached background execution, then a **second**
/// inbound turn whose Agent Status is composed while that execution is live.
pub(crate) const BACKGROUND_STATUS: &str = "background_status";
/// A native Todo mutation followed by a continuation, used to kill the
/// process around the Issue #130 opportunity and start boundaries.
pub(crate) const TODO_STATUS_TURN: &str = "todo_status_turn";
/// A reopened conversation that submits **nothing**, so recovery behavior can
/// be observed without a new inbound request creating work.
pub(crate) const RESUME_IDLE: &str = "resume_idle";

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

/// Blocks forever while still **owning** everything the scenario composed.
///
/// Taking the owner by value is the whole point. A scenario that ends without
/// waiting on runtime work — the no-client child, the composed-but-never-
/// activated child — would otherwise release the last semantic owner of its
/// `ConversationRuntime` on the way out. That closes the admission wake gate,
/// the admission worker reaches its terminal condition and exits, and the
/// armed durable boundary is never reached: the parent would then fail on its
/// outer liveness bound rather than on a conformance verdict, and whether it
/// failed at all would depend on which of the two won the race. Owning the
/// runtime here makes "the child is alive and idle at the moment of death" a
/// property of the type system instead of the scheduler.
///
/// Parking asynchronously (rather than blocking this thread) keeps every
/// runtime worker thread free, so the admission worker still reaches its
/// boundary on a single-core machine.
async fn park_owning<T>(_owner: T) -> ! {
    note("idle");
    std::thread::spawn(process_death::orphan_watchdog);
    loop {
        std::future::pending::<()>().await;
    }
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
        models: root.join("models.jsonc"),
        config: root.join("rustx.jsonc"),
        skill_paths: Vec::new(),
        no_skills: false,
        no_builtin_tools: false,
        no_tools: false,
        startup_session: rustx::local_runtime::StartupSession::Empty,
        session_name: None,
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
        Self::compose_lineage(root, scripts, approve, client, None).await
    }

    /// Composes over an explicit lineage instead of this lab's fixed one.
    ///
    /// `lineage` is `Some((conversation, artifacts_root))` for the
    /// catalog-owning scenarios, whose active conversation identity and
    /// private database directory are allocated by the Session catalog rather
    /// than fixed by the harness.
    async fn compose_lineage(
        root: &Path,
        scripts: Vec<Vec<FakeStep>>,
        approve: bool,
        client: bool,
        lineage: Option<(ConversationId, PathBuf)>,
    ) -> Result<Self, String> {
        let paths = lab_paths(root);
        let config_bytes = std::fs::read(&paths.config).map_err(|error| error.to_string())?;
        let runtime_config = CurrentRuntimeConfig::from_jsonc_slice(&config_bytes)
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
        let (conversation_id, artifacts_root) =
            lineage.unwrap_or_else(|| (ConversationId::new(CONVERSATION), paths.artifacts_root()));
        let core = LocalConversationCore::compose_from_config(
            &paths,
            &LocalRuntimeDependencies::default(),
            registry,
            runtime_config.clone(),
            SessionPersistentState {
                model: runtime_config.model.clone(),
            },
            conversation_id,
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

    /// Stages one **real** OS process as this conversation's next subagent
    /// child and returns the control peer the registry's driver talks to.
    ///
    /// The process runs `command` in *this* child's process group, exactly
    /// like the background row's real `sleep 300`: the parent's `killpg`
    /// reaps it with everything else, and until then a long-running one is a
    /// genuine orphan candidate. A child whose peer is never answered stays
    /// durably owned and unsettled at the moment of death; one that answers
    /// (and whose process exits, so the driver can observe a physical
    /// terminal) drives the publication transaction instead.
    fn stage_live_subagent_child(&self, root: &Path, command: &str) -> tokio::net::UnixStream {
        let (driver_end, peer) = tokio::net::UnixStream::pair().expect("subagent control pair");
        let (observation_end, _observation_peer) =
            tokio::net::UnixStream::pair().expect("subagent observation pair");
        let process = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the staged subagent child process");
        let runtime_root = root.join("private/artifacts/subagents/staged");
        std::fs::create_dir_all(&runtime_root).expect("staged child runtime root");
        self.runtime()
            .subagents()
            .expect("the composed runtime owns a subagent registry")
            .push_staged_override(crate::runtime::subagent::process::StagedChild::for_test(
                process,
                driver_end,
                observation_end,
                runtime_root,
            ));
        peer
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
                    ConversationObservation::InteractionPending { interaction, .. } => {
                        let id = interaction.interaction.clone();
                        log.push(Seen::InteractionPending);
                        if approve {
                            runtime
                                .respond_interaction(
                                    &id,
                                    InteractionResponse::Approval {
                                        decision: ApprovalDecision::Allow,
                                    },
                                )
                                .await
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

/// Composes one **catalog-owning** child.
///
/// This is the same composition the `rustx` binary performs for a native
/// Session, minus the model registry (a conformance child scripts its
/// provider) and the Runtime Client host (this child answers no protocol
/// input): the real `SessionCatalog` on the lab's runtime-private root, the
/// real conversation runtime of whatever node the catalog says is active, and
/// the real `LocalSessionSupervisor` with that runtime installed. `/fork` and
/// `/branch` are then the production supervisor operations, not a harness
/// re-implementation of them.
async fn compose_session_child(
    root: &Path,
    scripts: Vec<Vec<FakeStep>>,
) -> (Child, Arc<LocalSessionSupervisor>) {
    let paths = lab_paths(root);
    let config_bytes = std::fs::read(&paths.config).expect("read the lab runtime config");
    let runtime_config =
        CurrentRuntimeConfig::from_jsonc_slice(&config_bytes).expect("valid runtime config");
    let template = SessionPersistentState {
        model: runtime_config.model.clone(),
    };
    let catalog = match SessionCatalog::open_existing(&paths.runtime_root)
        .expect("open the native Session catalog")
    {
        Some(catalog) => catalog,
        None => SessionCatalog::create(&paths.runtime_root, &template)
            .expect("publish the first native Session"),
    };
    let (session_id, node, session_state) =
        catalog.active_lineage().expect("an active Session lineage");
    let database_path = catalog.database_path(&session_id, &node.conversation_id);
    let artifacts_root = database_path
        .parent()
        .expect("the active conversation database has a parent")
        .to_path_buf();
    let supervisor = Arc::new(LocalSessionSupervisor::new(
        catalog,
        session_state.model.clone(),
    ));
    let child = Child::compose_lineage(
        root,
        scripts,
        false,
        true,
        Some((node.conversation_id.clone(), artifacts_root)),
    )
    .await
    .unwrap_or_else(|error| panic!("the FND-06 session child could not compose: {error}"));
    supervisor
        .install_runtime(child.runtime().clone())
        .await
        .expect("install the active runtime into its supervisor");
    (child, supervisor)
}

/// The `MessageId` of the `nth` (1-based) ordinary human message of the
/// current canonical head, with the head's Surface revision.
///
/// This is exactly the boundary a `/fork` or `/branch` user picks. Runtime
/// messages are skipped explicitly: a detached terminal notice is also an
/// `InboundKind::Message`, so filtering on the kind alone would let a
/// publication the runtime authored become a cut boundary.
fn human_boundary(child: &Child, nth: usize) -> (SurfaceRevision, MessageId) {
    let (revision, messages) = child
        .runtime()
        .historical_head_snapshot()
        .expect("the canonical head snapshot");
    let id = messages
        .iter()
        .filter_map(|message| match message {
            crate::message::types::MessageBlock::User(user)
                if user.kind == crate::message::types::InboundKind::Message
                    && user.source == crate::message::types::UserSource::Human =>
            {
                Some(user.id.clone())
            }
            _ => None,
        })
        .nth(nth - 1)
        .unwrap_or_else(|| panic!("the canonical head holds a {nth}th human message"));
    (revision, id)
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

/// One partial publication whose model request fails before the attempt
/// terminal transaction. The publication audit is durable first; the parent
/// can therefore kill the producer transition and make recovery select the
/// same source from the same keyed audit on its second try.
fn unresolved_output_turn() -> Vec<FakeStep> {
    vec![
        started(),
        text(WIDE_TEXT),
        FakeStep::Emit(ModelEvent::Failed {
            error: ModelError {
                kind: ModelErrorKind::Transport,
                message: "connection interrupted after partial output".to_owned(),
                retry_disposition: ModelRetryDisposition::Never,
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
                generation: None,
            },
        }),
    ]
}

/// One partial publication that is retryable, so it becomes an internal audit
/// generation rather than an unresolved logical-step outcome.
fn retryable_unresolved_output_turn() -> Vec<FakeStep> {
    vec![
        started(),
        text(WIDE_TEXT),
        FakeStep::Emit(ModelEvent::Failed {
            error: ModelError {
                kind: ModelErrorKind::Transport,
                message: "connection interrupted after partial retry output".to_owned(),
                retry_disposition: ModelRetryDisposition::Transient,
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
                generation: None,
            },
        }),
    ]
}

/// The final retry generation is accepted canonically.
fn retry_success_turn() -> Vec<FakeStep> {
    vec![
        started(),
        text("accepted after internal retries"),
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

/// Plays the owned child's side of the delegation exactly once: await the
/// `Delegate` frame, answer with one succeeded terminal result.
///
/// This is the real IPC contract of a v1 child, spoken over the real control
/// channel the registry's driver owns.
async fn answer_subagent_delegation(peer: &mut tokio::net::UnixStream) {
    use crate::runtime::subagent::ipc::{
        ChildFrame, ChildResultStatus, ParentFrame, ResultFrame, read_parent_frame,
        write_child_frame,
    };
    let frame = read_parent_frame(peer).await.expect("a delegation frame");
    assert!(
        matches!(frame, Some(ParentFrame::Delegate(_))),
        "the durably owned child is delegated first"
    );
    write_child_frame(
        peer,
        &ChildFrame::Result(ResultFrame {
            status: ChildResultStatus::Succeeded,
            content: Some("the note says R1".to_owned()),
            diagnostic: None,
        }),
    )
    .await
    .expect("the terminal result frame");
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
    // One linear script per scenario makes this future large by construction;
    // boxing it keeps the child's entry frame small.
    runtime.block_on(Box::pin(scenario_body(&root, scenario)));
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
                // boundary instead. It parks here still owning its runtime, so
                // the admission worker provably survives to reach that
                // boundary.
                note("submitted");
                park_owning(child).await;
            }
        }
        UNRESOLVED_CARRYOVER => {
            let child = Child::require(root, vec![unresolved_output_turn()], false, true).await;
            child.submit("recover unresolved output");
            if std::env::var(process_death::GATE_ENV).is_ok() {
                // The parent chooses a durable terminal boundary. Keeping the
                // runtime alive here makes the process own the exact in-flight
                // transition until SIGKILL.
                park_owning(child).await;
            } else {
                child.log.wait_settled(1).await;
                note("settled");
            }
        }
        INTERNAL_RETRY_SUCCESS => {
            let child = Child::require(
                root,
                vec![
                    retryable_unresolved_output_turn(),
                    retryable_unresolved_output_turn(),
                    retry_success_turn(),
                ],
                false,
                true,
            )
            .await;
            child.submit("recover internal retry success");
            if std::env::var(process_death::GATE_ENV).is_ok() {
                // The final successful generation has already reached
                // canonical acceptance when this gate is reached. The parent
                // kills the attempt-terminal transaction so recovery must
                // derive that fact from durable request/snapshot evidence.
                park_owning(child).await;
            } else {
                child.log.wait_settled(1).await;
                note("settled");
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
        SUBAGENT_TOOL | SUBAGENT_SETTLED => {
            let call = ScriptedCall {
                id: "call-subagent-1",
                tool_id: "tool-subagent",
                name: "subagent",
                arguments: serde_json::json!({
                    "agent": "explore",
                    "task": "inspect the workspace note"
                }),
            };
            let child = Child::require(
                root,
                vec![
                    calling_turn(&call),
                    vec![started(), text("delegated"), done(ModelFinishReason::Stop)],
                ],
                false,
                true,
            )
            .await;
            // The staged child is a **real** OS process that never answers, so
            // the conversation durably owns a live child at the moment of
            // death. Staging is the registry's own `cfg(test)` seam: the spawn
            // and startup handshake are replaced, and everything the row
            // proves — the ownership commit, the durable lifecycle, recovery
            // reconciliation, and the terminal publication — is the real path.
            // A settling child must also *exit*, because a terminal candidate
            // becomes physical only when the driver reaps the process; an
            // unsettled one must stay alive to be the durably owned child the
            // kill lands on.
            let mut peer = child.stage_live_subagent_child(
                root,
                if scenario == SUBAGENT_SETTLED {
                    "exit 0"
                } else {
                    "exec sleep 300"
                },
            );
            child.submit("delegate the task");
            if scenario == SUBAGENT_SETTLED {
                // Answering the delegation makes the terminal *candidate*
                // durably known while the terminal *publication* is still an
                // uncommitted transaction — the state the publication
                // atomicity row kills in.
                answer_subagent_delegation(&mut peer).await;
            }
            child.log.wait_settled(1).await;
            // Only reached when no boundary was armed. The control peer stays
            // owned so nothing can settle the child from this side.
            note("settled");
            park_owning((child, peer)).await;
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
        RELOAD_BUSY_COMPACTION => {
            let (release, receiver) = model_release();
            let child = Child::require(
                root,
                vec![
                    wide_text_turn(),
                    vec![
                        FakeStep::ParkUntilReleased(receiver),
                        started(),
                        text("compact summary"),
                        done(ModelFinishReason::Stop),
                    ],
                ],
                false,
                true,
            )
            .await;
            child.submit("go");
            child.log.wait_settled(1).await;
            // No attempt owns the session here: the only owner is the manual
            // compaction, whose summary side request is provably in flight.
            let runtime = child.runtime().clone();
            let compaction = tokio::spawn(async move { runtime.compact_context().await });
            child.wait_model_parked().await;
            let reloaded = child.runtime().reload_resources().await;
            note(&format!("reload:{}", describe(&reloaded)));
            release.send_replace(true);
            let outcome = compaction.await.expect("the compaction task joins");
            note(&format!("compaction:{}", describe(&outcome)));
            park_owning(child).await;
        }
        RELOAD_BUSY_INTERACTION => {
            let call = read_call("call-read-busy", "note.txt");
            let child = Child::require(
                root,
                vec![
                    calling_turn(&call),
                    vec![started(), text("continued"), done(ModelFinishReason::Stop)],
                ],
                // A non-approving child leaves the waiter pending forever, so
                // the interaction owns the session at the reload boundary.
                false,
                true,
            )
            .await;
            child.submit("read the note");
            child
                .log
                .wait_for(|seen| {
                    seen.iter()
                        .any(|entry| matches!(entry, Seen::InteractionPending))
                })
                .await;
            let reloaded = child.runtime().reload_resources().await;
            note(&format!("reload:{}", describe(&reloaded)));
            park_owning(child).await;
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
            let runtime_config = CurrentRuntimeConfig::from_jsonc_slice(&config_bytes)
                .expect("valid runtime config");
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
            // The claim is that a *live* process which never activated its
            // runtime never adopts. Holding the composed core proves exactly
            // that; releasing it would only prove that a torn-down runtime
            // adopts nothing.
            park_owning(core).await;
        }
        SECOND_TURN => {
            let child = Child::require(
                root,
                vec![
                    wide_text_turn(),
                    vec![started(), text("second"), done(ModelFinishReason::Stop)],
                ],
                false,
                true,
            )
            .await;
            child.submit("first");
            child.log.wait_settled(1).await;
            // The first turn is durably answered and its attempt is durably
            // terminal. The second message is submitted from a quiescent
            // runtime, so the only durable transitions left are its own.
            rendezvous("first-attempt-settled");
            child.submit("second");
            park_owning(child).await;
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
        RELOAD_BUSY_TOOL => {
            // A **foreground** tool execution owns the session for as long as
            // the command runs. `sleep 300` outlives the parent's liveness
            // bound by construction, so the reload boundary below is reached
            // while the execution is provably still running rather than in a
            // race with its completion.
            let call = ScriptedCall {
                id: "call-bash-foreground",
                tool_id: "tool-bash",
                name: "bash",
                arguments: serde_json::json!({
                    "command": "sleep 300",
                    "execution_mode": "foreground"
                }),
            };
            let child = Child::require(
                root,
                vec![
                    calling_turn(&call),
                    vec![started(), text("continued"), done(ModelFinishReason::Stop)],
                ],
                false,
                true,
            )
            .await;
            child.submit("run the command");
            // The durable `ToolExecutionStarted` fact is the happens-after
            // proof that the execution is running: the reload below is
            // attempted strictly after it.
            child
                .log
                .wait_for(|seen| {
                    seen.iter().any(|entry| {
                        matches!(entry, Seen::Event(event)
                            if matches!(**event, RuntimeEvent::ToolExecutionStarted { .. }))
                    })
                })
                .await;
            let reloaded = child.runtime().reload_resources().await;
            note(&format!("reload:{}", describe(&reloaded)));
            park_owning(child).await;
        }
        BACKGROUND_STATUS => {
            let call = ScriptedCall {
                id: "call-bash-status",
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
                    vec![
                        started(),
                        text("second answer"),
                        done(ModelFinishReason::Stop),
                    ],
                ],
                false,
                true,
            )
            .await;
            child.submit("start the background command");
            child.log.wait_settled(1).await;
            // The second turn's Agent Status is composed while the execution
            // is durably owned and live, so the historical status fact names
            // it. That is what makes the reopened lineage's status a real
            // proof: the same runtime that would list a live execution lists
            // none.
            child.submit("what is running");
            child.log.wait_settled(2).await;
            note("settled");
            park_owning(child).await;
        }
        TODO_STATUS_TURN => {
            super::harness::write_runtime_config_with_todo(root);
            let call = ScriptedCall {
                id: "call-todo-status",
                tool_id: crate::tools::todo::TODO_TOOL_ID,
                name: "todo",
                arguments: serde_json::json!({
                    "action": "create",
                    "subject": "Keep the Todo reminder durable"
                }),
            };
            let child = Child::require(
                root,
                vec![
                    calling_turn(&call),
                    vec![started(), text("continued"), done(ModelFinishReason::Stop)],
                ],
                false,
                true,
            )
            .await;
            child.submit("create a Todo");
            child.log.wait_settled(1).await;
            note("settled");
            park_owning(child).await;
        }
        SESSION_FORK | SESSION_BRANCH => {
            let bash = ScriptedCall {
                id: "call-bash-lineage",
                tool_id: "tool-bash",
                name: "bash",
                arguments: serde_json::json!({
                    "command": "sleep 300",
                    "execution_mode": "background"
                }),
            };
            let delegate = ScriptedCall {
                id: "call-subagent-lineage",
                tool_id: "tool-subagent",
                name: "subagent",
                arguments: serde_json::json!({
                    "agent": "explore",
                    "task": "inspect the workspace note"
                }),
            };
            let (child, supervisor) = compose_session_child(
                root,
                vec![
                    calling_turn(&bash),
                    vec![
                        started(),
                        text("background started"),
                        done(ModelFinishReason::Stop),
                    ],
                    calling_turn(&delegate),
                    vec![started(), text("delegated"), done(ModelFinishReason::Stop)],
                    vec![
                        started(),
                        text("the background task is still running"),
                        done(ModelFinishReason::Stop),
                    ],
                    vec![
                        started(),
                        text("acknowledged"),
                        done(ModelFinishReason::Stop),
                    ],
                    // Exactly six scripts for exactly four attempts: the two
                    // owning turns take two model turns each, the subagent
                    // terminal notice and the final human turn one each.
                ],
            )
            .await;

            // The ownership facts and their identities are committed **first**,
            // so everything they wrote into canonical history lands inside the
            // prefix the cut will copy rather than behind it. That is the whole
            // point of the row: the destination inherits the *words* naming a
            // live execution and a real subagent child, and must inherit none
            // of the ownership.
            let mut peer = child.stage_live_subagent_child(root, "exit 0");
            child.submit("own the work");
            child.log.wait_settled(1).await;

            // The `subagent` tool returns as soon as the child is running, so
            // this attempt settles without the child having answered anything.
            child.submit("delegate the task");
            child.log.wait_settled(2).await;
            // Only now is the delegation answered. The terminal notice is
            // therefore published into an idle conversation and can only be
            // adopted as a turn of its own — there is no running attempt for it
            // to be drained into at a safe boundary, so the attempt count below
            // is exact rather than racing the publication.
            answer_subagent_delegation(&mut peer).await;
            child.log.wait_settled(3).await;

            // This turn's Agent Status is composed while `exec_1` is live. It
            // is also the cut boundary, so what the destination copies is
            // everything strictly before it: both tool results naming the
            // source's execution and subagent identities, the `UserSource::Agent`
            // message the child itself produced, and two Agent Status messages
            // naming the running execution.
            child.submit("what is running");
            child.log.wait_settled(4).await;

            let (revision, boundary) = human_boundary(&child, 3);
            let outcome = if scenario == SESSION_FORK {
                supervisor.fork_active(revision, boundary).await
            } else {
                supervisor.tree_branch(revision, boundary).await
            };
            note(&format!("cut:{}", describe(&outcome)));
            // Only reached when no publication boundary was armed. The process
            // now waits for its death still owning the whole composition, so
            // nothing can settle from this side.
            park_owning((child, peer, supervisor)).await;
        }
        SESSION_RESUME => {
            // Deliberately a plain turn that starts **nothing**. The copied
            // history names a live background execution and a real subagent
            // child; the only honest way to show those words were never
            // resolved into ownership is for this lineage to own nothing at
            // all while answering normally.
            let (child, supervisor) = compose_session_child(
                root,
                vec![vec![
                    started(),
                    text("resumed on the cut lineage"),
                    done(ModelFinishReason::Stop),
                ]],
            )
            .await;
            note(&format!(
                "recovery:{:?}",
                child.runtime().recovery().resume()
            ));
            child.submit("continue on the cut lineage");
            child.log.wait_settled(1).await;
            note("settled");
            park_owning((child, supervisor)).await;
        }
        RESUME_IDLE => {
            // Nothing is submitted here. A recovery-permitted continuation,
            // when an independent durable answer obligation allows one, is
            // therefore distinguishable from new inbound work. The
            // Issue #130 externally-started dead-tool case intentionally has
            // no such continuation: it remains terminal and cannot recover
            // the dead attempt's process-local PostToolBatch marker.
            let child = Child::require(
                root,
                vec![vec![
                    started(),
                    text("continued"),
                    done(ModelFinishReason::Stop),
                ]],
                false,
                true,
            )
            .await;
            note(&format!(
                "recovery:{:?}",
                child.runtime().recovery().resume()
            ));
            park_owning(child).await;
        }
        other => panic!("unknown FND-06 scenario {other}"),
    }
}
