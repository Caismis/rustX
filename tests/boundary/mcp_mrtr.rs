//! Deterministic MCP multi-round-trip execution regressions (Issue #242).
//!
//! Every test here drives a **real** MCP peer — the self-spawned stdio
//! fixture, over the ordinary `McpServerRuntime` transport and the ordinary
//! [`ToolExecutor`] boundary — so the proofs are about the shipped execution
//! path and not about a mock of it. The fixture's guard tools follow the
//! modern SEP-2322 shape: a round *returns* an `InputRequiredResult`, and a
//! later round observes the client's `inputResponses` and echoed
//! `requestState` in its own request params. No test uses a sleep as a
//! correctness proof: the races are decided through the coordinator's own
//! pending-publication observer, the MRTR continuation barrier, and the
//! fixture's own server-side notifications.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::durable::TranscriptCursor;
use crate::events::RuntimeEventEnvelope;
use crate::events::interaction::{
    QuestionnaireAnswer, QuestionnaireAnswerEntry, QuestionnaireResponse, QuestionnaireSubmission,
    SingleOptionAnswer,
};
use crate::runtime::identity::{AttemptId, ConversationId, InteractionId, McpServerId, ToolCallId};
use crate::runtime::interaction::{
    InteractionCoordinator, InteractionObserver, InteractionRequest, InteractionResponse,
    QuestionnaireRequester, RecordingInteractionAudit,
};
use crate::runtime::types::{CancellationReason, ConversationLifecycle};
use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
use crate::tools::mcp::MCP_MRTR_MAX_ROUNDS;
use crate::tools::mcp::fixture::{
    FIXTURE_MODE_ENV, FixtureServer, MRTR_CONFIRM_TOOL, MRTR_MIXED_TOOL, MRTR_MULTI_TOOL,
    MRTR_OBSERVATION_FILE_ENV, MRTR_OVERSIZED_STATE_TOOL, MRTR_PROGRESS_TOOL, MRTR_ROOTS_TOOL,
    MRTR_ROUNDS_ENV, MRTR_SAMPLING_TOOL, MRTR_SLOW_CONTINUATION_TOOL, MRTR_STATE_ONLY_TOOL,
    MRTR_TOOLS_ENV, MRTR_UNSUPPORTED_SCHEMA_TOOL, MrtrObservation, TOOL_PREFIX_ENV,
    fixture_round_state, fixture_spawn_args, mrtr_observations, serve_if_fixture_mode,
};
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
use crate::runtime::interaction::{
    InteractionAdmissionError, InteractionPublicationPermit, InteractionRef, InteractionRoute,
    InteractionRouteError, InteractionRouteEvent,
};
use crate::tools::background::{
    BackgroundDispatchOutcome, BackgroundResources, ConversationBackgroundRegistry,
};

/// Counts how many times the Tool Plane entered the executor boundary.
///
/// One model `ToolCall` must be one `ToolExecutor::start`, whatever the MRTR
/// round count: that is what makes "approval is evaluated once, not once per
/// round" true, because the Agent Loop's pre-tool policy runs before `start`.
struct CountingExecutor {
    inner: crate::tools::mcp::McpToolExecutor,
    starts: Arc<std::sync::atomic::AtomicUsize>,
}

impl ToolExecutor for CountingExecutor {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        self.starts
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        self.inner.start(invocation, context)
    }

    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        self.inner.progress_capability()
    }
}

/// Records every route event a child coordinator sends to its root.
#[derive(Default)]
struct RecordingRoute {
    events: Mutex<Vec<String>>,
}

impl RecordingRoute {
    fn kinds(&self) -> Vec<String> {
        self.events.lock().expect("route lock").clone()
    }
}

impl InteractionRoute for RecordingRoute {
    fn admit_publication(
        &self,
        interaction: InteractionRef,
    ) -> futures_util::future::BoxFuture<
        'static,
        Result<InteractionPublicationPermit, InteractionAdmissionError>,
    > {
        self.events
            .lock()
            .expect("route lock")
            .push("admit".to_owned());
        Box::pin(async move { Ok(InteractionPublicationPermit::for_interaction(interaction)) })
    }

    fn publish(
        &self,
        event: InteractionRouteEvent,
    ) -> futures_util::future::BoxFuture<'static, Result<(), InteractionRouteError>> {
        self.events.lock().expect("route lock").push(match event {
            InteractionRouteEvent::Requested(_) => "requested".to_owned(),
            InteractionRouteEvent::Settled { .. } => "settled".to_owned(),
        });
        Box::pin(async { Ok(()) })
    }

    fn try_publish(&self, _event: InteractionRouteEvent) -> Result<(), InteractionRouteError> {
        Ok(())
    }

    fn try_admit_publication(
        &self,
        interaction: InteractionRef,
    ) -> Result<InteractionPublicationPermit, InteractionAdmissionError> {
        Ok(InteractionPublicationPermit::for_interaction(interaction))
    }
}

