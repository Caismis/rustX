//! The native `ask_user` tool.
//!
//! `ask_user` is an ordinary foreground, sequential, approval-never Tool. One
//! invocation contains one bounded questionnaire and publishes exactly one
//! interaction through the runtime-owned `InteractionCoordinator`.

use futures_util::future::BoxFuture;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::events::{
    CustomAnswer, MultipleOptionAnswer, OptionSpecification, QuestionSpecification,
    QuestionnaireAnswer, QuestionnaireAnswerEntry, QuestionnaireResponse,
    QuestionnaireSpecification, QuestionnaireSubmission, SingleOptionAnswer,
    normalize_questionnaire_response, validate_questionnaire,
};
use crate::runtime::interaction::QuestionnaireFacts;
use crate::runtime::{InteractionOutcome, InteractionResponse};
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, input_schema};
use crate::tools::native::support::{cancelled_result, failed_result, success_json};
use crate::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
    ToolInvocation, ToolOrigin, ToolReplayPolicy,
};

/// The canonical model-facing name of the native questionnaire tool.
pub const ASK_USER_NAME: &str = crate::tools::executor::ASK_USER_TOOL_NAME;

/// The model-facing input shape. `multi_select` is optional in the model
/// contract and defaults to false through Serde and the generated schema.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AskUserInput {
    /// One to four related blocking questions.
    #[schemars(length(min = 1, max = 4), inner(length(min = 1, max = 4096)))]
    questions: Vec<AskUserQuestionInput>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AskUserQuestionInput {
    /// The full question text.
    #[schemars(length(min = 1, max = 4096), pattern(r"\S"))]
    question: String,
    /// The short tab label.
    #[schemars(length(min = 1, max = 16), pattern(r"\S"))]
    header: String,
    /// Two to four authored options. The client supplies the custom-answer row.
    #[schemars(length(min = 2, max = 4), inner(length(min = 1, max = 60)))]
    options: Vec<AskUserOptionInput>,
    /// Optional; omitted means single-select.
    #[serde(default)]
    multi_select: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AskUserOptionInput {
    /// The authored option label.
    #[schemars(length(min = 1, max = 60), pattern(r"\S"))]
    label: String,
    /// The option's meaning and trade-offs.
    #[schemars(length(min = 1, max = 1024), pattern(r"\S"))]
    description: String,
    /// Optional Markdown preview, only for single-select questions.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(length(min = 1, max = 8192), pattern(r"\S"))]
    preview: Option<String>,
}

fn deserialize_non_null_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

impl AskUserInput {
    fn from_wire(arguments: &serde_json::Value) -> Result<Self, String> {
        let input: Self = serde_json::from_value(arguments.clone())
            .map_err(|error| format!("invalid ask_user arguments: {error}"))?;
        validate_questionnaire(&input.specification())
            .map_err(|error| format!("invalid ask_user arguments: {error}"))?;
        Ok(input)
    }

    fn normalize(arguments: &serde_json::Value) -> Result<serde_json::Value, String> {
        let input = Self::from_wire(arguments)?;
        serde_json::to_value(input)
            .map_err(|error| format!("failed to normalize ask_user arguments: {error}"))
    }

    fn specification(&self) -> QuestionnaireSpecification {
        QuestionnaireSpecification {
            questions: self
                .questions
                .iter()
                .map(|question| QuestionSpecification {
                    question: question.question.clone(),
                    header: question.header.clone(),
                    options: question
                        .options
                        .iter()
                        .map(|option| OptionSpecification {
                            label: option.label.clone(),
                            description: option.description.clone(),
                            preview: option.preview.clone(),
                        })
                        .collect(),
                    multi_select: question.multi_select,
                })
                .collect(),
        }
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
        description: "Ask the user one structured questionnaire for a blocking decision. Group related blocking questions into one invocation instead of issuing consecutive ask_user calls. Use 1–4 questions, each with 2–4 authored options; every option needs a concise label and a description of its meaning or trade-offs. A custom answer is always available at runtime, so do not add an Other or custom-answer option. If one option is recommended, place it first and append (Recommended) to its label. multi_select defaults to false; previews are for single-select questions only. The user's answers are returned as an ordinary structured tool result.".to_owned(),
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
            let specification = input.specification();
            if let Err(error) = validate_questionnaire(&specification) {
                return failed_result(format!("invalid ask_user arguments: {error}"));
            }
            let Some(requester) = context.questionnaire_requester() else {
                return failed_result("ask_user interaction provider unavailable");
            };
            let outcome = requester
                .request_questionnaire(QuestionnaireFacts {
                    turn: 0,
                    questionnaire: specification.clone(),
                })
                .await;
            match outcome {
                Ok(InteractionOutcome::Responded {
                    response: InteractionResponse::Questionnaire { response },
                }) => questionnaire_result(&specification, &response),
                Ok(InteractionOutcome::Responded { .. }) => {
                    failed_result("ask_user received a mismatched interaction response")
                }
                Ok(InteractionOutcome::Cancelled { reason }) => cancelled_result(reason),
                Err(failure) if failure.is_unavailable() => {
                    failed_result("ask_user interaction provider unavailable")
                }
                Err(_) => failed_result("ask_user interaction control path failed"),
            }
        })
    }
}

