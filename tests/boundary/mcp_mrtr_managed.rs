//! The real managed **`FastMCP` 4** multi-round-trip acceptance (Issue #242).
//!
//! Everything in this proof is real: a real `uv` materialization of the
//! rustX-pinned `FastMCP` build, a real Python child speaking the MCP
//! `2026-07-28` inline lifecycle over the ordinary generic
//! [`McpServerRuntime`] transport, a real `tools/call` that answers with a
//! SEP-2322 `InputRequiredResult`, a real runtime-owned Questionnaire settled
//! through the conversation's [`InteractionCoordinator`], and a real
//! continuation round that completes the call.
//!
//! ```text
//! one model `ToolCall`
//!   -> one rustX `ToolInvocation`
//!   -> real managed `FastMCP` 4 child (negotiated 2026-07-28)
//!   -> tools/call round 1  -> `InputRequiredResult`
//!   -> one rustX runtime Interaction (deterministic test response)
//!   -> tools/call round 2  -> final `CallToolResult`
//!   -> exactly one rustX `ToolResult`
//! ```
//!
//! The fixture is written in the **modern guard style**, not with imperative
//! `ctx.elicit()`: `FastMCP` 4 refuses imperative elicitation on a `2026-07-28`
//! connection, and the guard form is what the protocol actually defines. The
//! tool returns an `InputRequiredResult` as the complete result of its leg,
//! and the next leg reads `ctx.input_responses` / `ctx.request_state`.
//!
//! `FastMCP` **seals** `requestState` on the wire and unseals it before the
//! tool body runs, so the final result echoing `ctx.request_state` is
//! byte-equivalence evidence of its own: a client that altered one byte of
//! the opaque token could not have produced this result.
//!
//! No `FastMCP`-specific execution path exists. This drives the same generic
//! `McpToolExecutor` every other MCP server uses.
//!
//! Following the repository's opt-in-by-availability convention (see
//! `tests/tools/uv.rs`), the acceptance reports an honest skip when `uv` is
//! not on `PATH`.

use std::sync::Arc;

use crate::durable::TranscriptCursor;
use crate::events::RuntimeEventEnvelope;
use crate::events::interaction::{
    QuestionnaireAnswer, QuestionnaireAnswerEntry, QuestionnaireResponse, QuestionnaireSubmission,
    SingleOptionAnswer,
};
use crate::runtime::identity::{AttemptId, ConversationId, InteractionId, ToolCallId};
use crate::runtime::interaction::{
    InteractionCoordinator, InteractionObserver, InteractionRequest, InteractionResponse,
    QuestionnaireRequester, RecordingInteractionAudit,
};
use crate::runtime::types::ConversationLifecycle;
use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
use crate::tools::mcp::{McpInvalidationState, McpServerRuntime};
use crate::tools::types::{
    ToolExecutionStatus, ToolInvocation, ToolInvocationId, ToolInvocationMode,
    ToolInvocationPolicy, ToolResultContent,
};
use crate::tools::workspace::Workspace;

/// The modern SEP-2322 guard tool, written against the `FastMCP` 4.0.3 API that
/// the rustX-pinned managed build actually provides.
const GUARD_SERVER: &str = r#"from pathlib import Path

import mcp_types
from fastmcp import Context, FastMCP

mcp = FastMCP("mrtr-tool")


@mcp.tool
def confirm_release(component: str, ctx: Context) -> str:
    """Confirm a release channel with the user, then report the decision."""
    with Path("mrtr-rounds.log").open("a", encoding="utf-8") as log:
        log.write("round\n")
    responses = ctx.input_responses
    if responses is None:
        return mcp_types.InputRequiredResult(
            input_requests={
                "release": mcp_types.ElicitRequest(
                    params=mcp_types.ElicitRequestFormParams(
                        message="Which release channel?",
                        requested_schema={
                            "type": "object",
                            "properties": {
                                "channel": {
                                    "type": "string",
                                    "enum": ["stable", "beta"],
                                }
                            },
                            "required": ["channel"],
                        },
                    )
                )
            },
            request_state=f"component={component}",
        )
    answer = responses["release"]
    content = getattr(answer, "content", None) or {}
    return f"{component}|{ctx.request_state}|{content.get('channel')}"
"#;

struct NoProgress;

impl ProgressReporter for NoProgress {
    fn report(&self, _progress: crate::tools::types::ToolProgress) {}
}

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