/// A progress reporter that keeps every reported fact **and** republishes it
/// on a channel.
///
/// The channel is what makes "the remote round is executing server-side" an
/// observable fact in this process: the fixture emits a genuine progress
/// notification at that point, and rustX forwards it through the one generic
/// progress seam. No polling, and no sleep.
struct RecordingProgress {
    reported: Mutex<Vec<ToolProgress>>,
    sender: tokio::sync::mpsc::UnboundedSender<ToolProgress>,
    receiver: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<ToolProgress>>,
}

impl Default for RecordingProgress {
    fn default() -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            reported: Mutex::new(Vec::new()),
            sender,
            receiver: tokio::sync::Mutex::new(receiver),
        }
    }
}

impl ProgressReporter for RecordingProgress {
    fn report(&self, progress: ToolProgress) {
        self.reported
            .lock()
            .expect("progress lock")
            .push(progress.clone());
        let _ = self.sender.send(progress);
    }
}

impl RecordingProgress {
    fn reported(&self) -> Vec<ToolProgress> {
        self.reported.lock().expect("progress lock").clone()
    }

    /// Resolves once the next remote progress fact arrives.
    async fn next(&self) -> ToolProgress {
        self.receiver
            .lock()
            .await
            .recv()
            .await
            .expect("one remote progress fact")
    }
}

/// Publishes every newly pending interaction id on a channel, so a test can
/// answer exactly the interaction the executor published — never a guessed
/// one, and never after a poll loop.
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
    rounds: Option<usize>,
}

/// The per-test model-facing tool-name prefix.
///
/// Every deterministic seam in this crate's MCP suite is keyed by the scoped
/// tool name, and the whole suite runs in one binary, so a fixture must mint
/// names that belong to exactly one test. The prefix is derived from the
/// test's own path.
fn tool_prefix(test_name: &str) -> String {
    let leaf = test_name.rsplit("::").next().unwrap_or(test_name);
    format!("{leaf}__")
}

