//! Deterministic MCP Tasks execution regressions (Issue #243).
//!
//! Every test here drives a **real** MCP peer — the self-spawned stdio
//! fixture (or, where in-flight HTTP ownership is the subject, the in-process
//! Streamable HTTP fixture) — over the ordinary `McpServerRuntime` transport
//! and the ordinary [`ToolExecutor`] boundary. The fixture answers
//! `tools/call` with a genuine `CreateTaskResult`, serves genuine `tasks/get`
//! snapshots, accepts genuine `tasks/update` responses, and acknowledges a
//! genuine `tasks/cancel`, so the proofs are about the shipped protocol path
//! rather than about a mock of it.
//!
//! No test uses a sleep as a correctness proof. Every race is decided at a
//! named linearization point: the task-request dispatch barriers
//! (`tasks/get`, `tasks/update`, `tasks/cancel`), the coordinator's own
//! pending-publication observer, and the HTTP fixture's server-side
//! disconnect observation.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::durable::TranscriptCursor;
use crate::events::RuntimeEventEnvelope;
use crate::events::interaction::{
    AnswerSpecification, OptionAnswer, OptionSpecification, QuestionSpecification,
    QuestionnaireAnswer, QuestionnaireAnswerEntry, QuestionnaireResponse, QuestionnaireSubmission,
};
use crate::runtime::identity::{AttemptId, ConversationId, InteractionId, McpServerId, ToolCallId};
use crate::runtime::interaction::{
    InteractionCoordinator, InteractionObserver, InteractionRequest, InteractionResponse,
    QuestionnaireRequester, RecordingInteractionAudit,
};
use crate::runtime::types::{CancellationReason, ConversationLifecycle};
use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
use crate::tools::mcp::fixture::tasks::{
    CANCEL_TASK, GET_TASK, TASK_CANCELLED_TOOL, TASK_EMPTY_INPUT_TOOL, TASK_FAILED_TOOL,
    TASK_FOREIGN_TOOL, TASK_FOREVER_TOOL, TASK_INPUT_TOOL, TASK_MRTR_TOOL, TASK_NEW_INPUT_TOOL,
    TASK_OBSERVATION_FILE_ENV, TASK_SAMPLING_TOOL, TASK_SIMPLE_TOOL, TASK_STALE_INPUT_TOOL,
    TASK_TOOLS_ENV, TASK_UNADVERTISED_ENV, TaskObservation, UPDATE_TASK, fixture_task_id,
    task_observations,
};
use crate::tools::mcp::fixture::{
    FIXTURE_MODE_ENV, FixtureServer, PROTOCOL_VERSIONS_ENV, TOOL_PREFIX_ENV, fixture_spawn_args,
    serve_if_fixture_mode,
};
use crate::tools::mcp::test_sync::TaskRequestBarrier;
use crate::tools::mcp::{
    McpInvalidationState, McpServerBinding, McpServerRuntime, McpTransportConfig,
};
use crate::tools::runtime::ConversationToolRuntime;
use crate::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationId, ToolInvocationMode,
    ToolInvocationPolicy, ToolProgress,
};
use crate::tools::workspace::Workspace;

use crate::runtime::inbound::ConversationInboundMailbox;
use crate::tools::background::{
    BackgroundDispatchOutcome, BackgroundResources, ConversationBackgroundRegistry,
};

/// The per-test MCP server identity one harness connects as.
///
/// Every deterministic seam in this suite is keyed by the server id — the
/// task-request barriers above all — and the whole suite runs in one binary
/// **in parallel**, so a suite-wide id would let one test's barrier park
/// another test's `tasks/get`. The id is therefore derived from the test's
/// own path, exactly as the model-facing tool prefix is.
fn server_id(test_name: &str) -> String {
    let leaf = test_name.rsplit("::").next().unwrap_or(test_name);
    format!("tasks-fixture-{leaf}")
}

/// A progress reporter that keeps every reported fact.
///
/// Polling is transport activity, not Tool progress: several tests assert
/// that a task driven through many `tasks/get` requests reports **no**
/// progress at all, which is what "rustX never fabricates liveness" means in
/// an observable form.
#[derive(Default)]
struct RecordingProgress {
    reported: Mutex<Vec<ToolProgress>>,
}

impl ProgressReporter for RecordingProgress {
    fn report(&self, progress: ToolProgress) {
        self.reported.lock().expect("progress lock").push(progress);
    }
}

impl RecordingProgress {
    fn reported(&self) -> Vec<ToolProgress> {
        self.reported.lock().expect("progress lock").clone()
    }
}

/// Publishes every newly pending interaction on a channel, so a test answers
/// exactly the interaction the executor published.
struct PendingSink {
    sender: tokio::sync::mpsc::UnboundedSender<InteractionRequest>,
}

impl InteractionObserver for PendingSink {
    fn on_pending(
        &self,
        request: &InteractionRequest,
        _audit: &RuntimeEventEnvelope,
        _transcript_cursor: TranscriptCursor,
    ) {
        let _ = self.sender.send(request.clone());
    }

    fn on_settled(
        &self,
        _id: &InteractionId,
        _outcome: &crate::runtime::interaction::InteractionOutcome,
        _audit: Option<&(RuntimeEventEnvelope, TranscriptCursor)>,
    ) {
    }
}

/// One connected fixture peer plus the conversation-owned interaction
/// authority its invocations use.
struct Harness {
    prefix: String,
    server: String,
    _directory: tempfile::TempDir,
    runtime: Arc<McpServerRuntime>,
    tool_runtime: ConversationToolRuntime,
    coordinator: Arc<InteractionCoordinator>,
    audit: Arc<RecordingInteractionAudit>,
    pending: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<InteractionRequest>>,
    observations: PathBuf,
    owner: crate::agent::cancellation::AgentCancellation,
}

/// The environment overlay one fixture launch runs with.
#[derive(Default)]
struct FixtureOptions {
    /// Publish the Tasks guard tools **without** advertising the extension.
    unadvertised: bool,
    /// Narrow the fixture to one legacy MCP revision.
    legacy: bool,
    wrong_ack: Option<&'static str>,
    completing_gate: Option<String>,
    oversized_seed: bool,
}

/// The per-test model-facing tool-name prefix, derived from the test's path.
fn tool_prefix(test_name: &str) -> String {
    let leaf = test_name.rsplit("::").next().unwrap_or(test_name);
    format!("{leaf}__")
}

