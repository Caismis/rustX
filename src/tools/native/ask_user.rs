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

use crate::runtime::interaction::QuestionFacts;
use crate::runtime::{InteractionOutcome, InteractionResponse, QuestionAnswer};
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, input_schema};
use crate::tools::native::support::{failed_result, success_json};
use crate::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
    ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy,
};

/// The canonical model-facing name of the native question tool.
pub const ASK_USER_NAME: &str = crate::tools::executor::ASK_USER_TOOL_NAME;

/// The canonical, preflight-normalized input contract of `ask_user`.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct AskUserInput {
    /// The question shown to the user.
    prompt: String,
    /// Optional finite answer choices.
    #[serde(skip_serializing_if = "Option::is_none")]
    choices: Option<Vec<String>>,
    /// Whether a free-text answer is accepted.
    allow_free_text: bool,
}

impl AskUserInput {
    /// Parses, validates, and normalizes model arguments at the registry
    /// preflight boundary. The returned value always contains an explicit
    /// `allow_free_text` mode, so a successful `PreparedInvocation` is an
    /// answerable Question and the executor need not rediscover semantic
    /// argument errors.
    fn from_wire(arguments: &serde_json::Value) -> Result<Self, String> {
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

    fn normalize(arguments: &serde_json::Value) -> Result<serde_json::Value, String> {
        let input = Self::from_wire(arguments)?;
        serde_json::to_value(input)
            .map_err(|error| format!("failed to normalize ask_user arguments: {error}"))
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
        .with_normalizer(AskUserInput::normalize)
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
            let input: AskUserInput = match serde_json::from_value(invocation.arguments) {
                Ok(input) => input,
                Err(error) => {
                    return failed_result(format!(
                        "ask_user received an invocation that was not preflight-normalized: {error}"
                    ));
                }
            };
            let Some(requester) = context.question_requester() else {
                return failed_result("ask_user interaction provider unavailable");
            };
            let outcome = requester.request_question(input.question_facts(0)).await;
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
    use crate::runtime::identity::{AttemptId, ConversationId, InteractionId, ToolCallId, ToolId};
    use crate::runtime::interaction::{
        InteractionCoordinator, InteractionObserver, InteractionRequest, QuestionRequester,
    };
    use crate::runtime::types::{CancellationReason, ConversationLifecycle};
    use crate::runtime::{CancellationSignal, ExecutionCancellation, InteractionKind};
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::executor::{PreflightOutcome, ToolRegistry};
    use crate::tools::managed_output::ManagedToolOutput;
    use crate::tools::types::{ToolCall, ToolInvocationMode};
    use crate::tools::workspace::Workspace;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio::sync::oneshot;

    struct NoopProgress;

    impl crate::tools::executor::ProgressReporter for NoopProgress {
        fn report(&self, _progress: crate::tools::types::ToolProgress) {}
    }

    fn context(
        dir: &TempDir,
        requester: Option<QuestionRequester>,
        cancellation: ExecutionCancellation,
    ) -> ToolExecutionContext<'_> {
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
        let context = ToolExecutionContext::new(
            conversation_id,
            None,
            cancellation,
            workspace,
            progress,
            artifacts,
            tool_output,
            environment,
        );
        match requester {
            Some(requester) => context.with_question_requester(requester),
            None => context,
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
        let registration = registration();
        registry
            .register_with_argument_normalizer(
                registration.definition,
                registration.executor,
                registration.normalizer,
            )
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
        fn on_pending(
            &self,
            request: &InteractionRequest,
            _audit: &crate::events::types::RuntimeEventEnvelope,
        ) {
            if let Some(sender) = self.sender.lock().expect("pending probe lock").take() {
                let _ = sender.send(request.clone());
            }
        }

        fn on_settled(
            &self,
            _interaction_id: &InteractionId,
            _outcome: &InteractionOutcome,
            _audit: Option<&crate::events::types::RuntimeEventEnvelope>,
        ) {
        }
    }

    /// The live coordinator and its durable audit capability, returned so a
    /// regression can assert the interaction plane as well as the tool result.
    struct AskUserRun {
        coordinator: Arc<InteractionCoordinator>,
        audit: Arc<crate::runtime::interaction::RecordingInteractionAudit>,
        request: InteractionRequest,
        result: ToolExecutionResult,
    }

    async fn execute_through_real_coordinator(
        arguments: serde_json::Value,
        answer: QuestionAnswer,
    ) -> AskUserRun {
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
        let conversation_id = ConversationId::new("ask-user-production-conversation");
        let audit =
            crate::runtime::interaction::RecordingInteractionAudit::new(conversation_id.clone());
        let coordinator = Arc::new(InteractionCoordinator::new(
            conversation_id,
            lifecycle,
            audit.clone(),
        ));
        coordinator.set_provider_available(true);
        let (pending_sender, pending_receiver) = oneshot::channel();
        coordinator.install_observer(Arc::new(PendingProbe {
            sender: Mutex::new(Some(pending_sender)),
        }));

        let dir = tempfile::tempdir().expect("temp dir");
        let cancellation = ExecutionCancellation::detached(
            CancellationSignal::new(),
            CancellationReason::UserRequested,
        );
        let requester = QuestionRequester::new(
            coordinator.clone(),
            AttemptId::new("ask-user-production-attempt"),
            cancellation.clone(),
            1,
        );
        let context = context(&dir, Some(requester), cancellation);
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
        AskUserRun {
            coordinator,
            audit,
            request,
            result,
        }
    }

    #[test]
    fn schema_and_preflight_agree_on_the_three_question_modes() {
        let schema = definition().input_schema;
        assert!(schema.get("anyOf").is_some(), "mode branches are explicit");
        let validator = jsonschema::Validator::new(&schema).expect("valid ask_user schema");

        let valid_arguments = [
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
        ];
        for arguments in valid_arguments {
            assert!(validator.is_valid(&arguments));
            assert!(matches!(preflight(arguments), PreflightOutcome::Ready(_)));
        }

        for arguments in [
            serde_json::json!({"prompt": "Choose", "allow_free_text": false}),
            serde_json::json!({"prompt": "Choose", "choices": []}),
            serde_json::json!({"prompt": "Choose", "choices": ["a"], "allow_free_text": null}),
            serde_json::json!({"prompt": "Choose", "unknown": true}),
        ] {
            assert!(!validator.is_valid(&arguments));
            assert!(matches!(
                preflight(arguments),
                PreflightOutcome::Rejected { .. }
            ));
        }

        let PreflightOutcome::Ready(bare) = preflight(serde_json::json!({
            "prompt": "What should I call it?"
        })) else {
            panic!("bare prompt should preflight");
        };
        assert_eq!(bare.invocation.arguments["allow_free_text"], true);

        let PreflightOutcome::Ready(choice_only) = preflight(serde_json::json!({
            "prompt": "Where?",
            "choices": ["staging", "production"]
        })) else {
            panic!("choice-only question should preflight");
        };
        assert_eq!(choice_only.invocation.arguments["allow_free_text"], false);

        let PreflightOutcome::Ready(mixed) = preflight(serde_json::json!({
            "prompt": "Where?",
            "choices": ["staging", "production"],
            "allow_free_text": true
        })) else {
            panic!("mixed question should preflight");
        };
        assert_eq!(mixed.invocation.arguments["allow_free_text"], true);
    }

    #[test]
    fn invalid_arguments_are_rejected_by_registry_preflight() {
        for (arguments, message) in [
            (
                serde_json::json!({"prompt": "Choose", "choices": []}),
                "choices must contain at least one",
            ),
            (
                serde_json::json!({"prompt": "Choose", "allow_free_text": false}),
                "allow_free_text must be true",
            ),
            (
                serde_json::json!({"prompt": "Choose", "allow_free_text": null}),
                "allow_free_text must be a boolean",
            ),
            (
                serde_json::json!({"prompt": "Choose", "choices": ["a", "a"]}),
                "choices must not contain duplicates",
            ),
        ] {
            let PreflightOutcome::Rejected { error, .. } = preflight(arguments) else {
                panic!("invalid ask_user arguments must be rejected during preflight");
            };
            assert!(
                error.contains(message),
                "{error:?} does not contain {message:?}"
            );
        }
    }

    #[test]
    fn preflight_uses_unicode_scalar_question_bounds() {
        let maximum_prompt = "🙂".repeat(4096);
        assert!(matches!(
            preflight(serde_json::json!({"prompt": maximum_prompt})),
            PreflightOutcome::Ready(_)
        ));
        let oversized_prompt = "🙂".repeat(4097);
        let PreflightOutcome::Rejected { error, .. } =
            preflight(serde_json::json!({"prompt": oversized_prompt}))
        else {
            panic!("prompt over the character bound must be rejected at preflight");
        };
        assert!(error.contains("4096 characters"), "{error}");

        let maximum_choice = "界".repeat(256);
        assert!(matches!(
            preflight(serde_json::json!({
                "prompt": "Choose",
                "choices": [maximum_choice]
            })),
            PreflightOutcome::Ready(_)
        ));
        let oversized_choice = "界".repeat(257);
        let PreflightOutcome::Rejected { error, .. } = preflight(serde_json::json!({
            "prompt": "Choose",
            "choices": [oversized_choice]
        })) else {
            panic!("choice over the character bound must be rejected at preflight");
        };
        assert!(error.contains("256 characters"), "{error}");
    }

    #[test]
    fn registry_preflight_rejects_structural_and_semantic_question_errors() {
        for arguments in [
            serde_json::json!({"prompt": ""}),
            serde_json::json!({"prompt": "Choose", "choices": [""]}),
            serde_json::json!({"prompt": "Choose", "choices": ["a", "a"]}),
            serde_json::json!({"prompt": "Choose", "choices": "a"}),
            serde_json::json!({"prompt": "Choose", "choices": null}),
            serde_json::json!({"prompt": "Choose", "allow_free_text": null}),
            serde_json::json!({"prompt": "Choose", "unknown": true}),
            serde_json::json!({"prompt": 7}),
            serde_json::json!({"prompt": "Choose", "allow_free_text": "yes"}),
            serde_json::json!({"prompt": "Choose", "choices": ["a"], "allow_free_text": 1}),
        ] {
            assert!(
                matches!(preflight(arguments), PreflightOutcome::Rejected { .. }),
                "invalid ask_user arguments must never become Ready"
            );
        }

        let too_many_choices = (0..=32)
            .map(|index| format!("choice-{index}"))
            .collect::<Vec<_>>();
        assert!(matches!(
            preflight(serde_json::json!({
                "prompt": "Choose",
                "choices": too_many_choices
            })),
            PreflightOutcome::Rejected { .. }
        ));

        let maximum_choices = (0..32)
            .map(|index| format!("choice-{index}"))
            .collect::<Vec<_>>();
        assert!(matches!(
            preflight(serde_json::json!({
                "prompt": "Choose",
                "choices": maximum_choices
            })),
            PreflightOutcome::Ready(_)
        ));
    }

    #[tokio::test]
    async fn production_path_publishes_bare_open_ended_question() {
        let AskUserRun {
            request, result, ..
        } = execute_through_real_coordinator(
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
        let AskUserRun {
            request, result, ..
        } = execute_through_real_coordinator(
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
        let AskUserRun {
            request, result, ..
        } = execute_through_real_coordinator(
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

    /// Issue #109 regression 6: one Question request and answer produce the
    /// durable requested/settled audit pair, and the live operation receives
    /// exactly one answer.
    ///
    /// The audit keeps the user's exact words as evidence; the canonical tool
    /// result carries the same answer to the model. Neither duplicates the
    /// other's job: the Journal records the human decision, canonical history
    /// records the conversation.
    #[tokio::test]
    async fn question_settlement_is_durable_audit_and_exactly_one_answer() {
        use crate::events::interaction::{InteractionSettlement, InteractionSubject};
        use crate::events::types::RuntimeEvent;

        let run = execute_through_real_coordinator(
            serde_json::json!({
                "prompt": "Where should I deploy?",
                "choices": ["staging", "production"]
            }),
            QuestionAnswer::Choice {
                value: "staging".to_owned(),
            },
        )
        .await;

        assert!(
            matches!(
                run.audit.events().as_slice(),
                [
                    RuntimeEvent::InteractionRequested {
                        interaction_id: requested_id,
                        subject: InteractionSubject::Question {
                            prompt,
                            choices: Some(choices),
                            allow_free_text: false,
                        },
                    },
                    RuntimeEvent::InteractionSettled {
                        interaction_id: settled_id,
                        settlement: InteractionSettlement::Answered {
                            answer: QuestionAnswer::Choice { value },
                        },
                    }
                ] if *requested_id == run.request.id
                    && settled_id == requested_id
                    && prompt == "Where should I deploy?"
                    && choices == &["staging".to_owned(), "production".to_owned()]
                    && value == "staging"
            ),
            "expected one requested/settled pair, saw {:?}",
            run.audit.events()
        );

        assert_eq!(run.result.status, ToolExecutionStatus::Success);
        assert_eq!(
            run.result.content,
            vec![crate::tools::types::ToolResultContent::Json {
                value: serde_json::json!({"answer": "staging", "kind": "choice"})
            }]
        );

        // The live operation already consumed its one answer; a second one is
        // refused and writes no further audit fact.
        assert!(matches!(
            run.coordinator.respond(
                &run.request.id,
                InteractionResponse::Question {
                    answer: QuestionAnswer::Choice {
                        value: "production".to_owned(),
                    },
                },
            ),
            Err(crate::runtime::interaction::InteractionError::NotPending { .. })
        ));
        assert_eq!(run.audit.events().len(), 2);
    }

    #[tokio::test]
    async fn production_path_cancellation_returns_one_cancelled_tool_result() {
        let registry = registry();
        let PreflightOutcome::Ready(prepared) = registry
            .preflight(&ToolCall {
                id: ToolCallId::new("ask-user-cancel-call"),
                tool_id: ToolId::new("tool-ask-user"),
                name: ASK_USER_NAME.to_owned(),
                arguments: serde_json::json!({"prompt": "Continue?"}),
            })
            .expect("ask_user identity resolves")
        else {
            panic!("valid ask_user arguments must preflight");
        };
        let lifecycle = ConversationLifecycle::new();
        assert!(lifecycle.activate());
        let conversation_id = ConversationId::new("ask-user-cancel-conversation");
        let coordinator = Arc::new(InteractionCoordinator::new(
            conversation_id.clone(),
            lifecycle,
            crate::runtime::interaction::RecordingInteractionAudit::new(conversation_id),
        ));
        coordinator.set_provider_available(true);
        let (pending_sender, pending_receiver) = oneshot::channel();
        coordinator.install_observer(Arc::new(PendingProbe {
            sender: Mutex::new(Some(pending_sender)),
        }));
        let signal = CancellationSignal::new();
        let cancellation =
            ExecutionCancellation::detached(signal.clone(), CancellationReason::RuntimeShutdown);
        let requester = QuestionRequester::new(
            coordinator.clone(),
            AttemptId::new("ask-user-cancel-attempt"),
            cancellation.clone(),
            1,
        );
        let dir = tempfile::tempdir().expect("temp dir");
        let context = context(&dir, Some(requester), cancellation);
        let executor = registry.executor(&prepared.invocation.tool_id);
        let mut execution = Box::pin(executor.execute(prepared.invocation, context));
        let request = tokio::select! {
            request = pending_receiver => request.expect("Question was published"),
            result = &mut execution => panic!("ask_user settled before cancellation: {result:?}"),
        };
        signal.cancel();
        let result = execution.await;
        assert!(matches!(
            result.status,
            ToolExecutionStatus::Cancelled {
                reason: CancellationReason::RuntimeShutdown
            }
        ));
        assert_eq!(coordinator.pending_count(), 0);
        assert!(matches!(
            coordinator.respond(
                &request.id,
                InteractionResponse::Question {
                    answer: QuestionAnswer::FreeText {
                        value: "late".to_owned(),
                    },
                },
            ),
            Err(crate::runtime::interaction::InteractionError::NotPending { .. })
        ));
    }

    #[tokio::test]
    async fn invalid_arguments_fail_at_preflight_before_provider_lookup() {
        let PreflightOutcome::Rejected { error, .. } = preflight(serde_json::json!({
            "prompt": "Choose",
            "choices": []
        })) else {
            panic!("invalid arguments must fail at preflight");
        };
        assert!(error.contains("choices must contain at least one"));
        assert!(!error.contains("provider unavailable"));
    }

    #[tokio::test]
    async fn ask_user_fails_explicitly_without_an_interaction_provider() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cancellation = ExecutionCancellation::detached(
            CancellationSignal::new(),
            CancellationReason::UserRequested,
        );
        let context = context(&dir, None, cancellation);
        let result = AskUserExecutor.execute(invocation(), context).await;
        assert!(matches!(result.status, ToolExecutionStatus::Failed { .. }));
    }
}
