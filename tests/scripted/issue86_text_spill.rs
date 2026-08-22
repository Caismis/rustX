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
/// 16 KiB preview bound by two orders of magnitude (300000 lines of ~13
/// bytes, ~4 MB), so the canonical terminal record staying small proves
/// the complete output is never duplicated into it.
const BIG_OUTPUT_COMMAND: &str = "for i in $(seq 1 300000); do echo line-$i; done";

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
/// through the real preflight/dispatch path, waits for its terminal
/// settlement, and returns the execution identity plus the live-output
/// locator the accepted result advertised at dispatch time.
async fn dispatch_big_background_bash(
    fixture: &IssueFixture,
) -> (rustx::runtime::identity::ToolExecutionId, String) {
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
            None,
        )
        .expect("dispatch prepared");
    let committed = fixture
        .runtime
        .background()
        .commit_dispatch(prepared_dispatch, &CancellationSignal::new())
        .expect("dispatch committed");
    let rustx::tools::background::BackgroundDispatchOutcome::Accepted {
        execution_id,
        result,
    } = committed
    else {
        panic!("the dispatch is accepted");
    };
    // The accepted result advertises the live-output locator immediately,
    // before the process completes (Issue #86).
    let advertised = match &result.content[0] {
        rustx::tools::types::ToolResultContent::Json { value } => value["output_path"]
            .as_str()
            .expect("the accepted result advertises the live-output locator")
            .to_owned(),
        other => panic!("the accepted result is JSON: {other:?}"),
    };
    assert!(std::path::Path::new(&advertised).is_absolute());
    assert!(advertised.ends_with("tasks/exec_1.output"), "{advertised}");
    assert!(
        std::path::Path::new(&advertised).exists(),
        "the advertised path exists from the dispatch commit point on"
    );
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
    (execution_id, advertised)
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
    let (execution_id, advertised) = dispatch_big_background_bash(&fixture).await;

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
    // The complete-vs-partial output truth is typed runtime-owned
    // continuation metadata; the tool-owned JSON carries no magic keys.
    for block in &result.content {
        if let rustx::tools::types::ToolResultContent::Json { value } = block {
            assert!(
                value.get("full_output").is_none()
                    && value.get("partial_output").is_none()
                    && value.get("note").is_none(),
                "the tool-owned JSON carries no continuation keys: {value}"
            );
        }
    }
    let Some(rustx::tools::ManagedOutputContinuation::Complete { locator }) =
        &result.managed_output
    else {
        panic!(
            "the settled background output is typed Complete, got {:?}",
            result.managed_output
        );
    };
    let full_output = locator.to_str().expect("utf8 output locator");
    assert!(std::path::Path::new(full_output).is_absolute());
    assert_eq!(
        full_output, advertised,
        "settlement reuses the dispatch-time live-output locator: no second file for the same payload"
    );
    assert!(
        std::path::Path::new(full_output).starts_with(fixture.runtime.tool_output().root()),
        "the output lives in the managed tool-output root: {full_output}"
    );
    // A very large background output is exactly one file: no duplicate
    // result spill was created for the same payload.
    assert!(
        std::fs::read_dir(fixture.runtime.tool_output().root().join("results"))
            .expect("results dir")
            .next()
            .is_none(),
        "background execution output never becomes a second result spill"
    );
    let spilled = std::fs::read_to_string(full_output).expect("output text");
    assert!(spilled.starts_with("line-1\n"));
    assert!(spilled.ends_with("line-300000\n"));

    // The terminal inbound message is text-only AND carries the exact
    // absolute spill locator plus the Read/Grep continuation instruction:
    // the canonical next-turn input lets the model inspect the spill.
    let message = terminal_message(&fixture);
    let mut saw_locator = false;
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
        if text.text.contains(full_output) {
            saw_locator = true;
            assert!(
                text.text.contains("Read or Grep"),
                "the continuation instruction travels with the locator: {}",
                text.text
            );
            // The terminal inbound is canonical conversation state and
            // must stay bounded against the ~4 MB complete output: only
            // the bounded projection enters it, never the full text (the
            // historical Claude Code JSONL-duplication bug is the
            // negative reference here).
            assert!(
                text.text.len() <= rustx::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES + 256,
                "the canonical inbound is the bounded projection ({} bytes), never the complete {}-byte output",
                text.text.len(),
                spilled.len()
            );
        }
    }
    assert!(
        saw_locator,
        "the terminal inbound text carries the exact absolute spill path {full_output}"
    );
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
    let (execution_id, _) = dispatch_big_background_bash(&fixture).await;
    // The exact absolute spill locator of the settled result.
    let snapshot = fixture
        .runtime
        .background()
        .snapshot(&execution_id)
        .expect("terminal snapshot");
    let result = snapshot.result.expect("terminal result");
    let Some(rustx::tools::ManagedOutputContinuation::Complete { locator }) =
        &result.managed_output
    else {
        panic!(
            "the settled background output is typed Complete, got {:?}",
            result.managed_output
        );
    };
    let full_output = locator.to_str().expect("utf8 output locator").to_owned();
    let terminal_text = match &terminal_message(&fixture).content[0] {
        UserContentBlock::Text(text) => text.text.clone(),
        other => panic!("text-only terminal inbound: {other:?}"),
    };
    assert!(
        terminal_text.contains(&full_output),
        "the terminal inbound text itself carries the exact spill path"
    );

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
        server.request_body(1).contains(
            &terminal_text
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
        ),
        "the second provider request carries the adopted text-only terminal message"
    );
    assert!(
        server.request_body(1).contains(&full_output),
        "the exact absolute spill path reaches the provider boundary"
    );
    assert!(
        !server.request_body(0).contains(execution_id.as_str()),
        "turn one predates the adoption"
    );
}