impl Harness {
    /// Connects the self-spawned stdio fixture with the Tasks guard tools
    /// published, and builds a live interaction coordinator.
    async fn connect(test_name: &str, options: FixtureOptions) -> Self {
        let directory = tempfile::tempdir().expect("fixture root");
        let workspace_root = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let observations = directory.path().join("task-observations.jsonl");
        let prefix = tool_prefix(test_name);
        let server = server_id(test_name);
        let mut environment = std::collections::BTreeMap::from([
            (FIXTURE_MODE_ENV.to_owned(), "1".to_owned()),
            (TOOL_PREFIX_ENV.to_owned(), prefix.clone()),
            (
                TASK_OBSERVATION_FILE_ENV.to_owned(),
                observations.display().to_string(),
            ),
        ]);
        if options.unadvertised {
            environment.insert(TASK_UNADVERTISED_ENV.to_owned(), "1".to_owned());
        } else {
            environment.insert(TASK_TOOLS_ENV.to_owned(), "1".to_owned());
        }
        if options.legacy {
            environment.insert(PROTOCOL_VERSIONS_ENV.to_owned(), "2025-06-18".to_owned());
        }
        if options.oversized_seed {
            environment.insert("RUSTX_TASK_OVERSIZED_SEED".to_owned(), "1".to_owned());
        }
        if let Some(method) = options.wrong_ack {
            environment.insert("RUSTX_TASK_WRONG_ACK".to_owned(), method.to_owned());
        }
        if let Some(address) = options.completing_gate {
            environment.insert("RUSTX_TASK_COMPLETING_GATE".to_owned(), address);
        }
        let binding = McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            activation: rustx::capabilities::activation::SourceActivation::Enabled,
            resource_workspace: None,
            transport: McpTransportConfig::Stdio {
                program: std::env::current_exe()
                    .expect("test executable")
                    .display()
                    .to_string(),
                args: fixture_spawn_args(test_name),
                cwd: None,
                environment,
            },
            policy: ToolInvocationPolicy::default(),
        };
        let workspace = Workspace::new(&workspace_root).expect("workspace");
        let runtime = McpServerRuntime::connect(
            &McpServerId::new(&server),
            &binding,
            &workspace,
            Arc::new(McpInvalidationState::new()),
        )
        .await
        .expect("the fixture connects");
        let expected = if options.legacy {
            "2025-06-18"
        } else {
            rmcp::model::ProtocolVersion::V_2026_07_28.as_str()
        };
        assert_eq!(
            runtime.protocol_version().as_str(),
            expected,
            "the negotiated revision qualifies every proof below"
        );
        let tool_runtime = ConversationToolRuntime::new(
            ConversationId::new("tasks"),
            workspace_root,
            directory.path().join("artifacts"),
        )
        .expect("tool runtime");
        let lifecycle = ConversationLifecycle::new();
        assert!(lifecycle.activate());
        let conversation_id = ConversationId::new("tasks");
        let audit = RecordingInteractionAudit::new(conversation_id.clone());
        let coordinator = Arc::new(InteractionCoordinator::new(
            conversation_id,
            lifecycle,
            audit.clone(),
        ));
        coordinator.set_provider_available(true);
        let (sender, pending) = tokio::sync::mpsc::unbounded_channel();
        coordinator.install_observer(Arc::new(PendingSink { sender }));
        Self {
            prefix,
            server,
            _directory: directory,
            runtime,
            tool_runtime,
            coordinator,
            audit,
            pending: tokio::sync::Mutex::new(pending),
            observations,
            owner: crate::agent::cancellation::AgentCancellation::new(
                CancellationReason::UserRequested,
            ),
        }
    }

    fn requester(&self) -> QuestionnaireRequester {
        QuestionnaireRequester::new(
            Arc::clone(&self.coordinator),
            AttemptId::new("tasks-attempt"),
            self.owner.execution_cancellation(),
            7,
        )
    }

    /// Starts one invocation of `tool` through the canonical `ToolExecutor`
    /// boundary. `interaction` decides whether this invocation holds the
    /// runtime-owned Questionnaire authority.
    fn invoke<'a>(
        &'a self,
        tool: &str,
        call_id: &str,
        progress: &'a RecordingProgress,
        interaction: bool,
    ) -> impl std::future::Future<Output = ToolExecutionResult> + 'a {
        let executor =
            crate::tools::mcp::McpToolExecutor::new(Arc::clone(&self.runtime), self.tool(tool));
        let invocation = ToolInvocation {
            id: ToolInvocationId::Agent {
                call_id: ToolCallId::new(call_id),
            },
            tool_id: crate::runtime::identity::ToolId::new("mcp:tasks"),
            tool_name: self.tool(tool),
            mode: ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"subject": "release"}),
        };
        let context = ToolExecutionContext::new(
            self.tool_runtime.conversation_id(),
            None,
            self.owner.execution_cancellation(),
            self.tool_runtime.workspace(),
            progress,
            self.tool_runtime.artifacts(),
            self.tool_runtime.tool_output(),
            self.tool_runtime.environment(),
        );
        let context = if interaction {
            context.with_questionnaire_requester(self.requester())
        } else {
            context
        };
        async move {
            let executor = executor;
            executor.start(invocation, context).completion.await
        }
    }

    /// Waits for the next published interaction — the coordinator's own
    /// observation, never a poll.
    async fn next_pending(&self) -> InteractionRequest {
        self.pending
            .lock()
            .await
            .recv()
            .await
            .expect("one pending interaction")
    }

    /// Answers a published questionnaire by choosing `label` for question 0.
    async fn answer(&self, request: &InteractionRequest, label: &str) {
        let questionnaire = questionnaire_of(request);
        let option_index = option_index(&questionnaire.questions[0], label);
        self.coordinator
            .respond_async(
                &request.id,
                InteractionResponse::Questionnaire {
                    response: QuestionnaireResponse::Submitted(QuestionnaireSubmission {
                        answers: vec![QuestionnaireAnswerEntry {
                            question_index: 0,
                            answer: QuestionnaireAnswer::Option(OptionAnswer { option_index }),
                        }],
                    }),
                },
            )
            .await
            .expect("the questionnaire accepts the answer");
    }

    /// The scoped model-facing name of one fixture tool.
    fn tool(&self, tool: &str) -> String {
        format!("{}{tool}", self.prefix)
    }

    /// The scoped task id one fixture scenario materializes.
    fn task_id(&self, tool: &str) -> String {
        fixture_task_id(&self.tool(tool))
    }

    /// Every task-protocol request the fixture actually received.
    fn observed(&self) -> Vec<TaskObservation> {
        task_observations(&self.observations)
    }

    /// Every observed request of one method.
    fn observed_method(&self, method: &str) -> Vec<TaskObservation> {
        self.observed()
            .into_iter()
            .filter(|observation| observation.method == method)
            .collect()
    }

    /// Installs a barrier at one task-request dispatch frontier of **this
    /// harness's own** server, so a parallel test's requests can never park
    /// on it.
    fn barrier(
        &self,
        method: &'static str,
    ) -> (
        Arc<TaskRequestBarrier>,
        crate::tools::mcp::test_sync::TaskRequestBarrierGuard,
    ) {
        TaskRequestBarrier::install(&self.server, method)
    }

    /// The same barrier, holding only from the `first_held`-th request.
    fn barrier_from(
        &self,
        method: &'static str,
        first_held: usize,
    ) -> (
        Arc<TaskRequestBarrier>,
        crate::tools::mcp::test_sync::TaskRequestBarrierGuard,
    ) {
        TaskRequestBarrier::install_from(&self.server, method, first_held)
    }

    async fn shutdown(&self) {
        let _ = self.runtime.close().await;
    }
}