impl Harness {
    /// Connects the self-spawned stdio fixture with the MRTR guard tools
    /// published, and builds a live interaction coordinator with an admitted
    /// provider.
    async fn connect(test_name: &str, options: FixtureOptions) -> Self {
        let directory = tempfile::tempdir().expect("fixture root");
        let workspace_root = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let observations = directory.path().join("mrtr-observations.jsonl");
        let prefix = tool_prefix(test_name);
        let mut environment = std::collections::BTreeMap::from([
            (FIXTURE_MODE_ENV.to_owned(), "1".to_owned()),
            (MRTR_TOOLS_ENV.to_owned(), "1".to_owned()),
            (TOOL_PREFIX_ENV.to_owned(), prefix.clone()),
            (
                MRTR_OBSERVATION_FILE_ENV.to_owned(),
                observations.display().to_string(),
            ),
        ]);
        if let Some(rounds) = options.rounds {
            environment.insert(MRTR_ROUNDS_ENV.to_owned(), rounds.to_string());
        }
        let binding = McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            activation: crate::capabilities::activation::SourceActivation::Enabled,
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
            &McpServerId::new("mrtr-fixture"),
            &binding,
            &workspace,
            Arc::new(McpInvalidationState::new()),
        )
        .await
        .expect("the fixture connects");
        // The whole feature is 2026-07-28 only, so every proof below is
        // qualified by the revision this connection actually negotiated.
        assert_eq!(
            runtime.protocol_version().as_str(),
            rmcp::model::ProtocolVersion::V_2026_07_28.as_str(),
            "MRTR requires the modern negotiated revision"
        );
        let tool_runtime = ConversationToolRuntime::new(
            ConversationId::new("mrtr"),
            workspace_root,
            directory.path().join("artifacts"),
        )
        .expect("tool runtime");
        let lifecycle = ConversationLifecycle::new();
        assert!(lifecycle.activate());
        let conversation_id = ConversationId::new("mrtr");
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
            AttemptId::new("mrtr-attempt"),
            self.owner.execution_cancellation(),
            7,
        )
    }

    /// Starts one invocation of `tool` through the canonical `ToolExecutor`
    /// boundary, with the runtime-owned interaction authority attached.
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
            tool_id: crate::runtime::identity::ToolId::new("mcp:mrtr"),
            tool_name: tool.to_owned(),
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
    async fn answer(&self, id: &InteractionId, label: &str) {
        self.coordinator
            .respond_async(
                id,
                InteractionResponse::Questionnaire {
                    response: QuestionnaireResponse::Submitted(QuestionnaireSubmission {
                        answers: vec![QuestionnaireAnswerEntry {
                            question_index: 0,
                            answer: QuestionnaireAnswer::SingleOption(SingleOptionAnswer {
                                label: label.to_owned(),
                            }),
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

    fn rounds(&self) -> Vec<MrtrObservation> {
        mrtr_observations(&self.observations)
    }

    async fn shutdown(&self) {
        let _ = self.runtime.close().await;
    }
}

fn failure(result: &ToolExecutionResult) -> &str {
    match &result.status {
        ToolExecutionStatus::Failed { error } => error,
        other => panic!("expected a deterministic failure, got {other:?}"),
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

/// An ordinary one-round MCP tool keeps its pre-#242 behaviour exactly: one
/// remote round, no interaction, one terminal result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ordinary_one_round_tool_publishes_no_interaction() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::an_ordinary_one_round_tool_publishes_no_interaction",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness.invoke("echo", "echo-1", &progress, true).await;
    assert!(matches!(result.status, ToolExecutionStatus::Success));
    assert_eq!(harness.coordinator.pending_count(), 0);
    assert!(harness.rounds().is_empty(), "echo is not an MRTR tool");
    assert!(
        harness.audit.events().is_empty(),
        "an ordinary call commits no interaction facts"
    );
    harness.shutdown().await;
}

/// The whole MRTR contract in one proof: one `ToolCall`, one invocation, two
/// remote rounds, exactly one runtime interaction, one terminal result — and
/// the opaque `requestState` comes back byte-identical while the business
/// arguments never change.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_supported_elicitation_drives_exactly_one_interaction_and_one_result() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::one_supported_elicitation_drives_exactly_one_interaction_and_one_result",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(MRTR_CONFIRM_TOOL, "confirm-1", &progress, true);
    tokio::pin!(call);
    let result = tokio::select! {
        result = &mut call => panic!("the call settled before asking: {result:?}"),
        request = harness.next_pending() => request,
    };
    // Exactly one interaction, and it is a Questionnaire owned by this
    // invocation.
    assert_eq!(harness.coordinator.pending_count(), 1);
    let crate::runtime::interaction::InteractionKind::Questionnaire {
        invocation_id,
        questionnaire,
    } = &result.kind
    else {
        panic!("MRTR publishes a Questionnaire, got {:?}", result.kind);
    };
    assert_eq!(
        invocation_id,
        &ToolInvocationId::Agent {
            call_id: ToolCallId::new("confirm-1"),
        },
        "the interaction names the one invocation that is running"
    );
    assert_eq!(questionnaire.questions.len(), 1);
    assert_eq!(
        questionnaire.questions[0]
            .options
            .iter()
            .map(|option| option.label.as_str())
            .collect::<Vec<_>>(),
        vec!["stable", "beta"]
    );
    harness.answer(&result.id, "beta").await;
    let settled = call.await;
    assert!(matches!(settled.status, ToolExecutionStatus::Success));
    assert_eq!(text(&settled), "rounds=2 choice=beta");
    assert_eq!(harness.coordinator.pending_count(), 0);

    let rounds = harness.rounds();
    assert_eq!(rounds.len(), 2, "exactly two physical tools/call rounds");
    assert_eq!(rounds[0].request_state, None);
    assert!(
        rounds[0].elicitation_advertised,
        "an invocation with interaction authority advertises elicitation per request"
    );
    // The opaque continuation state round-trips byte for byte.
    assert_eq!(
        rounds[1].request_state.as_deref(),
        Some(fixture_round_state(1).as_str())
    );
    assert_eq!(
        rounds[1].input_responses,
        Some(serde_json::json!({"ask": {"action": "accept", "content": {"channel": "beta"}}}))
    );
    // The model-issued business arguments are re-sent unchanged and never
    // carry continuation metadata.
    assert_eq!(
        rounds[0].arguments,
        serde_json::json!({"subject": "release"})
    );
    assert_eq!(rounds[1].arguments, rounds[0].arguments);
    harness.shutdown().await;
}

/// Two input requests in one round produce one coherent questionnaire whose
/// answers map back by **server key**, not by position.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn several_input_requests_in_one_round_stay_one_questionnaire() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::several_input_requests_in_one_round_stay_one_questionnaire",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(MRTR_MULTI_TOOL, "multi-1", &progress, true);
    tokio::pin!(call);
    let request = tokio::select! {
        result = &mut call => panic!("the call settled before asking: {result:?}"),
        request = harness.next_pending() => request,
    };
    assert_eq!(harness.coordinator.pending_count(), 1, "one questionnaire");
    let crate::runtime::interaction::InteractionKind::Questionnaire { questionnaire, .. } =
        &request.kind
    else {
        panic!("MRTR publishes a Questionnaire");
    };
    assert_eq!(questionnaire.questions.len(), 2);
    harness
        .coordinator
        .respond_async(
            &request.id,
            InteractionResponse::Questionnaire {
                response: QuestionnaireResponse::Submitted(QuestionnaireSubmission {
                    answers: vec![
                        QuestionnaireAnswerEntry {
                            question_index: 0,
                            answer: QuestionnaireAnswer::SingleOption(SingleOptionAnswer {
                                label: "stable".to_owned(),
                            }),
                        },
                        QuestionnaireAnswerEntry {
                            question_index: 1,
                            answer: QuestionnaireAnswer::SingleOption(SingleOptionAnswer {
                                label: "beta".to_owned(),
                            }),
                        },
                    ],
                }),
            },
        )
        .await
        .expect("both answers are accepted");
    let settled = call.await;
    assert!(matches!(settled.status, ToolExecutionStatus::Success));
    // "first" asked about `channel` and "second" about `fallback`: the
    // fixture echoes them separately, so a positional mapping would swap.
    assert_eq!(text(&settled), "stable+beta");
    let rounds = harness.rounds();
    assert_eq!(rounds.len(), 2);
    assert_eq!(
        rounds[1].input_responses,
        Some(serde_json::json!({
            "first": {"action": "accept", "content": {"channel": "stable"}},
            "second": {"action": "accept", "content": {"fallback": "beta"}},
        }))
    );
    harness.shutdown().await;
}

/// A `requestState`-only round is a legitimate continuation that asks nothing:
/// it re-dispatches with the exact opaque state and publishes no interaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_state_only_round_continues_without_any_interaction() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::a_state_only_round_continues_without_any_interaction",
        FixtureOptions { rounds: Some(3) },
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(MRTR_STATE_ONLY_TOOL, "state-only-1", &progress, true)
        .await;
    assert!(matches!(result.status, ToolExecutionStatus::Success));
    assert_eq!(text(&result), "state-only rounds: 3");
    assert_eq!(harness.coordinator.pending_count(), 0);
    assert!(
        harness.audit.events().is_empty(),
        "no interaction fact is committed for a state-only continuation"
    );
    let rounds = harness.rounds();
    assert_eq!(rounds.len(), 3);
    assert_eq!(
        rounds[2].request_state.as_deref(),
        Some(fixture_round_state(2).as_str())
    );
    harness.shutdown().await;
}

/// The invocation keeps one identity across the maximum permitted number of
/// rounds, and one more `input_required` fails deterministically without ever
/// publishing the interaction that would have gone with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_round_bound_admits_the_maximum_and_refuses_one_more() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    // At the bound: `MCP_MRTR_MAX_ROUNDS` physical rounds succeed.
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::the_round_bound_admits_the_maximum_and_refuses_one_more",
        FixtureOptions {
            rounds: Some(MCP_MRTR_MAX_ROUNDS),
        },
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(MRTR_CONFIRM_TOOL, "bound-1", &progress, true);
    tokio::pin!(call);
    let mut answered = 0usize;
    let settled = loop {
        tokio::select! {
            result = &mut call => break result,
            request = harness.next_pending() => {
                answered += 1;
                harness.answer(&request.id, "stable").await;
            }
        }
    };
    assert!(matches!(settled.status, ToolExecutionStatus::Success));
    assert_eq!(answered, MCP_MRTR_MAX_ROUNDS - 1);
    assert_eq!(harness.rounds().len(), MCP_MRTR_MAX_ROUNDS);
    harness.shutdown().await;
}

/// One `input_required` past the bound is refused, and the refusal happens
/// before the interaction it would have needed is published.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_round_past_the_bound_fails_before_publishing_an_interaction() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::one_round_past_the_bound_fails_before_publishing_an_interaction",
        FixtureOptions {
            rounds: Some(MCP_MRTR_MAX_ROUNDS + 1),
        },
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(MRTR_CONFIRM_TOOL, "bound-2", &progress, true);
    tokio::pin!(call);
    let mut answered = 0usize;
    let settled = loop {
        tokio::select! {
            result = &mut call => break result,
            request = harness.next_pending() => {
                answered += 1;
                harness.answer(&request.id, "stable").await;
            }
        }
    };
    let error = failure(&settled);
    assert!(
        error.contains("multi-round-trip bound"),
        "the bound is named in the diagnostic: {error}"
    );
    assert_eq!(
        answered,
        MCP_MRTR_MAX_ROUNDS - 1,
        "the refused round publishes no interaction of its own"
    );
    assert_eq!(harness.rounds().len(), MCP_MRTR_MAX_ROUNDS);
    assert_eq!(harness.coordinator.pending_count(), 0);
    harness.shutdown().await;
}

/// Sampling, roots, a mixed request set, an unrepresentable schema, and an
/// oversized `requestState` are all refused deterministically — with no
/// interaction published, no model call, and no workspace disclosure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_input_requests_fail_without_publishing_anything() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::unsupported_input_requests_fail_without_publishing_anything",
        FixtureOptions::default(),
    )
    .await;
    for (tool, call_id, expected) in [
        (MRTR_SAMPLING_TOOL, "sampling-1", "sampling"),
        (MRTR_ROOTS_TOOL, "roots-1", "roots"),
        (MRTR_MIXED_TOOL, "mixed-1", "sampling"),
        (MRTR_UNSUPPORTED_SCHEMA_TOOL, "schema-1", "bounded"),
        (MRTR_OVERSIZED_STATE_TOOL, "state-1", "retention bound"),
    ] {
        let progress = RecordingProgress::default();
        let result = harness.invoke(tool, call_id, &progress, true).await;
        let error = failure(&result);
        assert!(
            error.contains(expected),
            "{tool} names why it is unsupported: {error}"
        );
        assert_eq!(
            harness.coordinator.pending_count(),
            0,
            "{tool} publishes no interaction"
        );
    }
    assert!(
        harness.audit.events().is_empty(),
        "no unsupported round commits an interaction fact"
    );
    harness.shutdown().await;
}

