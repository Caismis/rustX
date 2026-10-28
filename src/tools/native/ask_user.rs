//! The native `ask_user` tool.
//!
//! `ask_user` is an ordinary foreground, sequential, approval-never Tool. One
//! invocation contains one bounded questionnaire and publishes exactly one
//! interaction through the runtime-owned `InteractionCoordinator`.

use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::events::InteractionRequester;
use crate::events::{
    AnswerSpecification, BooleanAnswer, CustomAnswer, IntegerAnswer, MAX_QUESTIONNAIRE_OPTIONS,
    MIN_QUESTIONNAIRE_OPTIONS, MultiChoiceSpecification, NumberAnswer, OptionAnswer,
    OptionSpecification, OptionsAnswer, QuestionSpecification, QuestionnaireAnswer,
    QuestionnaireAnswerEntry, QuestionnaireResponse, QuestionnaireSpecification,
    QuestionnaireSubmission, SingleChoiceSpecification, TextAnswer,
    normalize_questionnaire_response, validate_questionnaire,
};
use crate::runtime::interaction::QuestionnaireFacts;
use crate::runtime::{InteractionOutcome, InteractionResponse};
use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, input_schema};
use crate::tools::native::support::{cancelled_result, failed_result, success_json};
use crate::tools::types::{
    ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
    ToolInvocation, ToolOrigin, ToolReplayPolicy,
};

/// The canonical model-facing name of the native questionnaire tool.
pub const ASK_USER_NAME: &str = crate::tools::executor::ASK_USER_TOOL_NAME;

/// Bounds the complete model-facing argument diagnostic, including its prefix.
const MAX_ARGUMENT_DIAGNOSTIC_BYTES: usize = 1024;

fn invalid_arguments(detail: impl std::fmt::Display) -> String {
    crate::tools::limits::bound_utf8_text(
        format!("invalid ask_user arguments: {detail}"),
        MAX_ARGUMENT_DIAGNOSTIC_BYTES,
    )
}

// Serde errors can embed authored string values or unknown keys. Retain only
// typed indexes and property names found in the tool's generated input schema.
fn malformed_arguments(path: &serde_path_to_error::Path) -> String {
    let schema = input_schema::<AskUserInput>();
    let mut field_schema = &schema;
    let mut field_path = String::new();
    for segment in path {
        match segment {
            serde_path_to_error::Segment::Map { key } => {
                let Some(property) = field_schema["properties"].get(key) else {
                    break;
                };
                if !field_path.is_empty() {
                    field_path.push('.');
                }
                field_path.push_str(key);
                field_schema = property;
            }
            serde_path_to_error::Segment::Seq { index } => {
                let Some(items) = field_schema.get("items") else {
                    break;
                };
                let _ = write!(field_path, "[{index}]");
                field_schema = items;
            }
            serde_path_to_error::Segment::Enum { .. } | serde_path_to_error::Segment::Unknown => {
                break;
            }
        }
    }
    if field_path.is_empty() {
        field_path.push('$');
    }
    invalid_arguments(format!(
        "{field_path} does not match the declared input shape. Use the declared JSON types, include required fields, omit unknown fields, and omit optional fields instead of null."
    ))
}