/// The published questionnaire of one pending interaction.
fn questionnaire_of(
    request: &InteractionRequest,
) -> crate::events::interaction::QuestionnaireSpecification {
    match &request.kind {
        crate::runtime::interaction::InteractionKind::Questionnaire { questionnaire, .. } => {
            questionnaire.clone()
        }
        other => panic!("a task input request publishes a Questionnaire, got {other:?}"),
    }
}

/// The canonical requester facts of one pending interaction.
fn requester_of(request: &InteractionRequest) -> crate::events::interaction::InteractionRequester {
    match &request.kind {
        crate::runtime::interaction::InteractionKind::Questionnaire { requester, .. } => {
            requester.clone()
        }
        other => panic!("a task input request publishes a Questionnaire, got {other:?}"),
    }
}

fn choice_options(question: &QuestionSpecification) -> &[OptionSpecification] {
    match &question.answer {
        AnswerSpecification::SingleChoice(single) => &single.options,
        other => panic!("the question is not a single-choice question: {other:?}"),
    }
}

fn option_index(question: &QuestionSpecification, label: &str) -> usize {
    choice_options(question)
        .iter()
        .position(|option| option.label == label)
        .unwrap_or_else(|| panic!("the questionnaire offers {label:?}"))
}

fn failure(result: &ToolExecutionResult) -> &str {
    match &result.status {
        ToolExecutionStatus::Failed { error } => error,
        other => panic!("expected a deterministic failure, got {other:?}"),
    }
}

fn unknown(result: &ToolExecutionResult) -> &str {
    match &result.status {
        ToolExecutionStatus::OutcomeUnknown { detail } => detail,
        other => panic!("expected an honest unknown outcome, got {other:?}"),
    }
}

fn text(result: &ToolExecutionResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match content {
            crate::tools::types::ToolResultContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Capability advertisement
// ---------------------------------------------------------------------------

/// A foreground invocation that holds the runtime-owned Questionnaire
/// authority advertises **both** capabilities on every request it sends, and
/// they are independent facts rather than one bundled claim.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_foreground_invocation_advertises_tasks_and_elicitation() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::a_foreground_invocation_advertises_tasks_and_elicitation",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(TASK_SIMPLE_TOOL, "advertise-1", &progress, true)
        .await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Success),
        "{:?}",
        result.status
    );
    let observed = harness.observed();
    assert!(!observed.is_empty(), "the fixture recorded its requests");
    for observation in &observed {
        assert!(
            observation.tasks_advertised,
            "every request of a modern invocation advertises the Tasks extension: {observation:?}"
        );
        assert!(
            observation.elicitation_advertised,
            "this invocation holds interaction authority: {observation:?}"
        );
    }
    harness.shutdown().await;
}

/// An invocation with **no** interaction authority still advertises Tasks and
/// still drives `working -> working -> completed` to a result. Remote task
/// execution and human interaction are orthogonal, and the advertisement says
/// so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invocation_without_interaction_authority_still_advertises_tasks() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::an_invocation_without_interaction_authority_still_advertises_tasks",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(TASK_SIMPLE_TOOL, "advertise-2", &progress, false)
        .await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Success),
        "a task needing no human completes without interaction authority: {:?}",
        result.status
    );
    for observation in &harness.observed() {
        assert!(
            observation.tasks_advertised,
            "Tasks is a protocol capability rustX always has: {observation:?}"
        );
        assert!(
            !observation.elicitation_advertised,
            "elicitation is never advertised merely because Tasks are: {observation:?}"
        );
    }
    assert_eq!(harness.coordinator.pending_count(), 0);
    harness.shutdown().await;
}

/// A legacy peer's requests carry no per-request capabilities at all: rustX
/// never mutates the legacy handshake to make an extension reachable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_legacy_peer_receives_no_per_request_capabilities() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::a_legacy_peer_receives_no_per_request_capabilities",
        FixtureOptions {
            legacy: true,
            ..FixtureOptions::default()
        },
    )
    .await;
    let progress = RecordingProgress::default();
    let _ = harness
        .invoke(TASK_SIMPLE_TOOL, "legacy-1", &progress, true)
        .await;
    let calls = harness.observed_method("tools/call");
    assert_eq!(calls.len(), 1, "one physical tools/call: {calls:?}");
    assert!(
        !calls[0].tasks_advertised && !calls[0].elicitation_advertised,
        "a legacy request carries no _meta client capabilities: {:?}",
        calls[0]
    );
    harness.shutdown().await;
}

/// Advertisement is authority in both directions: a peer that never declared
/// the extension does not acquire the right to hand rustX a task lifecycle by
/// simply answering with one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unadvertised_task_result_is_refused_without_being_driven() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::an_unadvertised_task_result_is_refused_without_being_driven",
        FixtureOptions {
            unadvertised: true,
            ..FixtureOptions::default()
        },
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(TASK_SIMPLE_TOOL, "unadvertised-1", &progress, true)
        .await;
    let detail = unknown(&result);
    assert!(
        detail.contains("without advertising the io.modelcontextprotocol/tasks extension"),
        "the refusal names the missing negotiation: {detail}"
    );
    assert!(
        harness.observed_method(GET_TASK).is_empty(),
        "an unnegotiated task is never polled"
    );
    harness.shutdown().await;
}

// ---------------------------------------------------------------------------
// The task lifecycle
// ---------------------------------------------------------------------------

/// The whole contract in one proof: one `tools/call` materializes one task,
/// `working -> working -> completed` is driven by rustX's own poll loop, and
/// the invocation publishes exactly one terminal result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn working_working_completed_produces_exactly_one_result() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::working_working_completed_produces_exactly_one_result",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(TASK_SIMPLE_TOOL, "simple-1", &progress, true)
        .await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Success),
        "{:?}",
        result.status
    );
    assert_eq!(text(&result), "task simple done after 3 polls");
    assert_eq!(
        harness.observed_method("tools/call").len(),
        1,
        "exactly one physical tools/call: the original call is never replayed"
    );
    let polls = harness.observed_method(GET_TASK);
    assert_eq!(polls.len(), 3, "three polls reached the server: {polls:?}");
    for poll in &polls {
        assert_eq!(poll.task_id, harness.task_id(TASK_SIMPLE_TOOL));
    }
    assert!(
        harness.observed_method(CANCEL_TASK).is_empty(),
        "a task that completes is never cancelled"
    );
    // Polling is transport activity, not Tool liveness.
    assert!(
        progress.reported().is_empty(),
        "rustX never fabricates progress from its own polls: {:?}",
        progress.reported()
    );
    assert!(
        harness.audit.events().is_empty(),
        "a task needing no human commits no interaction facts"
    );
    harness.shutdown().await;
}

