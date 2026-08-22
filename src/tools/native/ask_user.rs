//! The native `ask_user` tool.
//!
//! `ask_user` is an ordinary foreground, sequential, approval-never Tool
//! whose executor publishes one bounded Question through the runtime-owned
//! `InteractionCoordinator`. It has no filesystem, network, process, or
//! authorization authority of its own.

use std::borrow::Cow;

use futures_util::future::BoxFuture;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Deserialize;

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

/// The normalized model-facing input contract of `ask_user`.
#[derive(Debug)]
struct AskUserInput {
    /// The question shown to the user.
    prompt: String,
    /// Optional finite answer choices.
    choices: Option<Vec<String>>,
    /// Whether a free-text answer is accepted.
    allow_free_text: bool,
}

impl AskUserInput {
    fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let wire: AskUserInputWire = serde_json::from_value(arguments.clone())
            .map_err(|error| format!("invalid ask_user arguments: {error}"))?;
        let choices = match wire.choices {
            None => None,
            Some(value) if value.is_null() => {
                return Err("choices must be an array when present".to_owned());
            }
            Some(value) => Some(
                serde_json::from_value(value)
                    .map_err(|error| format!("choices must be an array of strings: {error}"))?,
            ),
        };
        let requested_allow_free_text = match wire.allow_free_text {
            None => None,
            Some(value) => Some(
                value
                    .as_bool()
                    .ok_or_else(|| "allow_free_text must be a boolean when present".to_owned())?,
            ),
        };
        let allow_free_text = match (choices.as_ref(), requested_allow_free_text) {
            // A bare prompt is the ergonomic open-ended form.
            (None, None | Some(true)) | (Some(_), Some(true)) => true,
            (None, Some(false)) => {
                return Err(
                    "allow_free_text must be true when choices is omitted; omit it for an open-ended question or set it to true"
                        .to_owned(),
                );
            }
            // A choice list is choice-only unless the caller explicitly opts
            // into free text.
            (Some(_), None | Some(false)) => false,
        };
        let input = Self {
            prompt: wire.prompt,
            choices,
            allow_free_text,
        };
        input
            .question_facts(0)
            .validate()
            .map_err(|error| format!("invalid ask_user arguments: {error}"))?;
        Ok(input)
    }

    fn question_facts(&self, turn: u32) -> QuestionFacts {
        QuestionFacts {
            turn,
            prompt: self.prompt.clone(),
            choices: self.choices.clone(),
            allow_free_text: self.allow_free_text,
        }
    }
}

/// The raw wire shape used to preserve whether an optional field was omitted
/// or explicitly supplied as `null`/`false`.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AskUserInputWire {
    /// The question shown to the user.
    prompt: String,
    /// An omitted value is distinct from an explicit JSON `null`.
    #[serde(default, deserialize_with = "deserialize_present_value")]
    choices: Option<serde_json::Value>,
    /// An omitted value is distinct from an explicit JSON `null`.
    #[serde(default, deserialize_with = "deserialize_present_value")]
    allow_free_text: Option<serde_json::Value>,
}

fn deserialize_present_value<'de, D>(deserializer: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer).map(Some)
}

/// The schema-only representation of the three valid answer modes.
///
/// This is separate from [`AskUserInput`] because the runtime normalizes the
/// omitted `allow_free_text` field before publishing a Question, while the
/// public schema must still distinguish choice-only input from an explicit
/// choice-plus-free-text request.
#[derive(Debug, JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)]
enum AskUserInputSchema {
    /// `{ "prompt": "..." }` or the explicit free-text form without choices.
    OpenEnded(AskUserOpenEndedSchema),
    /// A non-empty choice list with omitted/false free-text mode.
    ChoiceOnly(AskUserChoiceOnlySchema),
    /// A non-empty choice list with explicitly enabled free text.
    ChoiceAndFreeText(AskUserChoiceAndFreeTextSchema),
}

#[derive(Debug, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct AskUserOpenEndedSchema {
    /// The question shown to the user.
    #[schemars(length(min = 1, max = 4096))]
    prompt: String,
    /// Optional explicit spelling of the open-ended mode; only `true` is valid.
    #[schemars(extend("const" = true))]
    allow_free_text: Option<bool>,
}

#[derive(Debug, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct AskUserChoiceOnlySchema {
    /// The question shown to the user.
    #[schemars(length(min = 1, max = 4096))]
    prompt: String,
    /// At least one finite choice is required when choices are present.
    #[schemars(length(min = 1, max = 32), inner(length(min = 1, max = 256)))]
    choices: Vec<String>,
    /// Optional explicit spelling of choice-only mode; only `false` is valid.
    #[schemars(extend("const" = false))]
    allow_free_text: Option<bool>,
}