/// An execution with **no** runtime-owned interaction authority never
/// advertises elicitation, and refuses an `input_required` answer
/// deterministically instead of inventing a waiter.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_execution_without_interaction_authority_advertises_nothing_and_refuses() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::an_execution_without_interaction_authority_advertises_nothing_and_refuses",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(MRTR_CONFIRM_TOOL, "no-authority-1", &progress, false)
        .await;
    let error = failure(&result);
    assert!(
        error.contains("no runtime-owned interaction authority"),
        "the refusal is explicit about ownership: {error}"
    );
    let rounds = harness.rounds();
    assert_eq!(rounds.len(), 1, "no continuation round is dispatched");
    assert!(
        !rounds[0].elicitation_advertised,
        "an execution that cannot settle elicitation never advertises it"
    );
    assert_eq!(harness.coordinator.pending_count(), 0);
    harness.shutdown().await;
}

/// Cancellation observed **before the first dispatch** produces a proven
/// cancellation and zero remote rounds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_before_the_first_dispatch_sends_no_round() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::cancellation_before_the_first_dispatch_sends_no_round",
        FixtureOptions::default(),
    )
    .await;
    assert!(
        harness
            .owner
            .request_cancel(CancellationReason::UserRequested)
    );
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(MRTR_CONFIRM_TOOL, "cancel-0", &progress, true)
        .await;
    // The generic lifecycle owns the canonical phase; what this proves is
    // that the settlement is a **proven** cancellation and that no remote
    // round was ever dispatched.
    assert!(
        matches!(result.status, ToolExecutionStatus::Cancelled { .. }),
        "{:?}",
        result.status
    );
    assert!(harness.rounds().is_empty(), "no remote round is dispatched");
    harness.shutdown().await;
}