/// A `CreateTaskResult` does **not** settle the invocation: the call is still
/// running when the first poll reaches its dispatch frontier.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_created_task_does_not_settle_the_invocation() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::a_created_task_does_not_settle_the_invocation",
        FixtureOptions::default(),
    )
    .await;
    let (barrier, _guard) = harness.barrier(GET_TASK);
    let progress = RecordingProgress::default();
    let call = harness.invoke(TASK_SIMPLE_TOOL, "pending-1", &progress, true);
    tokio::pin!(call);
    // The task exists — the server answered `tools/call` with it — and the
    // invocation is still pending at the first poll's dispatch frontier.
    tokio::select! {
        result = &mut call => panic!("the created task settled the invocation: {result:?}"),
        () = barrier.wait_arrived(1) => {}
    }
    assert_eq!(barrier.arrivals(), vec![harness.task_id(TASK_SIMPLE_TOOL)]);
    assert_eq!(harness.observed_method("tools/call").len(), 1);
    assert!(harness.observed_method(GET_TASK).is_empty());
    barrier.release();
    let settled = call.await;
    assert!(
        matches!(settled.status, ToolExecutionStatus::Success),
        "{:?}",
        settled.status
    );
    harness.shutdown().await;
}

/// A synchronous SEP-2322 round may precede task creation: an
/// `InputRequiredResult`, one Questionnaire, a continuation `tools/call`, and
/// only then a `CreateTaskResult`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_continuation_round_may_materialize_the_task() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::a_continuation_round_may_materialize_the_task",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(TASK_MRTR_TOOL, "mrtr-task-1", &progress, true);
    tokio::pin!(call);
    let settled = loop {
        tokio::select! {
            result = &mut call => break result,
            request = harness.next_pending() => harness.answer(&request, "stable").await,
        }
    };
    assert!(
        matches!(settled.status, ToolExecutionStatus::Success),
        "{:?}",
        settled.status
    );
    assert_eq!(text(&settled), "channel=stable");
    let calls = harness.observed_method("tools/call");
    assert_eq!(
        calls.len(),
        2,
        "one synchronous round, then the continuation that created the task: {calls:?}"
    );
    assert!(
        calls[1].input_responses.is_some(),
        "the continuation carried the typed answer: {:?}",
        calls[1]
    );
    assert!(
        !harness.observed_method(GET_TASK).is_empty(),
        "the task created by the continuation was driven"
    );
    harness.shutdown().await;
}

/// A remote task's `input_required` uses exactly the existing Questionnaire
/// owner — the same crate-private authority `ask_user` and synchronous MRTR
/// use — and its typed answer reaches the server as the exact
/// `inputResponses` map the MCP elicitation translation produced.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_input_uses_the_existing_questionnaire_owner() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::task_input_uses_the_existing_questionnaire_owner",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(TASK_INPUT_TOOL, "task-input-1", &progress, true);
    tokio::pin!(call);
    let request = tokio::select! {
        result = &mut call => panic!("the call settled before asking: {result:?}"),
        request = harness.next_pending() => request,
    };
    // The provider-independent requester facts name the MCP server, not an
    // rmcp value and not a display string.
    let requester = requester_of(&request);
    assert_eq!(
        requester.origin,
        crate::tools::types::ToolOrigin::Mcp {
            server_id: McpServerId::new(&harness.server),
        }
    );
    assert_eq!(requester.tool_name, harness.tool(TASK_INPUT_TOOL));
    let questionnaire = questionnaire_of(&request);
    assert_eq!(questionnaire.questions.len(), 1);
    harness.answer(&request, "stable").await;
    let settled = call.await;
    assert!(
        matches!(settled.status, ToolExecutionStatus::Success),
        "{:?}",
        settled.status
    );
    assert_eq!(text(&settled), "channel=stable");
    // Exactly one interaction, and the update carried the exact typed
    // protocol value — the option's own schema value, never a display label.
    assert_eq!(
        harness.audit.events().len(),
        2,
        "one requested, one settled"
    );
    let updates = harness.observed_method(UPDATE_TASK);
    assert_eq!(updates.len(), 1, "one update: {updates:?}");
    assert_eq!(
        updates[0].input_responses,
        Some(serde_json::json!({
            "ask": {"action": "accept", "content": {"channel": "stable"}},
        })),
        "the update carries the exact MRTR elicitation translation"
    );
    harness.shutdown().await;
}

/// The eventually-consistent repeat: a `tasks/get` after a successful
/// `tasks/update` may still name the key rustX just answered. It creates no
/// second interaction and sends no second update.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_input_request_creates_no_duplicate_interaction_or_update() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::a_stale_input_request_creates_no_duplicate_interaction_or_update",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(TASK_STALE_INPUT_TOOL, "stale-1", &progress, true);
    tokio::pin!(call);
    let mut asked = 0usize;
    let settled = loop {
        tokio::select! {
            result = &mut call => break result,
            request = harness.next_pending() => {
                asked += 1;
                harness.answer(&request, "beta").await;
            }
        }
    };
    assert!(
        matches!(settled.status, ToolExecutionStatus::Success),
        "{:?}",
        settled.status
    );
    assert_eq!(text(&settled), "channel=beta");
    assert_eq!(asked, 1, "the repeated key is not a fresh ask");
    assert_eq!(
        harness.observed_method(UPDATE_TASK).len(),
        1,
        "the repeated key is not answered twice on the wire"
    );
    assert!(
        harness.observed_method(GET_TASK).len() >= 4,
        "the stale snapshot was genuinely observed and polling continued"
    );
    assert_eq!(
        harness.audit.events().len(),
        2,
        "one requested, one settled"
    );
    harness.shutdown().await;
}

/// A later snapshot mixing one already-answered key with one genuinely new
/// key processes only the new outstanding work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_genuinely_new_input_request_is_still_processed() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::a_genuinely_new_input_request_is_still_processed",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(TASK_NEW_INPUT_TOOL, "new-key-1", &progress, true);
    tokio::pin!(call);
    let mut asked = Vec::new();
    let settled = loop {
        tokio::select! {
            result = &mut call => break result,
            request = harness.next_pending() => {
                let questionnaire = questionnaire_of(&request);
                asked.push(questionnaire.questions.len());
                harness.answer(&request, "stable").await;
            }
        }
    };
    assert!(
        matches!(settled.status, ToolExecutionStatus::Success),
        "{:?}",
        settled.status
    );
    assert_eq!(text(&settled), "channel=stable fallback=stable");
    assert_eq!(
        asked,
        vec![1, 1],
        "the mixed snapshot asked only about the genuinely new key"
    );
    let updates = harness.observed_method(UPDATE_TASK);
    assert_eq!(
        updates.len(),
        2,
        "one update per outstanding key: {updates:?}"
    );
    assert_eq!(
        updates[1].input_responses,
        Some(serde_json::json!({
            "second": {"action": "accept", "content": {"fallback": "stable"}},
        })),
        "the second update carries only the new key"
    );
    harness.shutdown().await;
}