/// The provider-boundary regression of the background live-output contract
/// (Issue #86): the model requests a background Bash execution; the rustX
/// tool result of that tool call — sent into the NEXT provider turn —
/// carries the exact absolute live-output path plus the Read/Grep
/// continuation guidance while the process is still running behind a FIFO
/// barrier. After the barrier releases, the terminal runtime message
/// references the exact same path.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_background_live_output_path_reaches_the_provider_before_completion() {
    let fixture = fixture();
    // The deterministic barrier: the command prints marker A, blocks on a
    // FIFO read, and prints marker B only after the test releases it.
    let fifo = fixture.runtime.workspace().root().join("turn.fifo");
    nix::unistd::mkfifo(
        &fifo,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .expect("mkfifo");
    let command = format!(
        "printf 'turn-marker-A\\n'; read -r _ < '{}'; printf 'turn-marker-B\\n'",
        fifo.display()
    );
    let arguments = serde_json::json!({
        "__rustx_execution": "background",
        "command": command,
    })
    .to_string();
    // A dynamically built SSE turn-one reply: one background Bash tool
    // call, then the tool_calls finish reason.
    let chunk = |delta: serde_json::Value, finish: serde_json::Value| {
        serde_json::json!({
            "id": "chatcmpl-bg",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "gpt-test",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        })
    };
    let turn_one_body = [
        chunk(
            serde_json::json!({"role": "assistant", "content": null, "tool_calls": [{
                "index": 0, "id": "call_bg", "type": "function",
                "function": {"name": "bash", "arguments": ""},
            }]}),
            serde_json::Value::Null,
        ),
        chunk(
            serde_json::json!({"tool_calls": [{
                "index": 0, "function": {"arguments": arguments},
            }]}),
            serde_json::Value::Null,
        ),
        chunk(serde_json::json!({}), serde_json::json!("tool_calls")),
    ]
    .into_iter()
    .map(|event| format!("data: {}\n", serde_json::to_string(&event).expect("sse")))
    .collect::<Vec<_>>()
    .join("\n")
        + "\ndata: [DONE]\n";
    let server = common::FixtureServer::start(move |attempt, _head| {
        if attempt == 0 {
            common::FixtureReply::body(200, "OK", "text/event-stream", turn_one_body.clone())
        } else {
            common::sse_fixture("openai_chat", "plain_text.sse")
        }
    })
    .await;
    let adapter: Arc<dyn rustx::model::ModelAdapter> = Arc::new(OpenAiChatCompletionsAdapter::new(
        OpenAiAdapterConfig::new("k", server.url("/v1")),
    ));
    let model = support::attempt_model(adapter, "background-model");
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
            agent_id: AgentId::new("agent-issue86-bg"),
            conversation_id: fixture.runtime.conversation_id().clone(),
            attempt_id: AttemptId::new("attempt-background"),
            conversation: rustx::conversation::ConversationState::from_messages(vec![
                MessageBlock::User(UserMessageBlock {
                    id: MessageId::new("msg-user-bg"),
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "run it in the background".to_owned(),
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
        "the attempt completes while the background execution runs: {:?}",
        result.outcome
    );
    assert_eq!(server.attempt_count(), 2, "exactly two provider turns");

    // The tool result rustX sent into the second provider turn.
    let tool_message = result
        .messages()
        .iter()
        .find_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("the background dispatch tool message");
    let accepted = tool_message
        .result
        .content
        .iter()
        .find_map(|block| match block {
            rustx::tools::types::ToolResultContent::Json { value } => Some(value.clone()),
            _ => None,
        })
        .expect("the accepted result JSON");
    let output_path = accepted["output_path"]
        .as_str()
        .expect("the accepted result advertises the live-output locator")
        .to_owned();
    assert!(std::path::Path::new(&output_path).is_absolute());
    assert!(
        output_path.ends_with("tasks/exec_1.output"),
        "{output_path}"
    );
    assert!(
        std::path::Path::new(&output_path).exists(),
        "the advertised path exists before the process completes"
    );
    assert!(
        accepted["note"]
            .as_str()
            .expect("note")
            .contains("Read or Grep"),
        "the continuation guidance travels with the locator"
    );

    // The exact absolute path and the Read/Grep guidance are in the SECOND
    // provider request — the model turn immediately after the dispatch —
    // while the process is still running behind the FIFO barrier.
    let second_request = server.request_body(1);
    assert!(
        second_request.contains(&output_path),
        "the provider request of the next turn carries the live-output path"
    );
    assert!(
        second_request.contains("Read or Grep"),
        "the provider request carries the continuation guidance"
    );
    let execution_id = rustx::runtime::identity::ToolExecutionId::new("exec_1");
    let running = fixture
        .runtime
        .background()
        .snapshot(&execution_id)
        .expect("snapshot");
    assert!(
        running.state.is_active(),
        "the execution is still running: {:?}",
        running.state
    );

    // Release the barrier; the terminal runtime message references the
    // EXACT same path as the complete output.
    let fifo_path = fifo.clone();
    tokio::task::spawn_blocking(move || std::fs::write(fifo_path, "go\n"))
        .await
        .expect("fifo writer")
        .expect("release the barrier");
    let terminal = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        fixture
            .runtime
            .background()
            .wait_until_terminal(&execution_id),
    )
    .await
    .expect("the execution settles (liveness guard)")
    .expect("terminal snapshot");
    assert_eq!(
        terminal.state,
        rustx::tools::background::BackgroundLifecycle::Succeeded
    );
    let batch = fixture
        .runtime
        .mailbox()
        .select_pending_batch()
        .expect("select")
        .expect("one pending terminal batch");
    let terminal_message = batch.items()[0].message();
    let terminal_text = match &terminal_message.content[..] {
        [UserContentBlock::Text(text)] => text.text.clone(),
        blocks => panic!("the terminal inbound is text-only: {blocks:?}"),
    };
    assert!(
        terminal_text.contains(&format!("Complete output: {output_path}")),
        "the terminal message reuses the dispatch-time locator: {terminal_text}"
    );
    assert!(
        terminal_text.contains("Read or Grep"),
        "the terminal guidance survives: {terminal_text}"
    );
    assert_eq!(
        std::fs::read_to_string(&output_path).expect("final output"),
        "turn-marker-A\nturn-marker-B\n",
        "the settled file holds the complete output"
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
                    managed_output: None,
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
            None,
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