/// Cancellation while the interaction is pending settles that interaction
/// through the coordinator's own contract, and no later MCP round is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_while_the_interaction_is_pending_sends_no_continuation() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::cancellation_while_the_interaction_is_pending_sends_no_continuation",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(MRTR_CONFIRM_TOOL, "cancel-pending", &progress, true);
    tokio::pin!(call);
    let request = tokio::select! {
        result = &mut call => panic!("the call settled before asking: {result:?}"),
        request = harness.next_pending() => request,
    };
    assert!(
        harness
            .owner
            .request_cancel(CancellationReason::RuntimeShutdown)
    );
    let settled = call.await;
    assert!(
        matches!(
            settled.status,
            ToolExecutionStatus::Cancelled {
                reason: CancellationReason::RuntimeShutdown,
                ..
            }
        ),
        "{:?}",
        settled.status
    );
    assert_eq!(harness.rounds().len(), 1, "no continuation round is sent");
    // The pending interaction was settled by the coordinator, not abandoned.
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
    harness.shutdown().await;
}

/// A human response that **loses** the race against cancellation cannot
/// trigger another MCP round.
///
/// The barrier is the exact linearization point: the response is already
/// accepted and mapped to `inputResponses`, and the invocation is held one
/// step before the continuation dispatch frontier.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_winning_the_continuation_frontier_dispatches_nothing() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::cancellation_winning_the_continuation_frontier_dispatches_nothing",
        FixtureOptions::default(),
    )
    .await;
    let (barrier, _guard) = crate::tools::mcp::test_sync::MrtrContinuationBarrier::install(
        &harness.tool(MRTR_CONFIRM_TOOL),
    );
    let progress = RecordingProgress::default();
    let call = harness.invoke(MRTR_CONFIRM_TOOL, "race-cancel", &progress, true);
    tokio::pin!(call);
    let request = tokio::select! {
        result = &mut call => panic!("the call settled before asking: {result:?}"),
        request = harness.next_pending() => request,
    };
    harness.answer(&request.id, "stable").await;
    // The response has been accepted and mapped; the continuation has not
    // been dispatched.
    tokio::select! {
        result = &mut call => panic!("the call settled at the barrier: {result:?}"),
        () = barrier.wait_arrived(1) => {}
    }
    assert_eq!(barrier.answered_requests(), vec![1]);
    assert_eq!(harness.rounds().len(), 1);
    // Cancellation wins the frontier.
    assert!(
        harness
            .owner
            .request_cancel(CancellationReason::RuntimeShutdown)
    );
    barrier.release();
    let settled = call.await;
    assert!(
        matches!(
            settled.status,
            ToolExecutionStatus::Cancelled {
                reason: CancellationReason::RuntimeShutdown,
                phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
            }
        ),
        "{:?}",
        settled.status
    );
    assert_eq!(
        harness.rounds().len(),
        1,
        "the late human response created no new remote round"
    );
    harness.shutdown().await;
}