// ---------------------------------------------------------------------------
// Remote terminal facts
// ---------------------------------------------------------------------------

/// A remote `failed` task is one bounded deterministic failure carrying the
/// protocol's own correlated error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_failed_task_is_one_bounded_failure() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::a_remote_failed_task_is_one_bounded_failure",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(TASK_FAILED_TOOL, "failed-1", &progress, true)
        .await;
    let error = failure(&result);
    assert!(
        error.contains("the MCP task failed") && error.contains("the fixture task failed remotely"),
        "the failure carries the task's own correlated error: {error}"
    );
    assert!(
        harness.observed_method(CANCEL_TASK).is_empty(),
        "a task that reached a terminal state is never cancelled"
    );
    harness.shutdown().await;
}

/// A remote `cancelled` task is a **protocol fact**, never a rustX
/// cancellation: no local `CancellationReason` is invented for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_cancelled_task_invents_no_local_cancellation() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::a_remote_cancelled_task_invents_no_local_cancellation",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(TASK_CANCELLED_TOOL, "remote-cancel-1", &progress, true)
        .await;
    assert!(
        !matches!(result.status, ToolExecutionStatus::Cancelled { .. }),
        "a remote protocol fact never becomes local cancellation authority: {:?}",
        result.status
    );
    let error = failure(&result);
    assert!(
        error.contains("the MCP server cancelled the task"),
        "the diagnostic names the remote authority: {error}"
    );
    assert!(
        harness.observed_method(CANCEL_TASK).is_empty(),
        "rustX did not ask for this cancellation"
    );
    harness.shutdown().await;
}

// ---------------------------------------------------------------------------
// Malformed task state
// ---------------------------------------------------------------------------

/// A snapshot describing a **different** task id is a second remote execution
/// identity trying to appear inside one invocation. It fails that invocation
/// deterministically and leaves the connection healthy.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_foreign_task_snapshot_fails_without_poisoning_the_server() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::a_foreign_task_snapshot_fails_without_poisoning_the_server",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(TASK_FOREIGN_TOOL, "foreign-1", &progress, true)
        .await;
    let detail = unknown(&result);
    assert!(
        detail.contains("with a snapshot of task"),
        "the diagnostic names both identities: {detail}"
    );
    // The peer answered, so the transport is healthy: an unrelated call on
    // the very same generation still succeeds.
    let healthy = harness
        .invoke("echo", "foreign-healthy", &progress, true)
        .await;
    assert!(
        matches!(healthy.status, ToolExecutionStatus::Success),
        "one malformed task never poisons an otherwise healthy server: {:?}",
        healthy.status
    );
    assert_eq!(harness.observed_method(CANCEL_TASK).len(), 1);
    harness.shutdown().await;
}

/// An `input_required` task naming no input request at all is contradictory:
/// it fails deterministically without publishing an interaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_contradictory_input_required_task_fails_deterministically() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::a_contradictory_input_required_task_fails_deterministically",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(TASK_EMPTY_INPUT_TOOL, "empty-input-1", &progress, true)
        .await;
    let detail = unknown(&result);
    assert!(
        detail.contains("carries no input requests"),
        "the diagnostic names the contradiction: {detail}"
    );
    assert_eq!(harness.coordinator.pending_count(), 0);
    assert!(harness.audit.events().is_empty());
    assert_eq!(harness.observed_method(CANCEL_TASK).len(), 1);
    harness.shutdown().await;
}

/// An unsupported in-task request kind is refused before anything is
/// published, exactly as it is on the synchronous MRTR path: an MCP server
/// may not initiate rustX model execution through a task either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_sampling_is_refused_without_publishing_anything() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::task_sampling_is_refused_without_publishing_anything",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(TASK_SAMPLING_TOOL, "task-sampling-1", &progress, true)
        .await;
    let detail = unknown(&result);
    assert!(
        detail.contains("MCP sampling"),
        "the refusal names the unsupported kind: {detail}"
    );
    assert_eq!(harness.coordinator.pending_count(), 0);
    assert!(harness.audit.events().is_empty());
    assert!(harness.observed_method(UPDATE_TASK).is_empty());
    assert_eq!(harness.observed_method(CANCEL_TASK).len(), 1);
    assert!(detail.contains("does not prove the remote task stopped"));
    harness.shutdown().await;
}

// ---------------------------------------------------------------------------
// Cancellation frontiers and effect certainty
// ---------------------------------------------------------------------------

/// Cancellation observed at the poll dispatch frontier prevents that
/// `tasks/get` entirely, sends exactly one cooperative `tasks/cancel`, and
/// settles as an honest unknown outcome — the remote task may still be
/// running, and rustX does not pretend otherwise.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_before_the_next_poll_dispatches_no_further_get() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::cancellation_before_the_next_poll_dispatches_no_further_get",
        FixtureOptions::default(),
    )
    .await;
    // The first poll runs to completion; the second is held at its frontier.
    let (barrier, _guard) = harness.barrier_from(GET_TASK, 2);
    let progress = RecordingProgress::default();
    let call = harness.invoke(TASK_FOREVER_TOOL, "cancel-poll-1", &progress, true);
    tokio::pin!(call);
    tokio::select! {
        result = &mut call => panic!("a never-terminating task settled: {result:?}"),
        () = barrier.wait_arrived(2) => {}
    }
    assert_eq!(
        harness.observed_method(GET_TASK).len(),
        1,
        "exactly one poll reached the server before the frontier"
    );
    assert!(
        harness
            .owner
            .request_cancel(CancellationReason::RuntimeShutdown)
    );
    barrier.release();
    let settled = call.await;
    let detail = unknown(&settled);
    assert!(
        detail.contains("cancelled"),
        "the settlement explains itself: {detail}"
    );
    assert_eq!(
        harness.observed_method(GET_TASK).len(),
        1,
        "the poll held at the frontier was never dispatched"
    );
    let cancels = harness.observed_method(CANCEL_TASK);
    assert_eq!(
        cancels.len(),
        1,
        "exactly one cooperative cancel: {cancels:?}"
    );
    assert_eq!(cancels[0].task_id, harness.task_id(TASK_FOREVER_TOOL));
    // The acknowledgement is not evidence.
    assert!(
        detail.contains("does not prove the remote task stopped"),
        "an acknowledged tasks/cancel is never remote-stop proof: {detail}"
    );
    harness.shutdown().await;
}

