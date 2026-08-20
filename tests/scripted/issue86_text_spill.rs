//! Issue #86: oversized textual output stays textual, and absolute
//! locators do not grant filesystem authority.
//!
//! These regressions prove the two halves of the architectural decision
//! end to end, deterministically and without sleeps:
//!
//! 1. A background Bash execution whose combined output crosses the preview
//!    bound spills the complete text into the conversation's managed
//!    tool-output root and publishes a **text-only** terminal inbound
//!    message — no `UserContentBlock::File` is ever created for textual
//!    overflow. After that message is adopted into the canonical
//!    conversation, the next text-only model invocation passes local
//!    content-modality validation and reaches the provider boundary.
//! 2. A genuine canonical `UserContentBlock::File` is still rejected
//!    locally for a text-only effective invocation, before any provider
//!    request exists — the fix is the architecture, not a weakened safety
//!    check.
//! 3. A genuine semantic artifact of a tool result still publishes as a
//!    `UserContentBlock::File` in the terminal inbound message: textual
//!    spill and semantic artifacts are separate domains.

use super::{common, support};

use std::sync::Arc;

use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::events::types::AttemptOutcome;
use rustx::message::content::TextBlock;
use rustx::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
use rustx::model::{OpenAiAdapterConfig, OpenAiChatCompletionsAdapter};
use rustx::runtime::CancellationSignal;
use rustx::runtime::identity::{
    AgentId, ArtifactId, AttemptId, ConversationId, MessageId, ToolCallId, ToolId,
};
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::{PreflightOutcome, ToolExecutor, ToolRegistry};
use rustx::tools::native::{NativeToolPolicies, NativeToolResources, register_native_tools};
use rustx::tools::runtime::ConversationToolRuntime;
use rustx::tools::types::{
    ToolCall, ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionResult, ToolExecutionStatus,
    ToolInvocation, ToolInvocationMode, ToolInvocationPolicy,
};

/// A background Bash command whose combined output provably crosses the
/// 16 KiB preview bound (3000 lines of ~13 bytes).
const BIG_OUTPUT_COMMAND: &str = "for i in $(seq 1 3000); do echo line-$i; done";

/// One conversation runtime with the native tool plane registered under a
/// model-selectable policy, so a scripted call can choose background Bash.
struct IssueFixture {
    _dir: tempfile::TempDir,
    runtime: ConversationToolRuntime,
    registry: ToolRegistry,
}

fn fixture() -> IssueFixture {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let runtime = ConversationToolRuntime::new(
        ConversationId::new("conv-issue86"),
        &workspace_root,
        dir.path().join("artifacts"),
    )
    .expect("tool runtime");
    let mut registry = ToolRegistry::new();
    register_native_tools(
        &mut registry,
        NativeToolResources {
            background: runtime.background().clone(),
            subagents: None,
        },
        NativeToolPolicies::uniform(ToolInvocationPolicy::new(
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Sequential,
        )),
    )
    .expect("native tools");
    IssueFixture {
        _dir: dir,
        runtime,
        registry,
    }
}

/// Dispatches one background Bash call with the oversized-output command
/// through the real preflight/dispatch path and waits for its terminal
/// settlement.
async fn dispatch_big_background_bash(
    fixture: &IssueFixture,
) -> rustx::runtime::identity::ToolExecutionId {
    let call = ToolCall {
        id: ToolCallId::new("call-big"),
        tool_id: ToolId::new("tool-bash"),
        name: "bash".to_owned(),
        arguments: serde_json::json!({
            "__rustx_execution": "background",
            "command": BIG_OUTPUT_COMMAND,
        }),
    };
    let outcome = fixture.registry.preflight(&call).expect("preflight");
    let PreflightOutcome::Ready(prepared) = outcome else {
        panic!("the background bash call preflights as ready");
    };
    let executor = fixture.registry.executor(&prepared.invocation.tool_id);
    let prepared_dispatch = fixture
        .runtime
        .background()
        .prepare_dispatch(
            &prepared.invocation,
            &executor,
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("dispatch prepared");
    let committed = fixture
        .runtime
        .background()
        .commit_dispatch(prepared_dispatch, &CancellationSignal::new())
        .expect("dispatch committed");
    let rustx::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        committed
    else {
        panic!("the dispatch is accepted");
    };
    let terminal = fixture
        .runtime
        .background()
        .wait_until_terminal(&execution_id)
        .await
        .expect("the execution settles");
    assert_eq!(
        terminal.state,
        rustx::tools::background::BackgroundLifecycle::Succeeded
    );
    execution_id
}

/// The one pending terminal inbound message of the mailbox.
fn terminal_message(fixture: &IssueFixture) -> UserMessageBlock {
    let batch = fixture
        .runtime
        .mailbox()
        .select_pending_batch()
        .expect("select")
        .expect("one pending terminal batch");
    assert_eq!(batch.items().len(), 1, "one terminal inbound message");
    batch.items()[0].message().clone()
}

/// Oversized textual output of a background Bash execution spills into the
/// managed tool-output root and publishes a text-only terminal inbound
/// message: no `UserContentBlock::File` is created by textual overflow.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_background_bash_publishes_a_text_only_terminal_inbound() {
    let fixture = fixture();
    let execution_id = dispatch_big_background_bash(&fixture).await;

    // The settled result is ordinary bounded text plus the absolute spill
    // locator; it carries no semantic artifact.
    let snapshot = fixture
        .runtime
        .background()
        .snapshot(&execution_id)
        .expect("terminal snapshot");
    let result = snapshot.result.expect("terminal result");
    assert!(
        result.artifacts.is_empty(),
        "textual overflow never enters the semantic artifact domain"
    );
    let content = result
        .content
        .iter()
        .map(|block| match block {
            rustx::tools::types::ToolResultContent::Json { value } => value.clone(),
            other => panic!("the bash result is ordinary structured text, got {other:?}"),
        })
        .next()
        .expect("json content");
    let full_output = content["full_output"]
        .as_str()
        .expect("the absolute spill locator");
    assert!(std::path::Path::new(full_output).is_absolute());
    assert!(
        std::path::Path::new(full_output).starts_with(fixture.runtime.tool_output().root()),
        "the spill lives in the managed tool-output root: {full_output}"
    );
    let spilled = std::fs::read_to_string(full_output).expect("spill text");
    assert!(spilled.starts_with("line-1\n"));
    assert!(spilled.ends_with("line-3000\n"));

    // The terminal inbound message is text-only.
    let message = terminal_message(&fixture);
    for block in &message.content {
        let UserContentBlock::Text(text) = block else {
            panic!("textual overflow must never publish a non-text block: {block:?}");
        };
        assert!(
            text.text.contains(&format!(
                "Background execution {execution_id} (bash) settled: succeeded"
            )),
            "the compact terminal summary: {}",
            text.text
        );
    }
}