/// The model-facing input shape. `multi_select` is optional in the model
/// contract and defaults to false through Serde and the generated schema.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AskUserInput {
    /// One to four related blocking questions with unique question text.
    #[schemars(length(min = 1, max = 4), inner(length(min = 1, max = 4096)))]
    questions: Vec<AskUserQuestionInput>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AskUserQuestionInput {
    /// Meaningful, non-whitespace question text, unique within this questionnaire.
    #[schemars(length(min = 1, max = 4096))]
    question: String,
    /// A meaningful, non-whitespace tab label of at most 16 characters.
    #[schemars(length(min = 1, max = 16))]
    header: String,
    /// Two to four authored options with distinct labels and meaningful descriptions. The runtime supplies the custom-answer choice; do not author Other or an equivalent reserved choice.
    #[schemars(length(min = 2, max = 4), inner(length(min = 1, max = 60)))]
    options: Vec<AskUserOptionInput>,
    /// Defaults to false (single-select). Multi-select questions cannot include option previews.
    #[serde(default)]
    multi_select: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AskUserOptionInput {
    /// A meaningful, non-whitespace label, unique within this question. Do not use Other, Type something., or Next: the runtime reserves these for custom-answer controls.
    #[schemars(length(min = 1, max = 60))]
    label: String,
    /// A meaningful, non-whitespace description of the option's meaning and trade-offs.
    #[schemars(length(min = 1, max = 1024))]
    description: String,
    /// Optional non-whitespace Markdown preview for single-select questions only. Omit when unused; null is not accepted.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(length(min = 1, max = 8192))]
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
        let input: Self = serde_path_to_error::deserialize(arguments)
            .map_err(|error| malformed_arguments(error.path()))?;
        input
            .validate_authoring_bounds()
            .and_then(|()| validate_questionnaire(&input.specification()))
            .map_err(invalid_arguments)?;
        Ok(input)
    }

    /// The **model-facing** authoring bounds of `ask_user`, which are narrower
    /// than the shared typed-choice contract on purpose: the tool asks its
    /// author for a genuine decision between 2–4 named alternatives, while the
    /// shared vocabulary must also carry, for example, a single-member MCP
    /// `enum`.
    fn validate_authoring_bounds(&self) -> Result<(), String> {
        for (index, question) in self.questions.iter().enumerate() {
            let count = question.options.len();
            if !(MIN_QUESTIONNAIRE_OPTIONS..=MAX_QUESTIONNAIRE_OPTIONS).contains(&count) {
                return Err(format!(
                    "questions[{index}].options must contain \
                     {MIN_QUESTIONNAIRE_OPTIONS}–{MAX_QUESTIONNAIRE_OPTIONS} authored options. Adjust the option count."
                ));
            }
        }
        Ok(())
    }

    fn normalize(arguments: &serde_json::Value) -> Result<serde_json::Value, String> {
        let input = Self::from_wire(arguments)?;
        serde_json::to_value(input).map_err(|_| {
            invalid_arguments("$ could not be normalized. Follow the declared input schema.")
        })
    }

    /// Maps the model-authored questionnaire onto the shared typed vocabulary.
    ///
    /// Native `ask_user` is exactly "authored choices **plus** a custom answer",
    /// which is now one point in that vocabulary rather than the whole of it:
    ///
    /// ```text
    /// multi_select: false -> SingleChoice { options, allow_custom: true }
    /// multi_select: true  -> MultiChoice  { options, 1..=options.len(),
    ///                                       allow_custom: true }
    /// ```
    ///
    /// Every existing native behaviour — authored options, descriptions,
    /// previews, multi-select, custom text answers, explicit decline — is
    /// preserved by that mapping; nothing about it is MCP-specific.
    fn specification(&self) -> QuestionnaireSpecification {
        QuestionnaireSpecification {
            questions: self
                .questions
                .iter()
                .map(|question| {
                    let options: Vec<OptionSpecification> = question
                        .options
                        .iter()
                        .map(|option| OptionSpecification {
                            label: option.label.clone(),
                            description: option.description.clone(),
                            preview: option.preview.clone(),
                        })
                        .collect();
                    let answer = if question.multi_select {
                        let max_selected = u32::try_from(options.len()).unwrap_or(u32::MAX);
                        AnswerSpecification::MultiChoice(MultiChoiceSpecification {
                            options,
                            // A submitted multi-select answer selects at least
                            // one option; leaving the question unanswered is
                            // still expressed by omitting it entirely.
                            min_selected: 1,
                            max_selected,
                            allow_custom: true,
                        })
                    } else {
                        AnswerSpecification::SingleChoice(SingleChoiceSpecification {
                            options,
                            allow_custom: true,
                        })
                    };
                    QuestionSpecification {
                        question: question.question.clone(),
                        header: question.header.clone(),
                        answer,
                    }
                })
                .collect(),
        }
    }
}

/// The canonical requester identity of the native questionnaire tool.
///
/// `ask_user` is a built-in tool, so its requester origin is
/// [`ToolOrigin::Builtin`] and never an MCP server. That is what keeps a
/// native prompt from being labelled as MCP by any Runtime Client.
fn ask_user_requester() -> InteractionRequester {
    InteractionRequester {
        tool_id: crate::runtime::identity::ToolId::new("tool-ask-user"),
        tool_name: ASK_USER_NAME.to_owned(),
        origin: ToolOrigin::Builtin,
    }
}