/// A human response that **loses** the race against cancellation cannot
/// dispatch `tasks/update`, and a late answer cannot resurrect the task.
///
/// The barrier is the exact linearization point: the typed response is
/// already accepted, mapped to `inputResponses`, and recorded as answered,
/// and the invocation is held one step before the update dispatch frontier.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_before_the_update_frontier_dispatches_no_update() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::cancellation_before_the_update_frontier_dispatches_no_update",
        FixtureOptions::default(),
    )
    .await;
    let (barrier, _guard) = harness.barrier(UPDATE_TASK);
    let progress = RecordingProgress::default();
    let call = harness.invoke(TASK_INPUT_TOOL, "cancel-update-1", &progress, true);
    tokio::pin!(call);
    let request = tokio::select! {
        result = &mut call => panic!("the call settled before asking: {result:?}"),
        request = harness.next_pending() => request,
    };
    harness.answer(&request, "stable").await;
    tokio::select! {
        result = &mut call => panic!("the call settled at the barrier: {result:?}"),
        () = barrier.wait_arrived(1) => {}
    }
    assert!(harness.observed_method(UPDATE_TASK).is_empty());
    assert!(
        harness
            .owner
            .request_cancel(CancellationReason::RuntimeShutdown)
    );
    barrier.release();
    let settled = call.await;
    let detail = unknown(&settled);
    assert!(
        detail.contains("cancelled"),
        "the settlement explains itself: {detail}"
    );
    assert!(
        harness.observed_method(UPDATE_TASK).is_empty(),
        "the late human response created no remote update"
    );
    assert_eq!(harness.observed_method(CANCEL_TASK).len(), 1);
    // A late answer finds nothing to resurrect.
    assert_eq!(harness.coordinator.pending_count(), 0);
    assert!(
        harness
            .coordinator
            .respond_async(
                &request.id,
                InteractionResponse::Questionnaire {
                    response: QuestionnaireResponse::Declined,
                },
            )
            .await
            .is_err(),
        "a late answer finds no pending interaction"
    );
    assert!(harness.observed_method(UPDATE_TASK).is_empty());
    harness.shutdown().await;
}

/// Cancellation racing the poll that would have completed the task has one
/// deterministic terminal winner and exactly one published result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_before_completing_poll_frontier_prevents_dispatch() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::cancellation_before_completing_poll_frontier_prevents_dispatch",
        FixtureOptions::default(),
    )
    .await;
    // Two polls run to completion; the third — the one that would complete
    // the task — is held at its dispatch frontier.
    let (barrier, _guard) = harness.barrier_from(GET_TASK, 3);
    let progress = RecordingProgress::default();
    let call = harness.invoke(TASK_SIMPLE_TOOL, "race-complete-1", &progress, true);
    tokio::pin!(call);
    tokio::select! {
        result = &mut call => panic!("the call settled early: {result:?}"),
        () = barrier.wait_arrived(3) => {}
    }
    assert_eq!(harness.observed_method(GET_TASK).len(), 2);
    assert!(
        harness
            .owner
            .request_cancel(CancellationReason::RuntimeShutdown)
    );
    barrier.release();
    let settled = call.await;
    // Deterministic winner: cancellation won the frontier, so the completing
    // poll was never dispatched and the remote outcome is genuinely unknown.
    let detail = unknown(&settled);
    assert!(detail.contains("cancelled"), "{detail}");
    assert_eq!(
        harness.observed_method(GET_TASK).len(),
        2,
        "the completing poll was never dispatched"
    );
    assert_eq!(harness.observed_method(CANCEL_TASK).len(), 1);
    harness.shutdown().await;
}

/// A remote task the runtime can no longer reach settles through the existing
/// bounded failure semantics, and the original `tools/call` is **never**
/// replayed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_loss_after_task_creation_never_replays_the_tool_call() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::connection_loss_after_task_creation_never_replays_the_tool_call",
        FixtureOptions::default(),
    )
    .await;
    // One poll succeeds, then the next is held at its dispatch frontier.
    let (barrier, _guard) = harness.barrier_from(GET_TASK, 2);
    let progress = RecordingProgress::default();
    let call = harness.invoke(TASK_FOREVER_TOOL, "lost-1", &progress, true);
    tokio::pin!(call);
    tokio::select! {
        result = &mut call => panic!("the call settled before polling: {result:?}"),
        () = barrier.wait_arrived(2) => {}
    }
    // The generation goes away while the task is active and no request is in
    // flight: the invocation holds no call gate at a dispatch frontier.
    let closing = harness.runtime.close();
    tokio::pin!(closing);
    let closed = tokio::select! {
        result = &mut closing => result,
        () = std::future::pending::<()>() => unreachable!(),
    };
    assert!(closed.is_ok(), "the fixture generation closes: {closed:?}");
    barrier.release();
    let settled = call.await;
    let detail = unknown(&settled);
    assert!(
        detail.contains("materialized remote task"),
        "the settlement names the task whose outcome is unknown: {detail}"
    );
    assert_eq!(
        harness.observed_method("tools/call").len(),
        1,
        "a lost generation never replays the original call"
    );
    assert_eq!(
        harness.observed_method(GET_TASK).len(),
        1,
        "no poll was dispatched after the generation closed"
    );
    assert!(
        harness.observed_method(CANCEL_TASK).is_empty(),
        "closed generation cannot carry cancellation"
    );
}

// ---------------------------------------------------------------------------
// Foreground / background orthogonality
// ---------------------------------------------------------------------------