/// After the text-only terminal inbound is adopted into the canonical
/// conversation, the next model invocation of a text-only effective model
/// passes local content-modality validation and reaches the provider
/// boundary. The provider is a raw fixture HTTP server behind the real
/// `OpenAI` Chat Completions adapter, so "reached the provider boundary" is
/// the server's own request count.
///
/// The terminal message arrives while the attempt is already running, so
/// the safe-boundary drain adopts it and drives exactly one further turn
/// whose request carries the terminal text.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adopted_textual_terminal_inbound_reaches_the_provider_for_a_text_only_model() {
    let fixture = fixture();
    let execution_id = dispatch_big_background_bash(&fixture).await;
    let terminal_text = match &terminal_message(&fixture).content[0] {
        UserContentBlock::Text(text) => text.text.clone(),
        other => panic!("text-only terminal inbound: {other:?}"),
    };

    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let adapter: Arc<dyn rustx::model::ModelAdapter> = Arc::new(OpenAiChatCompletionsAdapter::new(
        OpenAiAdapterConfig::new("k", server.url("/v1")),
    ));
    let model = support::attempt_model(adapter, "spill-model");
    let capability = common::capability_lease(fixture.registry.clone(), &fixture.runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let context_runtime = rustx::context::ContextRuntime::for_attempt(
        rustx::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(rustx::context::DefaultTokenEstimator),
        rustx::context::AgentStatusComposer::default(),
        &model,
    )
    .expect("context runtime");
    let result = AgentExecution::new(
        AgentExecutionRequest {
            agent_id: AgentId::new("agent-issue86"),
            conversation_id: fixture.runtime.conversation_id().clone(),
            attempt_id: AttemptId::new("attempt-after-spill"),
            conversation: rustx::conversation::ConversationState::from_messages(vec![
                MessageBlock::User(UserMessageBlock {
                    id: MessageId::new("msg-user-continue"),
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "continue".to_owned(),
                    })],
                    source: UserSource::Human,
                    kind: rustx::message::types::InboundKind::Message,
                    timestamp: None,
                }),
            ])
            .expect("bootstrap conversation"),
            initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
            timezone: None,
            model,
        },
        capability.into_lease(),
        &cancellation,
        context_runtime,
        &fixture.runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;

    assert!(
        matches!(result.outcome, AttemptOutcome::Completed { .. }),
        "the text-only attempt completes after adopting the textual spill terminal: {:?}",
        result.outcome
    );
    // The adopted terminal message is part of the canonical conversation.
    let adopted = result.messages().iter().any(|message| {
        matches!(
            message,
            MessageBlock::User(user)
                if user.content.iter().any(|block| matches!(
                    block,
                    UserContentBlock::Text(text) if text.text == terminal_text
                ))
        )
    });
    assert!(
        adopted,
        "the terminal inbound was adopted into canonical conversation state"
    );
    // Turn one runs first; the adopted batch then drives exactly one further
    // turn whose request carries the terminal text. Both requests passed
    // local modality validation and reached the provider boundary.
    assert_eq!(
        server.attempt_count(),
        2,
        "both model turns reached the provider boundary"
    );
    assert!(
        server
            .request_body(1)
            .contains(&terminal_text.replace('\\', "\\\\").replace('"', "\\\"")),
        "the second provider request carries the adopted text-only terminal message"
    );
    assert!(
        !server.request_body(0).contains(execution_id.as_str()),
        "turn one predates the adoption"
    );
}