/// A real managed `FastMCP` 4 tool completes a multi-round-trip call through
/// the generic MCP path and one runtime-owned Interaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn a_real_managed_fastmcp_tool_completes_through_one_runtime_interaction() {
    let uv = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("uv"))
            .find(|path| path.is_file())
    });
    if uv.is_none() {
        eprintln!(
            "uv unavailable; the real managed FastMCP 4 MRTR acceptance was NOT exercised (skipped)"
        );
        return;
    }
    let directory = tempfile::tempdir().expect("workspace root");
    let workspace_root = directory.path().join("workspace");
    let package_root = workspace_root.join(".agents/tools/mrtr-tool");
    std::fs::create_dir_all(&package_root).expect("package root");
    std::fs::write(package_root.join("server.py"), GUARD_SERVER).expect("server source");
    std::fs::write(package_root.join("requirements.txt"), "# none\n").expect("requirements");

    let workspace = Workspace::new(&workspace_root).expect("workspace");
    let discovered =
        crate::tools::python::discover_python_packages(&workspace).expect("package discovery");
    let package = discovered
        .into_iter()
        .find(|entry| entry.server_id == crate::tools::python::python_server_id("mrtr-tool"))
        .expect("the guard package is discovered")
        .outcome
        .expect("the guard package is valid");
    let store = crate::tools::python::PythonToolStore::new(directory.path().join("runtime"))
        .expect("python tool store");
    let prepared = store
        .ensure_prepared(&package, &crate::runtime::CancellationSignal::new())
        .await
        .expect("the managed FastMCP environment materializes");
    // The exact rustX-owned pin this acceptance ran against.
    eprintln!(
        "managed FastMCP pin: {}",
        crate::tools::python::MANAGED_FASTMCP_VERSION
    );

    let server_id = crate::tools::python::python_server_id("mrtr-tool");
    let runtime = McpServerRuntime::connect(
        &server_id,
        &prepared.server_binding(),
        &workspace,
        Arc::new(McpInvalidationState::new()),
    )
    .await
    .expect("the managed FastMCP child connects");
    // The negotiated revision of this connection, not the installed package
    // version, is what makes SEP-2322 applicable at all.
    assert_eq!(
        runtime.protocol_version().as_str(),
        rmcp::model::ProtocolVersion::V_2026_07_28.as_str(),
        "the managed FastMCP 4 child negotiates the modern revision"
    );
    let tools = runtime.list_tools().await.expect("tools/list");
    let definitions = crate::tools::mcp::definitions(
        &server_id,
        ToolInvocationPolicy::default(),
        &runtime,
        tools,
    );
    let (definition, executor) = definitions
        .into_iter()
        .find(|(definition, _)| definition.name == "confirm_release")
        .expect("the guard tool is discovered");

    let artifacts = tempfile::tempdir().expect("artifacts");
    let bundle = crate::tools::runtime::ConversationToolRuntime::new(
        ConversationId::new("mrtr-managed"),
        &workspace_root,
        artifacts.path(),
    )
    .expect("tool runtime");
    let lifecycle = ConversationLifecycle::new();
    assert!(lifecycle.activate());
    let conversation_id = ConversationId::new("mrtr-managed");
    let audit = RecordingInteractionAudit::new(conversation_id.clone());
    let coordinator = Arc::new(InteractionCoordinator::new(
        conversation_id,
        lifecycle,
        audit.clone(),
    ));
    coordinator.set_provider_available(true);
    let (sender, mut pending) = tokio::sync::mpsc::unbounded_channel();
    coordinator.install_observer(Arc::new(PendingSink { sender }));
    let owner = crate::agent::cancellation::AgentCancellation::new(
        crate::runtime::types::CancellationReason::UserRequested,
    );
    let requester = QuestionnaireRequester::new(
        Arc::clone(&coordinator),
        AttemptId::new("mrtr-managed-attempt"),
        owner.execution_cancellation(),
        1,
    );
    let progress = NoProgress;
    let context = ToolExecutionContext::new(
        bundle.conversation_id(),
        None,
        owner.execution_cancellation(),
        bundle.workspace(),
        &progress,
        bundle.artifacts(),
        bundle.tool_output(),
        bundle.environment(),
    )
    .with_questionnaire_requester(requester);
    let call = ToolExecutor::start(
        executor.as_ref(),
        ToolInvocation {
            id: ToolInvocationId::Agent {
                call_id: ToolCallId::new("managed-mrtr"),
            },
            tool_id: definition.id.clone(),
            tool_name: "confirm_release".to_owned(),
            mode: ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"component": "api"}),
        },
        context,
    )
    .completion;
    tokio::pin!(call);
    // Exactly one runtime Interaction, answered deterministically.
    let request = tokio::select! {
        result = &mut call => panic!("the call settled before asking: {result:?}"),
        request = pending.recv() => request.expect("one pending interaction"),
    };
    let crate::runtime::interaction::InteractionKind::Questionnaire { questionnaire, .. } =
        &request.kind
    else {
        panic!("the managed MRTR round publishes a Questionnaire");
    };
    assert_eq!(questionnaire.questions.len(), 1);
    assert_eq!(
        questionnaire.questions[0].question,
        "Which release channel?"
    );
    assert_eq!(
        questionnaire.questions[0]
            .options
            .iter()
            .map(|option| option.label.as_str())
            .collect::<Vec<_>>(),
        vec!["stable", "beta"]
    );
    coordinator
        .respond_async(
            &request.id,
            InteractionResponse::Questionnaire {
                response: QuestionnaireResponse::Submitted(QuestionnaireSubmission {
                    answers: vec![QuestionnaireAnswerEntry {
                        question_index: 0,
                        answer: QuestionnaireAnswer::SingleOption(SingleOptionAnswer {
                            label: "beta".to_owned(),
                        }),
                    }],
                }),
            },
        )
        .await
        .expect("the deterministic test response is accepted");
    let result = call.await;
    assert!(
        matches!(result.status, ToolExecutionStatus::Success),
        "{:?}",
        result.status
    );
    let text = result
        .content
        .iter()
        .filter_map(|content| match content {
            ToolResultContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    // The final result proves all three halves at once: the business argument
    // survived unchanged, FastMCP unsealed exactly the `requestState` rustX
    // echoed, and the human answer reached the tool body.
    assert_eq!(text, "api|component=api|beta");
    // Exactly two physical tools/call rounds reached the real child.
    let log = std::fs::read_to_string(workspace_root.join("mrtr-rounds.log"))
        .expect("the guard tool recorded its rounds");
    assert_eq!(
        log.lines().filter(|line| !line.is_empty()).count(),
        2,
        "one input_required round and one final round"
    );
    // Exactly one interaction, requested and settled once.
    assert_eq!(coordinator.pending_count(), 0);
    assert_eq!(
        audit.events().len(),
        2,
        "one requested and one settled interaction fact"
    );
    runtime.close().await.expect("physical settlement");
}