/// A task admitted for rustX background execution is driven by the **existing**
/// background owner: one detached identity, one settlement, no second
/// background record, and no interaction authority.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_background_execution_drives_the_task_through_its_existing_owner() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::a_background_execution_drives_the_task_through_its_existing_owner",
        FixtureOptions::default(),
    )
    .await;
    let directory = tempfile::tempdir().expect("background root");
    let workspace_root = directory.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let artifacts = directory.path().join("artifacts");
    let conversation = ConversationId::new("tasks-background");
    let mailbox = ConversationInboundMailbox::new(conversation.clone());
    let lifecycle = ConversationLifecycle::new();
    mailbox.bind_inactive(&lifecycle);
    assert!(lifecycle.activate());
    let registry = ConversationBackgroundRegistry::new(
        conversation.clone(),
        BackgroundResources {
            mailbox,
            workspace: Workspace::new(&workspace_root).expect("workspace"),
            artifacts: crate::tools::artifacts::ArtifactStore::new(
                conversation.clone(),
                &artifacts,
            )
            .expect("artifacts"),
            tool_output: crate::tools::managed_output::ManagedToolOutput::new(
                conversation,
                artifacts.join("tool-output"),
            )
            .expect("managed tool output"),
            clock: Arc::new(crate::runtime::SystemClock),
            event_sink: None,
        },
    );
    let executor: Arc<dyn ToolExecutor> = Arc::new(crate::tools::mcp::McpToolExecutor::new(
        Arc::clone(&harness.runtime),
        harness.tool(TASK_SIMPLE_TOOL),
    ));
    let invocation = ToolInvocation {
        id: ToolInvocationId::Agent {
            call_id: ToolCallId::new("background-task"),
        },
        tool_id: crate::runtime::identity::ToolId::new("mcp:tasks"),
        tool_name: harness.tool(TASK_SIMPLE_TOOL),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({"subject": "release"}),
    };
    let prepared = registry
        .prepare_dispatch(
            &invocation,
            &executor,
            crate::tools::environment::ToolEnvironment::new(),
        )
        .expect("the background dispatch prepares");
    let BackgroundDispatchOutcome::Accepted { execution_id, .. } = registry
        .commit_dispatch(prepared, &crate::runtime::CancellationSignal::new())
        .expect("the background dispatch commits")
    else {
        panic!("accepted");
    };
    let snapshot = registry
        .wait_until_terminal(&execution_id)
        .await
        .expect("the detached execution settles");
    // The same detached identity throughout, and exactly one settlement.
    assert_eq!(snapshot.execution_id, execution_id);
    let result = snapshot.result.expect("one terminal result");
    assert!(
        matches!(result.status, ToolExecutionStatus::Success),
        "the background owner drove the whole task lifecycle: {:?}",
        result.status
    );
    // The remote task id never became a rustX identity of any kind.
    let task_id = harness.task_id(TASK_SIMPLE_TOOL);
    assert_ne!(execution_id.to_string(), task_id);
    let published = format!("{:?}{:?}", result.status, result.content);
    assert!(
        !published.contains(&task_id),
        "the remote task id never reaches a published fact: {published}"
    );
    for observation in &harness.observed() {
        assert!(
            observation.tasks_advertised,
            "a detached execution still drives tasks: {observation:?}"
        );
        assert!(
            !observation.elicitation_advertised,
            "a detached execution never advertises interaction authority: {observation:?}"
        );
    }
    assert_eq!(harness.coordinator.pending_count(), 0);
    assert!(harness.audit.events().is_empty());
    harness.shutdown().await;
}

/// A background task that asks for a human fails honestly instead of creating
/// hidden pending interaction state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_task_asking_a_detached_execution_for_a_human_fails_honestly() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::a_task_asking_a_detached_execution_for_a_human_fails_honestly",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(TASK_INPUT_TOOL, "no-authority-task", &progress, false)
        .await;
    let detail = unknown(&result);
    assert!(
        detail.contains("no runtime-owned interaction authority"),
        "the refusal is explicit about ownership: {detail}"
    );
    assert_eq!(harness.coordinator.pending_count(), 0);
    assert!(harness.audit.events().is_empty());
    assert!(harness.observed_method(UPDATE_TASK).is_empty());
    // The remote task is still the server's, so rustX asks it to stop and
    // reports an unknown outcome rather than a proven failure of the effect.
    assert_eq!(harness.observed_method(CANCEL_TASK).len(), 1);
    assert!(detail.contains("does not prove the remote task stopped"));
    harness.shutdown().await;
}

// ---------------------------------------------------------------------------
// No protocol state escapes
// ---------------------------------------------------------------------------

/// The remote task id is protocol state on one stack frame: it never becomes
/// a `ToolExecutionId`, a canonical message, or a durable interaction fact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_remote_task_state_escapes_into_any_published_fact() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::no_remote_task_state_escapes_into_any_published_fact",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(TASK_INPUT_TOOL, "leak-1", &progress, true);
    tokio::pin!(call);
    let settled = loop {
        tokio::select! {
            result = &mut call => break result,
            request = harness.next_pending() => harness.answer(&request, "stable").await,
        }
    };
    assert!(matches!(settled.status, ToolExecutionStatus::Success));
    let task_id = harness.task_id(TASK_INPUT_TOOL);
    let markers = [
        task_id.as_str(),
        "taskId",
        "task_id",
        "inputRequests",
        "inputResponses",
        "input_required",
        "pollIntervalMs",
    ];
    let result_text = format!("{:?}{:?}", settled.status, settled.content);
    for marker in markers {
        assert!(
            !result_text.contains(marker),
            "the terminal result leaks {marker}: {result_text}"
        );
    }
    let committed = format!("{:?}", harness.audit.committed());
    for marker in markers {
        assert!(
            !committed.contains(marker),
            "a durable interaction fact leaks {marker}: {committed}"
        );
    }
    assert!(
        progress.reported().is_empty(),
        "polling reported no progress: {:?}",
        progress.reported()
    );
    harness.shutdown().await;
}

// ---------------------------------------------------------------------------
// Streamable HTTP: a task request is rustX-owned local activity
// ---------------------------------------------------------------------------