/// The mirror image: the continuation dispatch frontier wins first, so the
/// request genuinely exists and ordinary MCP cancellation semantics apply to
/// it — an unconfirmed remote outcome, never a fabricated cancellation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_continuation_that_wins_the_frontier_keeps_ordinary_mcp_semantics() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::a_continuation_that_wins_the_frontier_keeps_ordinary_mcp_semantics",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(
        MRTR_SLOW_CONTINUATION_TOOL,
        "race-dispatch",
        &progress,
        true,
    );
    tokio::pin!(call);
    let request = tokio::select! {
        result = &mut call => panic!("the call settled before asking: {result:?}"),
        request = harness.next_pending() => request,
    };
    harness.answer(&request.id, "stable").await;
    // The continuation round is genuinely in flight: the *server* says so,
    // through its own progress notification on the continuation round.
    tokio::select! {
        result = &mut call => panic!("the call settled before the continuation: {result:?}"),
        _ = progress.next() => {}
    }
    assert!(
        harness
            .owner
            .request_cancel(CancellationReason::RuntimeShutdown)
    );
    let settled = call.await;
    // Ordinary post-dispatch MCP semantics, unchanged by MRTR: the round
    // crossed the external-effect frontier, so the outcome is either a
    // correlated remote response or an honest `OutcomeUnknown` — never a
    // fabricated proven cancellation, and never a second terminal result.
    assert!(
        matches!(
            settled.status,
            ToolExecutionStatus::Success | ToolExecutionStatus::OutcomeUnknown { .. }
        ),
        "a dispatched continuation keeps ordinary MCP effect-certainty semantics: {:?}",
        settled.status
    );
    assert_eq!(harness.rounds().len(), 2, "the continuation was dispatched");
    harness.shutdown().await;
}

/// An explicit human decline is translated into the protocol's own
/// `decline` action; the server still gets a complete round and the
/// invocation still ends in exactly one terminal result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_explicit_decline_reaches_the_server_as_a_protocol_decline() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::an_explicit_decline_reaches_the_server_as_a_protocol_decline",
        FixtureOptions::default(),
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(MRTR_CONFIRM_TOOL, "decline-1", &progress, true);
    tokio::pin!(call);
    let request = tokio::select! {
        result = &mut call => panic!("the call settled before asking: {result:?}"),
        request = harness.next_pending() => request,
    };
    harness
        .coordinator
        .respond_async(
            &request.id,
            InteractionResponse::Questionnaire {
                response: QuestionnaireResponse::Declined,
            },
        )
        .await
        .expect("an explicit decline is accepted");
    let settled = call.await;
    assert!(matches!(settled.status, ToolExecutionStatus::Success));
    assert_eq!(text(&settled), "rounds=2 choice=<none>");
    let rounds = harness.rounds();
    assert_eq!(rounds.len(), 2);
    assert_eq!(
        rounds[1].input_responses,
        Some(serde_json::json!({"ask": {"action": "decline"}}))
    );
    harness.shutdown().await;
}

/// With no capable human provider the existing coordinator contract decides:
/// the invocation fails with the coordinator's own unavailability, and rustX
/// invents no MCP-specific waiting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_capable_interaction_client_uses_the_existing_coordinator_contract() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::no_capable_interaction_client_uses_the_existing_coordinator_contract",
        FixtureOptions::default(),
    )
    .await;
    harness.coordinator.set_provider_available(false);
    let progress = RecordingProgress::default();
    let result = harness
        .invoke(MRTR_CONFIRM_TOOL, "unavailable-1", &progress, true)
        .await;
    let error = failure(&result);
    assert!(
        error.contains("no capable human interaction provider was admitted"),
        "the coordinator's own diagnostic is preserved: {error}"
    );
    assert_eq!(harness.rounds().len(), 1, "no continuation is dispatched");
    assert!(
        harness.audit.events().is_empty(),
        "an unavailable provider commits no requested fact"
    );
    harness.shutdown().await;
}