/// The safety check is not weakened: a genuine canonical
/// `UserContentBlock::File` in the conversation is still rejected by local
/// content-modality validation for a text-only effective invocation, and
/// the provider never sees a request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_genuine_file_block_is_still_rejected_locally_for_a_text_only_model() {
    let fixture = fixture();
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let adapter: Arc<dyn rustx::model::ModelAdapter> = Arc::new(OpenAiChatCompletionsAdapter::new(
        OpenAiAdapterConfig::new("k", server.url("/v1")),
    ));
    let model = support::attempt_model(adapter, "spill-model");
    let capability = common::capability_lease(fixture.registry.clone(), &fixture.runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let context_runtime = rustx::context::ContextRuntime::for_attempt(
        rustx::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(rustx::context::DefaultTokenEstimator),
        rustx::context::AgentStatusComposer::default(),
        &model,
    )
    .expect("context runtime");
    let result = AgentExecution::new(
        AgentExecutionRequest {
            agent_id: AgentId::new("agent-issue86"),
            conversation_id: fixture.runtime.conversation_id().clone(),
            attempt_id: AttemptId::new("attempt-genuine-file"),
            conversation: rustx::conversation::ConversationState::from_messages(vec![
                MessageBlock::User(UserMessageBlock {
                    id: MessageId::new("msg-user-file"),
                    content: vec![
                        UserContentBlock::Text(TextBlock {
                            text: "summarize the report".to_owned(),
                        }),
                        UserContentBlock::File(rustx::message::content::FileReference {
                            artifact_id: ArtifactId::new("artifact-report"),
                            name: Some("report.pdf".to_owned()),
                            mime_type: Some("application/pdf".to_owned()),
                            description: None,
                        }),
                    ],
                    source: UserSource::Human,
                    kind: rustx::message::types::InboundKind::Message,
                    timestamp: None,
                }),
            ])
            .expect("bootstrap conversation"),
            initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
            timezone: None,
            model,
        },
        capability.into_lease(),
        &cancellation,
        context_runtime,
        &fixture.runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;

    assert!(
        matches!(result.outcome, AttemptOutcome::Failed { .. }),
        "a genuine File block against text-only capabilities fails locally: {:?}",
        result.outcome
    );
    assert_eq!(
        server.attempt_count(),
        0,
        "the provider receives zero requests"
    );
}

/// The terminal-inbound artifact publication is preserved for genuine
/// semantic artifacts: a tool result carrying a real `FileReference` still
/// publishes a `UserContentBlock::File`. Only textual overflow left that
/// domain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn genuine_artifacts_still_publish_as_file_blocks_in_the_terminal_inbound() {
    /// A stub executor whose result carries one genuine semantic artifact.
    struct ArtifactExecutor;
    impl ToolExecutor for ArtifactExecutor {
        fn execute<'a>(
            &'a self,
            _invocation: ToolInvocation,
            _context: rustx::tools::executor::ToolExecutionContext<'a>,
        ) -> futures_util::future::BoxFuture<'a, ToolExecutionResult> {
            Box::pin(async move {
                ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: vec![rustx::message::content::FileReference {
                        artifact_id: ArtifactId::new("artifact-report"),
                        name: Some("report.pdf".to_owned()),
                        mime_type: Some("application/pdf".to_owned()),
                        description: None,
                    }],
                    truncation: None,
                }
            })
        }
    }

    let fixture = fixture();
    let invocation = ToolInvocation {
        call_id: ToolCallId::new("call-artifact"),
        tool_id: ToolId::new("tool-artifact"),
        tool_name: "artifact".to_owned(),
        mode: ToolInvocationMode::Background,
        arguments: serde_json::json!({}),
    };
    let executor: Arc<dyn ToolExecutor> = Arc::new(ArtifactExecutor);
    let prepared = fixture
        .runtime
        .background()
        .prepare_dispatch(
            &invocation,
            &executor,
            rustx::tools::environment::ToolEnvironment::new(),
        )
        .expect("dispatch prepared");
    let committed = fixture
        .runtime
        .background()
        .commit_dispatch(prepared, &CancellationSignal::new())
        .expect("dispatch committed");
    let rustx::tools::background::BackgroundDispatchOutcome::Accepted { execution_id, .. } =
        committed
    else {
        panic!("the dispatch is accepted");
    };
    fixture
        .runtime
        .background()
        .wait_until_terminal(&execution_id)
        .await
        .expect("the execution settles");

    let message = terminal_message(&fixture);
    assert!(
        message.content.iter().any(|block| matches!(
            block,
            UserContentBlock::File(reference)
                if reference.artifact_id.as_str() == "artifact-report"
        )),
        "a genuine semantic artifact still publishes as a File block"
    );
}