/// Cancellation while a `tasks/get` HTTP request is genuinely in flight
/// terminates **that** request's own local participant and proves it released
/// before the invocation settles.
///
/// This is the regression the widened request-ownership seam exists for: a
/// task-control request is not a `tools/call`, and before Issue #243 it would
/// have been an untracked request whose in-flight POST no settlement could
/// terminate.
///
/// # Synchronization proof
///
/// - `wait_task_gets` resolves only once the server's `tasks/get` handler was
///   entered, which is strictly stronger than the effect frontier, so
///   everything after it is provably post-frontier;
/// - the handler emits nothing before it is released, so the HTTP response
///   headers of that `tasks/get` are genuinely still outstanding;
/// - `wait_terminated` is the **server-side** proof of rustX's local
///   ownership settlement: rmcp's Streamable HTTP server cancels a handler
///   that has emitted nothing when the client disconnects its HTTP request,
///   so this count rises only because rustX actually dropped its in-flight
///   HTTP request and its socket closed.
///
/// No wall clock is waited on anywhere in this test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_releases_the_in_flight_http_task_request() {
    use crate::tools::mcp::fixture::streamable_http::{HttpFixture, HttpFixtureControl};

    let server = HttpFixture::start(HttpFixtureControl::new()).await;
    let directory = tempfile::tempdir().expect("http root");
    let workspace_root = directory.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let workspace = Workspace::new(&workspace_root).expect("workspace");
    let runtime = McpServerRuntime::connect(
        &McpServerId::new("tasks-http"),
        &server.binding(),
        &workspace,
        Arc::new(McpInvalidationState::new()),
    )
    .await
    .expect("the HTTP fixture connects");
    let cancel_release = runtime.hold_http_release(CANCEL_TASK);
    let tool_runtime = ConversationToolRuntime::new(
        ConversationId::new("tasks-http"),
        workspace_root,
        directory.path().join("artifacts"),
    )
    .expect("tool runtime");
    let owner =
        crate::agent::cancellation::AgentCancellation::new(CancellationReason::UserRequested);
    let progress = RecordingProgress::default();
    let executor =
        crate::tools::mcp::McpToolExecutor::new(Arc::clone(&runtime), server.control.task());
    let context = ToolExecutionContext::new(
        tool_runtime.conversation_id(),
        None,
        owner.execution_cancellation(),
        tool_runtime.workspace(),
        &progress,
        tool_runtime.artifacts(),
        tool_runtime.tool_output(),
        tool_runtime.environment(),
    );
    let call = executor
        .start(
            ToolInvocation {
                id: ToolInvocationId::Agent {
                    call_id: ToolCallId::new("http-task-1"),
                },
                tool_id: crate::runtime::identity::ToolId::new("mcp:tasks-http"),
                tool_name: server.control.task(),
                mode: ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            context,
        )
        .completion;
    tokio::pin!(call);
    // The task exists and its first poll is a real, outstanding HTTP request.
    tokio::select! {
        result = &mut call => panic!("the call settled before polling: {result:?}"),
        () = server.control.wait_task_gets(1) => {}
    }
    let poll = runtime
        .http_request_states()
        .into_iter()
        .find(|(_, method, _, _)| method.as_deref() == Some(GET_TASK))
        .expect("in-flight poll identity");
    assert_eq!(poll.2, "HttpOwned");
    assert!(
        owner.request_cancel(CancellationReason::RuntimeShutdown),
        "cancellation is requested while the poll is in flight"
    );
    tokio::select! {
        result = &mut call => panic!("settled before cancel HTTP release: {result:?}; outstanding {:?}", runtime.http_request_states()),
        () = cancel_release.wait_held_and_observed() => {}
    }
    server.control.wait_terminated(1).await;
    let cancel_id = cancel_release.request_id();
    assert_ne!(poll.0, cancel_id);
    assert_eq!(
        runtime.http_request_states(),
        vec![(
            cancel_id.clone(),
            Some(CANCEL_TASK.to_owned()),
            "HttpOwned".to_owned(),
            true
        )]
    );
    eprintln!(
        "poll {:?} released; cancel {cancel_id:?} ACK observed, HttpOwned and admitted",
        poll.0
    );
    assert!(
        futures_util::FutureExt::now_or_never(call.as_mut()).is_none(),
        "ACK is not local release"
    );
    cancel_release.release();
    let settled = call.await;
    let detail = unknown(&settled);
    assert!(
        detail.contains("does not prove the remote task stopped"),
        "an acknowledged cooperative cancel is never remote-stop proof: {detail}"
    );
    assert_eq!(
        server.control.task_gets(),
        1,
        "no poll was dispatched after the terminal decision"
    );
    assert_eq!(
        server.control.task_cancels(),
        1,
        "exactly one cooperative tasks/cancel, on the session that survived \
         rustX aborting its own request"
    );
    assert_eq!(
        runtime.outstanding_http_requests(),
        0,
        "no request lifecycle state outlives the settled invocation"
    );
    let _ = runtime.close().await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tasks_update_rejects_an_unrelated_success() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::tasks_update_rejects_an_unrelated_success",
        FixtureOptions {
            wrong_ack: Some(UPDATE_TASK),
            ..FixtureOptions::default()
        },
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(TASK_INPUT_TOOL, "wrong-update", &progress, true);
    tokio::pin!(call);
    let pending = tokio::select! {
        result = &mut call => panic!("premature settlement {result:?}"),
        pending = harness.next_pending() => pending,
    };
    harness.answer(&pending, "stable").await;
    let settled = call.await;
    assert!(unknown(&settled).contains("expected TaskAckResult"));
    assert_eq!(harness.observed_method(UPDATE_TASK).len(), 1);
    assert_eq!(harness.observed_method(GET_TASK).len(), 1);
    assert_eq!(harness.observed_method(CANCEL_TASK).len(), 1);
    assert!(harness.runtime.unusable_reason().is_none());
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tasks_cancel_rejects_an_unrelated_success() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::tasks_cancel_rejects_an_unrelated_success",
        FixtureOptions {
            wrong_ack: Some(CANCEL_TASK),
            ..FixtureOptions::default()
        },
    )
    .await;
    let progress = RecordingProgress::default();
    let settled = harness
        .invoke(TASK_INPUT_TOOL, "wrong-cancel", &progress, false)
        .await;
    let detail = unknown(&settled);
    assert!(detail.contains("produced no acknowledgement"));
    assert!(!detail.contains("server acknowledged"));
    assert_eq!(harness.observed_method(CANCEL_TASK).len(), 1);
    assert_eq!(harness.observed_method(GET_TASK).len(), 1);
    assert!(harness.runtime.unusable_reason().is_none());
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn correlated_completion_after_frontier_outranks_cancellation() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("gate listener");
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::correlated_completion_after_frontier_outranks_cancellation",
        FixtureOptions {
            completing_gate: Some(listener.local_addr().expect("address").to_string()),
            ..FixtureOptions::default()
        },
    )
    .await;
    // Freeze only the completing request's arbitration. It remains a real
    // in-flight stdio request, and the ordinary biased response arbitration
    // runs once both competing facts are observable.
    let (barrier, _guard) = harness.barrier_from("tasks/get:correlated", 3);
    let progress = RecordingProgress::default();
    let call = harness.invoke(TASK_SIMPLE_TOOL, "post-frontier", &progress, true);
    tokio::pin!(call);
    let (mut gate, _) = tokio::select! {
        result = &mut call => panic!("premature settlement {result:?}"),
        connection = listener.accept() => connection.expect("completing handler"),
    };
    assert_eq!(gate.read_u8().await.expect("entered"), 1);
    assert_eq!(harness.observed_method(GET_TASK).len(), 3);
    assert!(
        harness
            .owner
            .request_cancel(CancellationReason::UserRequested)
    );
    gate.write_all(&[1])
        .await
        .expect("release terminal response");
    barrier.release();
    let settled = call.await;
    assert!(matches!(settled.status, ToolExecutionStatus::Success));
    assert_eq!(text(&settled), "task simple done after 3 polls");
    assert_eq!(harness.observed_method("tools/call").len(), 1);
    assert_eq!(harness.observed_method(GET_TASK).len(), 3);
    assert!(harness.observed_method(UPDATE_TASK).is_empty());
    assert!(harness.observed_method(CANCEL_TASK).is_empty());
    assert!(progress.reported().is_empty());
    harness.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_creation_metadata_still_cancels_the_trusted_task() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_tasks::invalid_creation_metadata_still_cancels_the_trusted_task",
        FixtureOptions {
            oversized_seed: true,
            ..FixtureOptions::default()
        },
    )
    .await;
    let progress = RecordingProgress::default();
    let settled = harness
        .invoke(TASK_SIMPLE_TOOL, "oversized-seed", &progress, true)
        .await;
    let detail = unknown(&settled);
    assert!(detail.contains("status message"));
    assert!(detail.contains("does not prove the remote task stopped"));
    assert_eq!(harness.observed_method(CANCEL_TASK).len(), 1);
    assert!(harness.observed_method(GET_TASK).is_empty());
    assert!(harness.observed_method(UPDATE_TASK).is_empty());
    assert!(harness.runtime.unusable_reason().is_none());
    harness.shutdown().await;
}