#[derive(Debug, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct AskUserChoiceAndFreeTextSchema {
    /// The question shown to the user.
    #[schemars(length(min = 1, max = 4096))]
    prompt: String,
    /// At least one finite choice is required when choices are present.
    #[schemars(length(min = 1, max = 32), inner(length(min = 1, max = 256)))]
    choices: Vec<String>,
    /// Explicit opt-in for answers outside the listed choices.
    #[schemars(extend("const" = true))]
    allow_free_text: bool,
}

impl JsonSchema for AskUserInput {
    fn schema_name() -> Cow<'static, str> {
        "AskUserInput".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::AskUserInput").into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = AskUserInputSchema::json_schema(generator);
        schema
            .as_object_mut()
            .expect("ask_user schema is an object schema")
            .insert("type".to_owned(), serde_json::json!("object"));
        schema
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
            let Some(attempt_id) = context.attempt_id else {
                return failed_result("ask_user requires an active Agent Loop attempt");
            };
            let Some(cancellation) = context.agent_cancellation else {
                return failed_result("ask_user requires an active attempt cancellation authority");
            };
            let facts = input.question_facts(context.turn);
            if let Err(error) = facts.validate() {
                return failed_result(format!("invalid ask_user arguments: {error}"));
            }
            let Some(interaction) = context.interaction.as_ref() else {
                return failed_result("ask_user interaction provider unavailable");
            };
            let outcome = interaction
                .request_question(attempt_id.clone(), facts, cancellation)
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
    use crate::runtime::identity::{AttemptId, ConversationId, InteractionId, ToolCallId, ToolId};
    use crate::runtime::interaction::{
        InteractionCoordinator, InteractionObserver, InteractionRequest,
    };
    use crate::runtime::types::{CancellationReason, ConversationLifecycle};
    use crate::runtime::{InteractionKind, ToolInteraction};
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::executor::{PreflightOutcome, ToolRegistry};
    use crate::tools::managed_output::ManagedToolOutput;
    use crate::tools::types::{ToolCall, ToolInvocationMode};
    use crate::tools::workspace::Workspace;
    use futures_util::future::ready;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio::sync::oneshot;

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

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry
            .register(definition(), Arc::new(AskUserExecutor))
            .expect("ask_user registration");
        registry
    }

    fn preflight(arguments: serde_json::Value) -> PreflightOutcome {
        registry()
            .preflight(&ToolCall {
                id: ToolCallId::new("ask-user-preflight-call"),
                tool_id: ToolId::new("tool-ask-user"),
                name: ASK_USER_NAME.to_owned(),
                arguments,
            })
            .expect("ask_user identity resolves")
    }

    #[derive(Default)]
    struct PendingProbe {
        sender: Mutex<Option<oneshot::Sender<InteractionRequest>>>,
    }

    impl InteractionObserver for PendingProbe {
        fn on_pending(&self, request: &InteractionRequest) {
            if let Some(sender) = self.sender.lock().expect("pending probe lock").take() {
                let _ = sender.send(request.clone());
            }
        }

        fn on_settled(&self, _interaction_id: &InteractionId, _outcome: &InteractionOutcome) {}
    }

    async fn execute_through_real_coordinator(
        arguments: serde_json::Value,
        answer: QuestionAnswer,
    ) -> (InteractionRequest, ToolExecutionResult) {
        let registry = registry();
        let PreflightOutcome::Ready(prepared) = registry
            .preflight(&ToolCall {
                id: ToolCallId::new("ask-user-production-call"),
                tool_id: ToolId::new("tool-ask-user"),
                name: ASK_USER_NAME.to_owned(),
                arguments,
            })
            .expect("ask_user identity resolves")
        else {
            panic!("ask_user arguments must be schema-valid");
        };

        let lifecycle = ConversationLifecycle::new();
        assert!(lifecycle.activate());
        let coordinator = Arc::new(InteractionCoordinator::new(
            ConversationId::new("ask-user-production-conversation"),
            lifecycle,
        ));
        coordinator.set_provider_available(true);
        let (pending_sender, pending_receiver) = oneshot::channel();
        coordinator.install_observer(Arc::new(PendingProbe {
            sender: Mutex::new(Some(pending_sender)),
        }));

        let interaction: Arc<dyn ToolInteraction> = coordinator.clone();
        let dir = tempfile::tempdir().expect("temp dir");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let context = context(&dir, Some(interaction), &cancellation);
        let executor = registry.executor(&prepared.invocation.tool_id);
        let mut execution = Box::pin(executor.execute(prepared.invocation, context));
        let request = tokio::select! {
            request = pending_receiver => request.expect("Question was published"),
            result = &mut execution => panic!("ask_user settled before publishing: {result:?}"),
        };
        coordinator
            .respond(&request.id, InteractionResponse::Question { answer })
            .expect("Question response accepted");
        let result = execution.await;
        (request, result)
    }

    #[test]
    fn schema_and_preflight_agree_on_the_three_question_modes() {
        let schema = definition().input_schema;
        assert!(schema.get("anyOf").is_some(), "mode branches are explicit");

        for arguments in [
            serde_json::json!({"prompt": "What should I call it?"}),
            serde_json::json!({
                "prompt": "Where should I deploy?",
                "choices": ["staging", "production"]
            }),
            serde_json::json!({
                "prompt": "Where should I deploy?",
                "choices": ["staging", "production"],
                "allow_free_text": true
            }),
        ] {
            assert!(matches!(preflight(arguments), PreflightOutcome::Ready(_)));
        }

        for arguments in [
            serde_json::json!({"prompt": "Choose", "choices": []}),
            serde_json::json!({"prompt": "Choose", "allow_free_text": false}),
            serde_json::json!({
                "prompt": "Choose",
                "choices": ["a"],
                "allow_free_text": null
            }),
        ] {
            assert!(matches!(
                preflight(arguments),
                PreflightOutcome::Rejected { .. }
            ));
        }
    }

    #[test]
    fn invalid_answer_modes_have_clear_argument_errors() {
        let empty_choices = AskUserInput::parse(&serde_json::json!({
            "prompt": "Choose",
            "choices": []
        }))
        .expect_err("empty choices are invalid");
        assert!(empty_choices.contains("choices must contain at least one"));

        let disabled_open_ended = AskUserInput::parse(&serde_json::json!({
            "prompt": "Choose",
            "allow_free_text": false
        }))
        .expect_err("an open-ended question must allow free text");
        assert!(disabled_open_ended.contains("allow_free_text must be true"));

        let null_mode = AskUserInput::parse(&serde_json::json!({
            "prompt": "Choose",
            "allow_free_text": null
        }))
        .expect_err("null is not an answer mode");
        assert!(null_mode.contains("allow_free_text must be a boolean"));
    }

    #[tokio::test]
    async fn production_path_publishes_bare_open_ended_question() {
        let (request, result) = execute_through_real_coordinator(
            serde_json::json!({"prompt": "What should I call it?"}),
            QuestionAnswer::FreeText {
                value: "typed answer".to_owned(),
            },
        )
        .await;
        assert!(matches!(
            request.kind,
            InteractionKind::Question {
                choices: None,
                allow_free_text: true,
                ..
            }
        ));
        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(
            result.content,
            vec![crate::tools::types::ToolResultContent::Json {
                value: serde_json::json!({"answer": "typed answer", "kind": "free_text"})
            }]
        );
    }

    #[tokio::test]
    async fn production_path_publishes_choice_only_question() {
        let (request, result) = execute_through_real_coordinator(
            serde_json::json!({
                "prompt": "Where should I deploy?",
                "choices": ["staging", "production"]
            }),
            QuestionAnswer::Choice {
                value: "staging".to_owned(),
            },
        )
        .await;
        assert!(matches!(
            request.kind,
            InteractionKind::Question {
                choices: Some(_),
                allow_free_text: false,
                ..
            }
        ));
        assert_eq!(result.status, ToolExecutionStatus::Success);
    }

    #[tokio::test]
    async fn production_path_publishes_choice_and_free_text_question() {
        let (request, result) = execute_through_real_coordinator(
            serde_json::json!({
                "prompt": "Where should I deploy?",
                "choices": ["staging", "production"],
                "allow_free_text": true
            }),
            QuestionAnswer::FreeText {
                value: "a private environment".to_owned(),
            },
        )
        .await;
        assert!(matches!(
            request.kind,
            InteractionKind::Question {
                choices: Some(_),
                allow_free_text: true,
                ..
            }
        ));
        assert_eq!(result.status, ToolExecutionStatus::Success);
    }

    #[tokio::test]
    async fn invalid_arguments_fail_before_provider_lookup() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let context = context(&dir, None, &cancellation);
        let result = AskUserExecutor
            .execute(
                ToolInvocation {
                    arguments: serde_json::json!({
                        "prompt": "Choose",
                        "choices": []
                    }),
                    ..invocation()
                },
                context,
            )
            .await;
        let ToolExecutionStatus::Failed { error } = result.status else {
            panic!("invalid arguments must fail as a ToolResult");
        };
        assert!(error.contains("choices must contain at least one"));
        assert!(!error.contains("provider unavailable"));
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