/// Progress reported by several MCP rounds all belongs to the one invocation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn progress_from_every_round_belongs_to_one_invocation() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::progress_from_every_round_belongs_to_one_invocation",
        FixtureOptions { rounds: Some(3) },
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(MRTR_PROGRESS_TOOL, "progress-1", &progress, true);
    tokio::pin!(call);
    let mut answered = 0usize;
    let settled = loop {
        tokio::select! {
            result = &mut call => break result,
            request = harness.next_pending() => {
                answered += 1;
                harness.answer(&request.id, "stable").await;
            }
        }
    };
    assert!(matches!(settled.status, ToolExecutionStatus::Success));
    assert_eq!(answered, 2);
    assert_eq!(harness.rounds().len(), 3);
    let reported = progress.reported();
    assert!(
        reported.len() >= 2,
        "progress from more than one round reaches the one invocation reporter: {reported:?}"
    );
    harness.shutdown().await;
}

/// No MRTR protocol state escapes the executor.
///
/// The terminal `ToolExecutionResult`, every reported progress fact, and every
/// durable interaction envelope are checked for the protocol markers. This is
/// the executor-boundary proof behind the canonical-history and durable-state
/// invariants: the Agent Loop's canonical `ToolResult`, the Event Journal's
/// interaction facts, and — because Goal, Workflow, and Subagent state are all
/// built from these same published facts — every durable domain downstream can
/// only contain what is checked here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_mrtr_protocol_state_escapes_into_any_published_fact() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::no_mrtr_protocol_state_escapes_into_any_published_fact",
        FixtureOptions { rounds: Some(3) },
    )
    .await;
    let progress = RecordingProgress::default();
    let call = harness.invoke(MRTR_PROGRESS_TOOL, "leak-1", &progress, true);
    tokio::pin!(call);
    let settled = loop {
        tokio::select! {
            result = &mut call => break result,
            request = harness.next_pending() => harness.answer(&request.id, "stable").await,
        }
    };
    assert!(matches!(settled.status, ToolExecutionStatus::Success));
    // Three physical rounds, exactly one terminal result, and the opaque
    // state provably existed on the wire.
    let rounds = harness.rounds();
    assert_eq!(rounds.len(), 3);
    assert!(rounds[1].request_state.is_some());

    let markers = [
        "requestState",
        "request_state",
        "inputResponses",
        "input_responses",
        "inputRequests",
        "input_required",
        crate::tools::mcp::fixture::FIXTURE_ROUND_STATE_PREFIX,
    ];
    let result_text = format!("{:?}{:?}", settled.status, settled.content);
    for marker in markers {
        assert!(
            !result_text.contains(marker),
            "the terminal result leaks {marker}: {result_text}"
        );
    }
    let progress_text = format!("{:?}", progress.reported());
    for marker in markers {
        assert!(
            !progress_text.contains(marker),
            "a progress fact leaks {marker}: {progress_text}"
        );
    }
    // The durable interaction facts describe the Questionnaire and nothing
    // else: two requested facts and two settled facts, no protocol state.
    let committed = format!("{:?}", harness.audit.committed());
    for marker in markers {
        assert!(
            !committed.contains(marker),
            "a durable interaction fact leaks {marker}: {committed}"
        );
    }
    assert_eq!(
        harness.audit.events().len(),
        4,
        "two rounds commit one requested and one settled fact each"
    );
    harness.shutdown().await;
}

