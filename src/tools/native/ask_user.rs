//! The native `ask_user` tool.
//!
//! `ask_user` is an ordinary foreground, sequential, approval-never Tool
//! whose executor publishes one bounded Question through the runtime-owned
//! `InteractionCoordinator`. It has no filesystem, network, process, or
//! authorization authority of its own.

use futures_util::future::BoxFuture;

use crate::runtime::{InteractionOutcome, InteractionResponse, QuestionAnswer, QuestionFacts};
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, input_schema};
use crate::tools::native::support::{failed_result, success_json};
use crate::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
    ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy,
};

/// The canonical model-facing name of the native question tool.
pub const ASK_USER_NAME: &str = crate::tools::executor::ASK_USER_TOOL_NAME;

/// The bounded model-facing input contract of `ask_user`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct AskUserInput {
    /// The question shown to the user.
    prompt: String,
    /// Optional finite answer choices.
    #[serde(default)]
    choices: Option<Vec<String>>,
    /// Whether a free-text answer is accepted.
    #[serde(default)]
    allow_free_text: bool,
}

impl AskUserInput {
    fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(arguments.clone())
            .map_err(|error| format!("invalid ask_user arguments: {error}"))
    }
}

/// The tool-owned registration of `ask_user`.
#[must_use]
pub(super) fn registration() -> NativeToolRegistration {
    NativeToolRegistration::new(definition(), std::sync::Arc::new(AskUserExecutor))
}

fn definition() -> ToolDefinition {
    ToolDefinition {
        id: crate::runtime::identity::ToolId::new("tool-ask-user"),
        name: ASK_USER_NAME.to_owned(),
        description: "Ask the user one bounded question with optional finite choices and optional free text. The answer is returned to the model as an ordinary tool result.".to_owned(),
        input_schema: input_schema::<AskUserInput>(),
        execution_policy: ToolExecutionPolicy::ForegroundOnly,
        concurrency_policy: ToolConcurrencyPolicy::Sequential,
        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
        replay_policy: ToolReplayPolicy::Never,
        origin: ToolOrigin::Builtin,
    }
}

struct AskUserExecutor;

impl ToolExecutor for AskUserExecutor {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move {
            let input = match AskUserInput::parse(&invocation.arguments) {
                Ok(input) => input,
                Err(error) => return failed_result(error),
            };
            let Some(interaction) = context.interaction.as_ref() else {
                return failed_result("ask_user interaction provider unavailable");
            };
            let Some(attempt_id) = context.attempt_id else {
                return failed_result("ask_user requires an active Agent Loop attempt");
            };
            let Some(cancellation) = context.agent_cancellation else {
                return failed_result("ask_user requires an active attempt cancellation authority");
            };
            let outcome = interaction
                .request_question(
                    attempt_id.clone(),
                    QuestionFacts {
                        turn: context.turn,
                        prompt: input.prompt,
                        choices: input.choices,
                        allow_free_text: input.allow_free_text,
                    },
                    cancellation,
                )
                .await;
            match outcome {
                InteractionOutcome::Answered {
                    response: InteractionResponse::Question { answer },
                } => answer_result(answer),
                InteractionOutcome::Answered { .. } => {
                    failed_result("ask_user received a mismatched interaction response")
                }
                InteractionOutcome::Cancelled { reason } => ToolExecutionResult {
                    status: ToolExecutionStatus::Cancelled { reason },
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                },
                InteractionOutcome::Unavailable => {
                    failed_result("ask_user interaction provider unavailable")
                }
            }
        })
    }
}

fn answer_result(answer: QuestionAnswer) -> ToolExecutionResult {
    let (kind, value) = match answer {
        QuestionAnswer::Choice { value } => ("choice", value),
        QuestionAnswer::FreeText { value } => ("free_text", value),
    };
    success_json(serde_json::json!({"answer": value, "kind": kind}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentCancellation;
    use crate::runtime::identity::{AttemptId, ConversationId, ToolCallId, ToolId};
    use crate::runtime::types::CancellationReason;
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::managed_output::ManagedToolOutput;
    use crate::tools::types::ToolInvocationMode;
    use crate::tools::workspace::Workspace;
    use futures_util::future::ready;
    use tempfile::TempDir;

    struct NoopProgress;

    impl crate::tools::executor::ProgressReporter for NoopProgress {
        fn report(&self, _progress: crate::tools::types::ToolProgress) {}
    }

    struct ScriptedQuestion;

    impl crate::runtime::ToolInteraction for ScriptedQuestion {
        fn request_question<'a>(
            &'a self,
            _attempt_id: AttemptId,
            _facts: QuestionFacts,
            _cancellation: &'a AgentCancellation,
        ) -> BoxFuture<'a, InteractionOutcome> {
            Box::pin(ready(InteractionOutcome::Answered {
                response: InteractionResponse::Question {
                    answer: QuestionAnswer::FreeText {
                        value: "typed answer".to_owned(),
                    },
                },
            }))
        }
    }

    fn context<'a>(
        dir: &'a TempDir,
        interaction: Option<std::sync::Arc<dyn crate::runtime::ToolInteraction>>,
        cancellation: &'a AgentCancellation,
    ) -> ToolExecutionContext<'a> {
        let workspace = Box::leak(Box::new(Workspace::new(dir.path()).expect("workspace")));
        let conversation_id = ConversationId::new("ask-user-test");
        let artifacts = Box::leak(Box::new(
            ArtifactStore::new(conversation_id.clone(), dir.path().join("artifacts"))
                .expect("artifacts"),
        ));
        let tool_output = Box::leak(Box::new(
            ManagedToolOutput::new(conversation_id.clone(), dir.path().join("tool-output"))
                .expect("tool output"),
        ));
        let environment = Box::leak(Box::new(ToolEnvironment::new()));
        let progress = Box::leak(Box::new(NoopProgress));
        let conversation_id = Box::leak(Box::new(conversation_id));
        let attempt_id = Box::leak(Box::new(AttemptId::new("ask-user-attempt")));
        ToolExecutionContext {
            conversation_id,
            execution_id: None,
            cancellation: crate::runtime::ExecutionCancellation::detached(
                cancellation.signal(),
                cancellation.reason(),
            ),
            workspace,
            progress,
            artifacts,
            tool_output,
            environment,
            skill_resources: None,
            interaction,
            attempt_id: Some(attempt_id),
            turn: 1,
            agent_cancellation: Some(cancellation),
        }
    }

    fn invocation() -> ToolInvocation {
        ToolInvocation {
            call_id: ToolCallId::new("ask-user-call"),
            tool_id: ToolId::new("tool-ask-user"),
            tool_name: ASK_USER_NAME.to_owned(),
            mode: ToolInvocationMode::Foreground,
            arguments: serde_json::json!({
                "prompt": "What should I use?",
                "allow_free_text": true
            }),
        }
    }

    #[tokio::test]
    async fn ask_user_returns_a_normal_success_tool_result() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let context = context(
            &dir,
            Some(std::sync::Arc::new(ScriptedQuestion)),
            &cancellation,
        );
        let result = AskUserExecutor.execute(invocation(), context).await;
        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(
            result.content,
            vec![crate::tools::types::ToolResultContent::Json {
                value: serde_json::json!({"answer": "typed answer", "kind": "free_text"})
            }]
        );
    }

    #[tokio::test]
    async fn ask_user_fails_explicitly_without_an_interaction_provider() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let context = context(&dir, None, &cancellation);
        let result = AskUserExecutor.execute(invocation(), context).await;
        assert!(matches!(result.status, ToolExecutionStatus::Failed { .. }));
    }
}