fn questionnaire_result(
    specification: &QuestionnaireSpecification,
    response: &QuestionnaireResponse,
) -> ToolExecutionResult {
    let response = match normalize_questionnaire_response(specification, response) {
        Ok(response) => response,
        Err(error) => return failed_result(format!("invalid questionnaire response: {error}")),
    };
    match response {
        QuestionnaireResponse::Declined => success_json(serde_json::json!({
            "cancelled": true,
            "answers": []
        })),
        QuestionnaireResponse::Submitted(QuestionnaireSubmission { answers }) => {
            let answers = answers
                .into_iter()
                .map(|entry: QuestionnaireAnswerEntry| {
                    let question = &specification.questions[entry.question_index];
                    let mut result = serde_json::json!({
                        "question_index": entry.question_index,
                        "question": question.question,
                        "header": question.header,
                    });
                    match entry.answer {
                        QuestionnaireAnswer::SingleOption(SingleOptionAnswer { label }) => {
                            result["kind"] = serde_json::json!("option");
                            result["answer"] = serde_json::json!(label);
                        }
                        QuestionnaireAnswer::Custom(CustomAnswer { answer }) => {
                            result["kind"] = serde_json::json!("custom");
                            result["answer"] = serde_json::json!(answer);
                        }
                        QuestionnaireAnswer::MultipleOption(MultipleOptionAnswer { selected }) => {
                            result["kind"] = serde_json::json!("multiple");
                            result["selected"] = serde_json::json!(selected);
                        }
                    }
                    result
                })
                .collect::<Vec<_>>();
            success_json(serde_json::json!({
                "cancelled": false,
                "answers": answers
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        MAX_CUSTOM_ANSWER_CHARS, MAX_OPTION_DESCRIPTION_CHARS, MAX_OPTION_LABEL_CHARS,
        MAX_OPTION_PREVIEW_CHARS, MAX_QUESTION_HEADER_CHARS, MAX_QUESTION_TEXT_CHARS,
        MAX_QUESTIONNAIRE_OPTIONS, MAX_QUESTIONNAIRE_QUESTIONS, MIN_QUESTIONNAIRE_OPTIONS,
    };
    use crate::runtime::identity::ToolId;
    use crate::tools::executor::{PreflightOutcome, ToolRegistry};
    use crate::tools::types::{ToolCall, ToolExecutionStatus, ToolResultContent};

    fn option(label: &str) -> serde_json::Value {
        serde_json::json!({
            "label": label,
            "description": format!("Trade-offs for {label}.")
        })
    }

    fn question(index: usize) -> serde_json::Value {
        serde_json::json!({
            "question": format!("Which direction {index}?"),
            "header": format!("Choice {index}"),
            "options": [option("First"), option("Second")]
        })
    }

    fn valid(count: usize) -> serde_json::Value {
        serde_json::json!({
            "questions": (0..count).map(question).collect::<Vec<_>>()
        })
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

    fn preflight(registry: &ToolRegistry, arguments: serde_json::Value) -> bool {
        matches!(
            registry.preflight(&ToolCall {
                id: crate::runtime::identity::ToolCallId::new("ask-user-test"),
                tool_id: ToolId::new("tool-ask-user"),
                name: ASK_USER_NAME.to_owned(),
                arguments,
            }),
            Ok(PreflightOutcome::Ready(_))
        )
    }

    #[test]
    fn schema_is_one_plain_questionnaire_object() {
        let schema = definition().input_schema;
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema.get("anyOf").is_none());
        assert!(schema.get("oneOf").is_none());
        assert_eq!(schema["required"], serde_json::json!(["questions"]));
        assert_eq!(schema["properties"]["questions"]["minItems"], 1);
        assert_eq!(schema["properties"]["questions"]["maxItems"], 4);
        let question_schema = &schema["properties"]["questions"]["items"];
        assert_eq!(
            question_schema["required"],
            serde_json::json!(["question", "header", "options"])
        );
        assert_eq!(question_schema["additionalProperties"], false);
        assert_eq!(question_schema["properties"]["options"]["minItems"], 2);
        assert_eq!(question_schema["properties"]["options"]["maxItems"], 4);
        let option_schema = &question_schema["properties"]["options"]["items"];
        assert_eq!(
            option_schema["required"],
            serde_json::json!(["label", "description"])
        );
        assert_eq!(option_schema["additionalProperties"], false);
        assert!(option_schema["properties"].get("preview").is_some());
        assert_eq!(
            question_schema["properties"]["multi_select"]["type"],
            "boolean"
        );
        assert_eq!(
            question_schema["properties"]["multi_select"]["default"],
            false
        );
    }

    #[test]
    fn valid_one_and_four_question_payloads_preflight() {
        let registry = registry();
        assert!(preflight(&registry, valid(1)));
        assert!(preflight(&registry, valid(4)));
    }

    #[test]
    fn malformed_stringified_values_are_rejected_without_coercion() {
        let mut malformed = valid(1);
        malformed["questions"][0]["multi_select"] = serde_json::json!("true");
        malformed["questions"][0]["options"] = serde_json::json!(
            "[{\"label\":\"First\",\"description\":\"one\"},{\"label\":\"Second\",\"description\":\"two\"}]"
        );
        assert!(!preflight(&registry(), malformed.clone()));
        assert!(AskUserInput::from_wire(&malformed).is_err());

        let reported_shape = serde_json::json!({
            "allow_free_text": "true",
            "choices": "[\"Swiss style\", \"Electronic magazine style\"]",
            "prompt": "Which visual style should I use?"
        });
        assert!(!preflight(&registry(), reported_shape.clone()));
        assert!(AskUserInput::from_wire(&reported_shape).is_err());
    }

    #[test]
    fn result_is_derived_from_request_facts_and_canonical_answers() {
        let mut specification = AskUserInput::from_wire(&valid(2))
            .expect("fixture questionnaire")
            .specification();
        specification.questions[1].multi_select = true;
        let result = questionnaire_result(
            &specification,
            &QuestionnaireResponse::Submitted(QuestionnaireSubmission {
                answers: vec![
                    QuestionnaireAnswerEntry {
                        question_index: 1,
                        answer: QuestionnaireAnswer::MultipleOption(MultipleOptionAnswer {
                            selected: vec!["Second".to_owned(), "First".to_owned()],
                        }),
                    },
                    QuestionnaireAnswerEntry {
                        question_index: 0,
                        answer: QuestionnaireAnswer::Custom(CustomAnswer {
                            answer: "user-defined".to_owned(),
                        }),
                    },
                ],
            }),
        );
        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(
            result.content,
            vec![ToolResultContent::Json {
                value: serde_json::json!({
                    "cancelled": false,
                    "answers": [
                        {
                            "question_index": 0,
                            "question": "Which direction 0?",
                            "header": "Choice 0",
                            "kind": "custom",
                            "answer": "user-defined"
                        },
                        {
                            "question_index": 1,
                            "question": "Which direction 1?",
                            "header": "Choice 1",
                            "kind": "multiple",
                            "selected": ["First", "Second"]
                        }
                    ]
                })
            }]
        );

        let declined = questionnaire_result(&specification, &QuestionnaireResponse::Declined);
        assert_eq!(declined.status, ToolExecutionStatus::Success);
        assert_eq!(
            declined.content,
            vec![ToolResultContent::Json {
                value: serde_json::json!({"cancelled": true, "answers": []})
            }]
        );
    }

    #[test]
    fn schema_bounds_and_unknown_fields_are_enforced_by_preflight() {
        let registry = registry();
        let mut unknown = valid(1);
        unknown["unexpected"] = serde_json::json!(true);
        assert!(!preflight(&registry, unknown));
        let mut reserved = valid(1);
        reserved["questions"][0]["options"][0]["label"] = "Other".into();
        assert!(!preflight(&registry, reserved));
        let mut too_many = valid(1);
        too_many["questions"] = serde_json::json!((0..5).map(question).collect::<Vec<_>>());
        assert!(!preflight(&registry, too_many));
    }

    #[test]
    fn every_model_facing_questionnaire_bound_is_preflighted() {
        let registry = registry();
        let rejected = |value: serde_json::Value| {
            assert!(
                !preflight(&registry, value.clone()),
                "expected rejection: {value}"
            );
        };

        let mut empty_questions = valid(1);
        empty_questions["questions"] = serde_json::json!([]);
        rejected(empty_questions);

        let mut empty_question = valid(1);
        empty_question["questions"][0]["question"] = "".into();
        rejected(empty_question);

        let mut long_question = valid(1);
        long_question["questions"][0]["question"] =
            serde_json::json!("é".repeat(MAX_QUESTION_TEXT_CHARS + 1));
        rejected(long_question);

        let mut empty_header = valid(1);
        empty_header["questions"][0]["header"] = "".into();
        rejected(empty_header);

        let mut long_header = valid(1);
        long_header["questions"][0]["header"] =
            serde_json::json!("h".repeat(MAX_QUESTION_HEADER_CHARS + 1));
        rejected(long_header);

        let mut too_few_options = valid(1);
        too_few_options["questions"][0]["options"] = serde_json::json!([option("First")]);
        rejected(too_few_options);

        let mut too_many_options = valid(1);
        too_many_options["questions"][0]["options"] = serde_json::json!([
            option("A"),
            option("B"),
            option("C"),
            option("D"),
            option("E")
        ]);
        rejected(too_many_options);

        let mut empty_label = valid(1);
        empty_label["questions"][0]["options"][0]["label"] = "".into();
        rejected(empty_label);

        let mut long_label = valid(1);
        long_label["questions"][0]["options"][0]["label"] =
            serde_json::json!("l".repeat(MAX_OPTION_LABEL_CHARS + 1));
        rejected(long_label);

        let mut empty_description = valid(1);
        empty_description["questions"][0]["options"][0]["description"] = "".into();
        rejected(empty_description);

        let mut long_description = valid(1);
        long_description["questions"][0]["options"][0]["description"] =
            serde_json::json!("d".repeat(MAX_OPTION_DESCRIPTION_CHARS + 1));
        rejected(long_description);

        let mut empty_preview = valid(1);
        empty_preview["questions"][0]["options"][0]["preview"] = "".into();
        rejected(empty_preview);

        let mut null_preview = valid(1);
        null_preview["questions"][0]["options"][0]["preview"] = serde_json::Value::Null;
        rejected(null_preview);

        let mut long_preview = valid(1);
        long_preview["questions"][0]["options"][0]["preview"] =
            serde_json::json!("p".repeat(MAX_OPTION_PREVIEW_CHARS + 1));
        rejected(long_preview);

        let mut duplicate_questions = valid(2);
        duplicate_questions["questions"][1]["question"] =
            duplicate_questions["questions"][0]["question"].clone();
        rejected(duplicate_questions);

        let mut duplicate_labels = valid(1);
        duplicate_labels["questions"][0]["options"][1]["label"] =
            duplicate_labels["questions"][0]["options"][0]["label"].clone();
        rejected(duplicate_labels);

        for reserved in ["Other", "Type something.", "Next"] {
            let mut reserved_label = valid(1);
            reserved_label["questions"][0]["options"][0]["label"] = reserved.into();
            rejected(reserved_label);
        }

        let mut nested_unknown = valid(1);
        nested_unknown["questions"][0]["unexpected"] = true.into();
        rejected(nested_unknown);
        let mut option_unknown = valid(1);
        option_unknown["questions"][0]["options"][0]["unexpected"] = true.into();
        rejected(option_unknown);

        let mut preview_on_multi = valid(1);
        preview_on_multi["questions"][0]["multi_select"] = true.into();
        preview_on_multi["questions"][0]["options"][0]["preview"] = "# preview".into();
        rejected(preview_on_multi);
    }

    #[test]
    fn selected_contract_constants_are_explicit_and_bounded() {
        assert_eq!(MAX_QUESTIONNAIRE_QUESTIONS, 4);
        assert_eq!(MIN_QUESTIONNAIRE_OPTIONS, 2);
        assert_eq!(MAX_QUESTIONNAIRE_OPTIONS, 4);
        assert_eq!(MAX_QUESTION_TEXT_CHARS, 4096);
        assert_eq!(MAX_QUESTION_HEADER_CHARS, 16);
        assert_eq!(MAX_OPTION_LABEL_CHARS, 60);
        assert_eq!(MAX_OPTION_DESCRIPTION_CHARS, 1024);
        assert_eq!(MAX_OPTION_PREVIEW_CHARS, 8192);
        assert_eq!(MAX_CUSTOM_ANSWER_CHARS, 4096);
    }
}