/// The tool-owned registration of `ask_user`.
#[must_use]
pub(super) fn registration() -> NativeToolRegistration {
    NativeToolRegistration::new(definition(), std::sync::Arc::new(AskUserExecutor))
        .with_normalizer(AskUserInput::normalize)
}

pub(super) fn definition() -> ToolDefinition {
    ToolDefinition {
        id: crate::runtime::identity::ToolId::new("tool-ask-user"),
        name: ASK_USER_NAME.to_owned(),
        description: "Ask the user one structured questionnaire for a blocking decision. Group related blocking questions into one invocation instead of issuing consecutive ask_user calls. Use 1–4 questions with unique question text, each with 2–4 authored options. Option labels must be unique within their question; every option needs a meaningful label and description of its meaning or trade-offs. A custom answer is always available at runtime, so do not author Other or an equivalent reserved custom-answer choice. If one option is recommended, place it first and append (Recommended) to its label. multi_select defaults to false. Optional previews are for single-select questions only; multi-select questions cannot include previews. The user's answers are returned as an ordinary structured tool result.".to_owned(),
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
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                let Ok(input) = serde_json::from_value::<AskUserInput>(invocation.arguments) else {
                    return failed_result(
                        "ask_user received an invocation that was not preflight-normalized",
                    );
                };
                let specification = input.specification();
                if let Err(error) = validate_questionnaire(&specification) {
                    return failed_result(invalid_arguments(error));
                }
                let Some(requester) = context.questionnaire_requester() else {
                    return failed_result("ask_user interaction provider unavailable");
                };
                let outcome = requester
                    .request_questionnaire(QuestionnaireFacts {
                        invocation_id: invocation.id,
                        requester: ask_user_requester(),
                        questionnaire: specification.clone(),
                    })
                    .await;
                match outcome {
                    Ok(InteractionOutcome::DeadlineExpired { .. }) => {
                        crate::tools::invocation::terminal(
                            crate::tools::types::ToolExecutionStatus::TimedOut,
                        )
                    }
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
            }),
            cancellation,
        )
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
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
                    // The model-facing result stays label-shaped, because the
                    // labels are what the model authored. The *canonical*
                    // identity on the wire is the option index; the label is
                    // resolved here, from the same immutable request facts.
                    let options = question.answer.options();
                    match entry.answer {
                        QuestionnaireAnswer::Option(OptionAnswer { option_index }) => {
                            result["kind"] = serde_json::json!("option");
                            result["answer"] =
                                serde_json::json!(options[option_index].label.clone());
                        }
                        QuestionnaireAnswer::Custom(CustomAnswer { answer }) => {
                            result["kind"] = serde_json::json!("custom");
                            result["answer"] = serde_json::json!(answer);
                        }
                        QuestionnaireAnswer::Options(OptionsAnswer { option_indices }) => {
                            result["kind"] = serde_json::json!("multiple");
                            result["selected"] = serde_json::json!(
                                option_indices
                                    .into_iter()
                                    .map(|index| options[index].label.clone())
                                    .collect::<Vec<_>>()
                            );
                        }
                        // The remaining typed shapes are never produced by the
                        // native tool's own SingleChoice/MultiChoice questions,
                        // but the mapping stays total rather than panicking.
                        QuestionnaireAnswer::Text(TextAnswer { value }) => {
                            result["kind"] = serde_json::json!("text");
                            result["answer"] = serde_json::json!(value);
                        }
                        QuestionnaireAnswer::Number(NumberAnswer { value }) => {
                            result["kind"] = serde_json::json!("number");
                            // `FiniteNumber` cannot be NaN or infinite, so the
                            // JSON number always exists; the fallback exists
                            // only to keep this mapping total without a panic.
                            result["answer"] = value
                                .to_json_number()
                                .map_or(serde_json::Value::Null, serde_json::Value::Number);
                        }
                        QuestionnaireAnswer::Integer(IntegerAnswer { value }) => {
                            result["kind"] = serde_json::json!("integer");
                            // The model-facing result carries the exact whole
                            // number as a JSON integer: this is a Rust->model
                            // hop, not the Runtime Client wire, so no binary64
                            // stage exists to round it.
                            result["answer"] = serde_json::json!(value.get());
                        }
                        QuestionnaireAnswer::Boolean(BooleanAnswer { value }) => {
                            result["kind"] = serde_json::json!("boolean");
                            result["answer"] = serde_json::json!(value);
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

    fn schema_keyword_paths(schema: &serde_json::Value, keyword: &str) -> Vec<String> {
        fn visit(value: &serde_json::Value, keyword: &str, path: &str, paths: &mut Vec<String>) {
            match value {
                serde_json::Value::Object(object) => {
                    for (key, value) in object {
                        let child = format!("{path}/{key}");
                        if key == keyword {
                            paths.push(child.clone());
                        }
                        visit(value, keyword, &child, paths);
                    }
                }
                serde_json::Value::Array(values) => {
                    for (index, value) in values.iter().enumerate() {
                        visit(value, keyword, &format!("{path}/{index}"), paths);
                    }
                }
                _ => {}
            }
        }
        let mut paths = Vec::new();
        visit(schema, keyword, "", &mut paths);
        paths.sort();
        paths
    }

    fn strip_schema_guidance_and_patterns(schema: &mut serde_json::Value) {
        match schema {
            serde_json::Value::Object(object) => {
                object.remove("pattern");
                // Preserve the actual description property, whose value is a
                // schema object; only string-valued descriptions are guidance.
                if object
                    .get("description")
                    .is_some_and(serde_json::Value::is_string)
                {
                    object.remove("description");
                }
                for value in object.values_mut() {
                    strip_schema_guidance_and_patterns(value);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    strip_schema_guidance_and_patterns(value);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn compiled_schema_delta_only_removes_five_patterns_and_updates_guidance() {
        // Captured by compiling definition() at the starting origin/main SHA,
        // 3c4b85a627e87383f244600e8b19887f103b82f0, before changing its attributes.
        let mut before: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/ask_user/schema-before.json"
        ))
        .unwrap();
        let mut original_definition = definition();
        original_definition.input_schema = before.clone();
        let compiled_before = crate::tools::schema::compile_model_definition(&original_definition)
            .expect("baseline definition compiles for model adapters");
        assert_eq!(compiled_before.input_schema, before);
        let mut after = registry().model_definitions().remove(0).input_schema;
        let captured_after: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/ask_user/schema-after.json"
        ))
        .unwrap();
        assert_eq!(after, captured_after);
        assert_eq!(after, definition().input_schema);
        let text_paths = [
            ("/properties/questions/items/properties/question", 4096),
            ("/properties/questions/items/properties/header", 16),
            (
                "/properties/questions/items/properties/options/items/properties/label",
                60,
            ),
            (
                "/properties/questions/items/properties/options/items/properties/description",
                1024,
            ),
            (
                "/properties/questions/items/properties/options/items/properties/preview",
                8192,
            ),
        ];
        let mut expected_patterns: Vec<_> = text_paths
            .iter()
            .map(|(path, _)| format!("{path}/pattern"))
            .collect();
        expected_patterns.sort();
        assert_eq!(schema_keyword_paths(&before, "pattern"), expected_patterns);
        assert!(schema_keyword_paths(&after, "pattern").is_empty());
        assert!(schema_keyword_paths(&after, "uniqueItems").is_empty());
        for (path, max) in text_paths {
            assert_eq!(before.pointer(path).unwrap()["pattern"], r"\S");
            let field = after.pointer(path).unwrap();
            assert_eq!(field["type"], "string");
            assert_eq!(field["minLength"], 1);
            assert_eq!(field["maxLength"], max);
        }
        jsonschema::validator_for(&after).expect("compiled native schema remains valid");
        strip_schema_guidance_and_patterns(&mut before);
        strip_schema_guidance_and_patterns(&mut after);
        assert_eq!(after, before, "all structural schema metadata is preserved");
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
        let mut input = AskUserInput::from_wire(&valid(2)).expect("fixture questionnaire");
        input.questions[1].multi_select = true;
        let specification = input.specification();
        let result = questionnaire_result(
            &specification,
            &QuestionnaireResponse::Submitted(QuestionnaireSubmission {
                answers: vec![
                    QuestionnaireAnswerEntry {
                        question_index: 1,
                        answer: QuestionnaireAnswer::Options(OptionsAnswer {
                            option_indices: vec![0, 1],
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
    fn whitespace_only_authored_fields_remain_invalid_without_schema_patterns() {
        let registry = registry();
        for path in [
            "/questions/0/question",
            "/questions/0/header",
            "/questions/0/options/0/label",
            "/questions/0/options/0/description",
            "/questions/0/options/0/preview",
        ] {
            let mut arguments = valid(1);
            arguments["questions"][0]["options"][0]["preview"] = "A valid preview".into();
            *arguments.pointer_mut(path).unwrap() = " \t\n\u{3000}".into();
            let error = AskUserInput::from_wire(&arguments).unwrap_err();
            assert!(error.contains("non-whitespace text"), "{path}: {error}");
            assert!(error.contains("Supply meaningful text."));
            assert!(!preflight(&registry, arguments));
        }
    }

    #[test]
    fn malformed_argument_diagnostics_keep_safe_paths_without_authored_values() {
        let registry = registry();
        let private_value = "秘密字段-private-model-value".repeat(1024);
        for (path, expected) in [
            ("/questions", "questions"),
            ("/questions/0", "questions[0]"),
            ("/questions/0/question", "questions[0].question"),
            ("/questions/0/header", "questions[0].header"),
            ("/questions/0/options", "questions[0].options"),
            ("/questions/0/options/0", "questions[0].options[0]"),
            (
                "/questions/0/options/0/label",
                "questions[0].options[0].label",
            ),
            (
                "/questions/0/options/0/description",
                "questions[0].options[0].description",
            ),
            (
                "/questions/0/options/0/preview",
                "questions[0].options[0].preview",
            ),
            ("/questions/0/multi_select", "questions[0].multi_select"),
        ] {
            let mut arguments = valid(1);
            arguments["questions"][0]["multi_select"] = false.into();
            arguments["questions"][0]["options"][0]["preview"] = "Preview".into();
            let original = arguments.pointer_mut(path).unwrap();
            *original = if original.is_string() {
                serde_json::json!({"private-key": private_value})
            } else {
                private_value.clone().into()
            };
            let error = AskUserInput::from_wire(&arguments).unwrap_err();
            assert!(error.contains(expected), "{path}: {error}");
            assert!(error.contains("Use the declared JSON types"));
            assert!(!error.contains("private"));
            assert!(!error.contains("秘密字段"));
            assert!(error.len() <= MAX_ARGUMENT_DIAGNOSTIC_BYTES);
            assert!(std::str::from_utf8(error.as_bytes()).is_ok());
            assert_eq!(error, AskUserInput::from_wire(&arguments).unwrap_err());
            assert!(!preflight(&registry, arguments));
        }

        for (container, expected) in [
            ("", "$"),
            ("/questions/0", "questions[0]"),
            ("/questions/0/options/0", "questions[0].options[0]"),
        ] {
            let mut arguments = valid(1);
            arguments
                .pointer_mut(container)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(private_value.clone(), private_value.clone().into());
            let error = AskUserInput::from_wire(&arguments).unwrap_err();
            assert!(error.contains(expected), "{container}: {error}");
            assert!(error.contains("omit unknown fields"));
            assert!(!error.contains("private"));
            assert!(!error.contains("秘密字段"));
            assert!(error.len() <= MAX_ARGUMENT_DIAGNOSTIC_BYTES);
            assert_eq!(error, AskUserInput::from_wire(&arguments).unwrap_err());
            assert!(!preflight(&registry, arguments));
        }
    }

    #[test]
    fn reserved_runtime_metadata_rejection_is_bounded_without_echoing_the_private_key() {
        let mut arguments = valid(1);
        let private_key = format!("__rustx_{}", "private-key-秘密字段".repeat(1024));
        arguments
            .as_object_mut()
            .unwrap()
            .insert(private_key.clone(), true.into());
        let outcome = registry()
            .preflight(&ToolCall {
                id: crate::runtime::identity::ToolCallId::new("reserved-metadata"),
                tool_id: ToolId::new("tool-ask-user"),
                name: ASK_USER_NAME.to_owned(),
                arguments,
            })
            .unwrap();
        let PreflightOutcome::Rejected { error, .. } = outcome else {
            panic!("reserved metadata must be rejected before native execution");
        };
        assert!(error.contains('$'));
        assert!(error.contains("reserved"));
        assert!(error.contains("__rustx_"));
        assert!(error.contains("Remove"));
        assert!(!error.contains(&private_key));
        assert!(!error.contains("private-key"));
        assert!(!error.contains("秘密字段"));
        assert!(error.len() <= MAX_ARGUMENT_DIAGNOSTIC_BYTES);
        assert!(std::str::from_utf8(error.as_bytes()).is_ok());
    }

    #[test]
    fn missing_fields_and_null_preview_have_safe_actionable_diagnostics() {
        for container in ["", "/questions/0", "/questions/0/options/0"] {
            let mut arguments = valid(1);
            *arguments.pointer_mut(container).unwrap() = serde_json::json!({});
            let error = AskUserInput::from_wire(&arguments).unwrap_err();
            assert!(error.contains("include required fields"));
            assert!(error.len() <= MAX_ARGUMENT_DIAGNOSTIC_BYTES);
        }
        let mut arguments = valid(1);
        arguments["questions"][0]["options"][0]["preview"] = serde_json::Value::Null;
        let error = AskUserInput::from_wire(&arguments).unwrap_err();
        assert!(error.contains("questions[0].options[0].preview"));
        assert!(error.contains("omit optional fields instead of null"));
    }

    #[test]
    fn reserved_label_diagnostics_name_the_field_and_correction_without_the_label() {
        for reserved in ["Other", "Type something.", "Next"] {
            let mut arguments = valid(1);
            arguments["questions"][0]["options"][0]["label"] = reserved.into();
            let error = AskUserInput::from_wire(&arguments).unwrap_err();
            assert!(error.contains("questions[0].options[0].label"));
            assert!(error.contains("reserved"));
            assert!(error.contains("Replace it with a distinct authored choice"));
            assert!(!error.contains(reserved));
            assert!(error.len() <= MAX_ARGUMENT_DIAGNOSTIC_BYTES);
        }
    }

    #[test]
    fn argument_diagnostic_bound_counts_bytes_without_splitting_utf8() {
        for text in ["a".repeat(2048), "修正".repeat(1024)] {
            let error = invalid_arguments(&text);
            assert!(error.starts_with("invalid ask_user arguments: "));
            assert!(error.len() <= MAX_ARGUMENT_DIAGNOSTIC_BYTES);
            assert!(error.len() >= MAX_ARGUMENT_DIAGNOSTIC_BYTES - 3);
            assert!(std::str::from_utf8(error.as_bytes()).is_ok());
            assert_eq!(error, invalid_arguments(&text));
        }
    }

    /// Native `ask_user` maps onto the shared typed vocabulary as
    /// "authored choices **plus** a custom answer", and its requester is a
    /// built-in tool that no Runtime Client can mistake for MCP.
    #[test]
    fn native_ask_user_is_a_builtin_choice_question_with_a_custom_answer() {
        let requester = ask_user_requester();
        assert_eq!(requester.origin, ToolOrigin::Builtin);
        assert_eq!(requester.tool_name, ASK_USER_NAME);
        assert!(requester.mcp_server().is_none(), "ask_user is never MCP");
        assert!(requester.validate().is_ok());

        let mut input = AskUserInput::from_wire(&valid(2)).expect("fixture questionnaire");
        input.questions[1].multi_select = true;
        let specification = input.specification();
        let AnswerSpecification::SingleChoice(single) = &specification.questions[0].answer else {
            panic!("a single-select ask_user question is a SingleChoice");
        };
        assert!(
            single.allow_custom,
            "native ask_user keeps its custom answer"
        );
        assert_eq!(single.options.len(), 2);
        let AnswerSpecification::MultiChoice(multi) = &specification.questions[1].answer else {
            panic!("a multi-select ask_user question is a MultiChoice");
        };
        assert!(multi.allow_custom);
        assert_eq!((multi.min_selected, multi.max_selected), (1, 2));

        // Previews, descriptions, and the decline response are unchanged.
        let mut with_preview = valid(1);
        with_preview["questions"][0]["options"][0]["preview"] = "# preview".into();
        let previewed = AskUserInput::from_wire(&with_preview)
            .expect("previews remain a native option facet")
            .specification();
        assert_eq!(
            previewed.questions[0].answer.options()[0]
                .preview
                .as_deref(),
            Some("# preview")
        );
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