/// One model `ToolCall` is one executor start, whatever the round count.
///
/// This is what makes "approval is evaluated once, not once per round" true:
/// the Agent Loop's pre-tool policy runs before `ToolExecutor::start`, and the
/// whole MRTR lifetime lives inside that single call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn several_rounds_are_still_exactly_one_executor_start() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::several_rounds_are_still_exactly_one_executor_start",
        FixtureOptions { rounds: Some(4) },
    )
    .await;
    let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executor = CountingExecutor {
        inner: crate::tools::mcp::McpToolExecutor::new(
            Arc::clone(&harness.runtime),
            harness.tool(MRTR_CONFIRM_TOOL),
        ),
        starts: Arc::clone(&starts),
    };
    let progress = RecordingProgress::default();
    let context = ToolExecutionContext::new(
        harness.tool_runtime.conversation_id(),
        None,
        harness.owner.execution_cancellation(),
        harness.tool_runtime.workspace(),
        &progress,
        harness.tool_runtime.artifacts(),
        harness.tool_runtime.tool_output(),
        harness.tool_runtime.environment(),
    )
    .with_questionnaire_requester(harness.requester());
    let call = executor.start(
        ToolInvocation {
            id: ToolInvocationId::Agent {
                call_id: ToolCallId::new("one-start"),
            },
            tool_id: crate::runtime::identity::ToolId::new("mcp:mrtr"),
            tool_name: harness.tool(MRTR_CONFIRM_TOOL),
            mode: ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"subject": "release"}),
        },
        context,
    );
    let completion = call.completion;
    tokio::pin!(completion);
    let mut answered = 0usize;
    let settled = loop {
        tokio::select! {
            result = &mut completion => break result,
            request = harness.next_pending() => {
                answered += 1;
                harness.answer(&request.id, "stable").await;
            }
        }
    };
    assert!(matches!(settled.status, ToolExecutionStatus::Success));
    assert_eq!(answered, 3, "three input-required rounds");
    assert_eq!(harness.rounds().len(), 4, "four physical tools/call rounds");
    assert_eq!(
        starts.load(std::sync::atomic::Ordering::Acquire),
        1,
        "one ToolCall is one executor start, so approval cannot be re-evaluated per round"
    );
    harness.shutdown().await;
}

/// A routed (child/subagent) coordinator keeps its authority over an MRTR
/// questionnaire: the request is admitted and published through the existing
/// reliable route, exactly as `ask_user` is, and MRTR adds no route of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_routed_child_coordinator_still_owns_the_mrtr_questionnaire() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::a_routed_child_coordinator_still_owns_the_mrtr_questionnaire",
        FixtureOptions::default(),
    )
    .await;
    let route = Arc::new(RecordingRoute::default());
    harness.coordinator.install_route(route.clone());
    // A routed child has no local provider of its own: the root's permit is
    // the publication frontier.
    harness.coordinator.set_provider_available(false);
    let progress = RecordingProgress::default();
    let call = harness.invoke(MRTR_CONFIRM_TOOL, "routed-1", &progress, true);
    tokio::pin!(call);
    let request = tokio::select! {
        result = &mut call => panic!("the call settled before asking: {result:?}"),
        request = harness.next_pending() => request,
    };
    harness.answer(&request.id, "beta").await;
    let settled = call.await;
    assert!(matches!(settled.status, ToolExecutionStatus::Success));
    assert_eq!(text(&settled), "rounds=2 choice=beta");
    assert_eq!(
        route.kinds(),
        vec!["admit", "requested", "settled"],
        "MRTR uses the existing routed publication/settlement path unchanged"
    );
    harness.shutdown().await;
}

/// A **detached background** MCP execution never advertises elicitation and
/// refuses an `input_required` answer deterministically, under its own
/// `ToolExecutionId` and with exactly one settlement.
///
/// This is the honest resolution of Issue #242 §4 for background work: rustX
/// has no background human-interaction domain — `ask_user` is foreground-only
/// and an interaction is owned by a live attempt — so instead of inventing one
/// (or attributing an interaction to an already-terminal attempt) the adapter
/// simply tells the server the truth *per request*, and refuses what it cannot
/// serve.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_background_mcp_execution_advertises_nothing_and_settles_once() {
    if serve_if_fixture_mode(FixtureServer::from_env()).await {
        return;
    }
    let harness = Harness::connect(
        "boundary_suites::mcp_mrtr::a_background_mcp_execution_advertises_nothing_and_settles_once",
        FixtureOptions::default(),
    )
    .await;
    let directory = tempfile::tempdir().expect("background root");
    let workspace_root = directory.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let artifacts = directory.path().join("artifacts");
    let conversation = ConversationId::new("mrtr-background");
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
        harness.tool(MRTR_CONFIRM_TOOL),
    ));
    let invocation = ToolInvocation {
        id: ToolInvocationId::Agent {
            call_id: ToolCallId::new("background-mrtr"),
        },
        tool_id: crate::runtime::identity::ToolId::new("mcp:mrtr"),
        tool_name: harness.tool(MRTR_CONFIRM_TOOL),
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
    let error = failure(&result);
    assert!(
        error.contains("no runtime-owned interaction authority"),
        "the refusal is explicit about ownership: {error}"
    );
    let rounds = harness.rounds();
    assert_eq!(rounds.len(), 1, "no continuation round is dispatched");
    assert!(
        !rounds[0].elicitation_advertised,
        "a detached execution never advertises a capability it cannot settle"
    );
    // No interaction of any kind was created for the detached work.
    assert_eq!(harness.coordinator.pending_count(), 0);
    assert!(harness.audit.events().is_empty());
    harness.shutdown().await;
}
