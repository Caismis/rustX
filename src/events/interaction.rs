//! The provider-independent durable contract of the native interaction audit.
//!
//! Questionnaire facts live here because the same immutable values are used by
//! the model-facing tool, the live coordinator request, and the Event Journal
//! subject. Keeping the semantic validator at this boundary prevents the
//! registry, coordinator, and durable authority from drifting apart.

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize};

use crate::runtime::identity::{ToolCallId, ToolId};
use crate::runtime::types::CancellationReason;

/// Maximum number of questions in one foreground questionnaire.
pub const MAX_QUESTIONNAIRE_QUESTIONS: usize = 4;
/// Maximum number of authored options in one question.
pub const MAX_QUESTIONNAIRE_OPTIONS: usize = 4;
/// Minimum number of authored options in one question.
pub const MIN_QUESTIONNAIRE_OPTIONS: usize = 2;
/// Maximum Unicode scalar count of question text.
pub const MAX_QUESTION_TEXT_CHARS: usize = 4096;
/// Maximum Unicode scalar count of a question tab label.
pub const MAX_QUESTION_HEADER_CHARS: usize = 16;
/// Maximum Unicode scalar count of an authored option label.
pub const MAX_OPTION_LABEL_CHARS: usize = 60;
/// Maximum Unicode scalar count retained for an option description.
pub const MAX_OPTION_DESCRIPTION_CHARS: usize = 1024;
/// Maximum Unicode scalar count retained for an option Markdown preview.
pub const MAX_OPTION_PREVIEW_CHARS: usize = 8192;
/// Maximum Unicode scalar count retained for a custom answer.
pub const MAX_CUSTOM_ANSWER_CHARS: usize = 4096;

/// The longest policy explanation an Approval request may carry.
pub const MAX_APPROVAL_REQUEST_REASON_CHARS: usize = 1024;
/// The longest client-facing reason an Approval denial may carry.
pub const MAX_APPROVAL_DENIAL_REASON_CHARS: usize = 1024;
/// The longest model-facing tool name an Approval subject may name.
pub const MAX_APPROVAL_TOOL_NAME_CHARS: usize = 256;

const RESERVED_OPTION_LABELS: [&str; 3] = ["Other", "Type something.", "Next"];
const ARGUMENTS_DIGEST_DOMAIN: &[u8] = b"rustx-interaction-arguments-v1\n";
const DIGEST_HEX_CHARS: usize = 64;

/// One authored option in a questionnaire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionSpecification {
    /// The short option label shown in the selection list.
    pub label: String,
    /// The meaning and trade-offs of the option.
    pub description: String,
    /// Optional Markdown rendered in the preview pane.
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub preview: Option<String>,
}

fn deserialize_non_null_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

/// One question in a questionnaire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionSpecification {
    /// The full question shown above the choices.
    pub question: String,
    /// The short label used by the question tab.
    pub header: String,
    /// The finite authored options. The client adds its custom-answer row.
    pub options: Vec<OptionSpecification>,
    /// Whether several authored options may be selected.
    pub multi_select: bool,
}

/// The complete immutable questionnaire shown to a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionnaireSpecification {
    /// One to four related blocking questions.
    pub questions: Vec<QuestionSpecification>,
}

/// A single authored option selected for a single-select question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SingleOptionAnswer {
    /// The authored option label; the client never sends option text facts.
    pub label: String,
}

/// A custom answer entered by the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomAnswer {
    /// The bounded user-entered answer.
    pub answer: String,
}

/// Several authored options selected for a multi-select question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultipleOptionAnswer {
    /// Authored labels in canonical authored-option order.
    pub selected: Vec<String>,
}

/// One decision for one question. It carries only an index and a decision,
/// never a client-echoed copy of the request facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionnaireAnswerEntry {
    /// Zero-based index into the immutable questionnaire.
    pub question_index: usize,
    /// The typed decision for that question.
    pub answer: QuestionnaireAnswer,
}

/// A typed answer decision accepted from a Runtime Client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum QuestionnaireAnswer {
    /// One authored option.
    SingleOption(SingleOptionAnswer),
    /// One bounded custom answer.
    Custom(CustomAnswer),
    /// A non-empty set of authored options.
    MultipleOption(MultipleOptionAnswer),
}

/// The submitted decisions for one questionnaire. Omitted questions are
/// intentionally allowed so a user may submit a partial questionnaire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionnaireSubmission {
    /// At most one entry per answered question.
    pub answers: Vec<QuestionnaireAnswerEntry>,
}

/// The explicit user-decline response. It is distinct from attempt
/// cancellation and is settled as a successful tool result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionnaireDeclined;

/// The two valid terminal responses to a questionnaire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum QuestionnaireResponse {
    /// The accepted, possibly partial answer set.
    Submitted(QuestionnaireSubmission),
    /// The user explicitly declined, or submitted no answers.
    Declined,
}

/// The bounded terminal answer facts used by all interaction layers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionSubject {
    /// A tool invocation was held at the pre-tool policy boundary.
    Approval {
        /// The canonical model-issued call identity.
        call_id: ToolCallId,
        /// The registry-resolved tool identity.
        tool_id: ToolId,
        /// The model-facing tool name.
        tool_name: String,
        /// The digest of the canonical model-issued arguments.
        arguments_digest: String,
        /// The bounded policy explanation shown to the client.
        reason: String,
    },
    /// The complete questionnaire shown to the user, stored by value.
    Questionnaire {
        /// The exact immutable facts projected to the Runtime Client.
        questionnaire: QuestionnaireSpecification,
    },
}

/// The distinct durable terminal settlements of one interaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionSettlement {
    /// A client allowed the exact approval subject.
    Approved,
    /// A client denied the exact approval subject.
    Denied {
        /// The bounded client-facing denial reason.
        reason: String,
    },
    /// The canonical normalized questionnaire answer set.
    QuestionnaireSubmitted {
        /// The accepted answer decisions in question order.
        submission: QuestionnaireSubmission,
    },
    /// The user declined the questionnaire.
    QuestionnaireDeclined,
    /// The owning attempt cancellation authority won the rendezvous.
    Cancelled {
        /// The first-winner cancellation cause.
        reason: CancellationReason,
    },
}

/// The lowercase hex SHA-256 that an Approval subject pins its arguments by.
#[must_use]
pub fn interaction_arguments_digest(arguments: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    hasher.update(ARGUMENTS_DIGEST_DOMAIN);
    hasher.update(arguments.to_string().as_bytes());
    let mut digest = String::with_capacity(DIGEST_HEX_CHARS);
    for byte in hasher.finalize() {
        let _ = write!(digest, "{byte:02x}");
    }
    digest
}

fn is_canonical_digest(value: &str) -> bool {
    value.len() == DIGEST_HEX_CHARS
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_text(value: &str, name: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{name} must be non-empty"));
    }
    if value.chars().count() > max {
        return Err(format!("{name} exceeds {max} characters"));
    }
    Ok(())
}

fn is_reserved_option_label(label: &str) -> bool {
    RESERVED_OPTION_LABELS.contains(&label)
}

/// Validates the complete questionnaire contract shared by model preflight,
/// live publication, Runtime Client response checking, and durable storage.
///
/// # Errors
///
/// Returns an error when any questionnaire field is outside its bounded
/// contract, duplicated, reserved for the client, or otherwise inconsistent.
pub fn validate_questionnaire(questionnaire: &QuestionnaireSpecification) -> Result<(), String> {
    let count = questionnaire.questions.len();
    if count == 0 || count > MAX_QUESTIONNAIRE_QUESTIONS {
        return Err(format!(
            "questions must contain 1–{MAX_QUESTIONNAIRE_QUESTIONS} questions"
        ));
    }
    let mut question_texts = BTreeSet::new();
    for (question_index, question) in questionnaire.questions.iter().enumerate() {
        validate_text(
            &question.question,
            &format!("question {question_index} text"),
            MAX_QUESTION_TEXT_CHARS,
        )?;
        if !question_texts.insert(&question.question) {
            return Err("question text must be unique within one questionnaire".to_owned());
        }
        validate_text(
            &question.header,
            &format!("question {question_index} header"),
            MAX_QUESTION_HEADER_CHARS,
        )?;
        let option_count = question.options.len();
        if !(MIN_QUESTIONNAIRE_OPTIONS..=MAX_QUESTIONNAIRE_OPTIONS).contains(&option_count) {
            return Err(format!(
                "question {question_index} options must contain {MIN_QUESTIONNAIRE_OPTIONS}–{MAX_QUESTIONNAIRE_OPTIONS} options"
            ));
        }
        let mut labels = BTreeSet::new();
        for (option_index, option) in question.options.iter().enumerate() {
            validate_text(
                &option.label,
                &format!("question {question_index} option {option_index} label"),
                MAX_OPTION_LABEL_CHARS,
            )?;
            if is_reserved_option_label(&option.label) {
                return Err(format!(
                    "question {question_index} option label {:?} is reserved for the client",
                    option.label
                ));
            }
            if !labels.insert(&option.label) {
                return Err(format!(
                    "question {question_index} option labels must be unique"
                ));
            }
            validate_text(
                &option.description,
                &format!("question {question_index} option {option_index} description"),
                MAX_OPTION_DESCRIPTION_CHARS,
            )?;
            if let Some(preview) = &option.preview {
                validate_text(
                    preview,
                    &format!("question {question_index} option {option_index} preview"),
                    MAX_OPTION_PREVIEW_CHARS,
                )?;
                if question.multi_select {
                    return Err(format!(
                        "question {question_index} multi-select options cannot include previews"
                    ));
                }
            }
        }
    }
    Ok(())
}

fn option_position(question: &QuestionSpecification, label: &str) -> Option<usize> {
    question
        .options
        .iter()
        .position(|option| option.label == label)
}

fn validate_custom(answer: &CustomAnswer) -> Result<(), String> {
    validate_text(&answer.answer, "custom answer", MAX_CUSTOM_ANSWER_CHARS)
}

/// Validates and canonically orders a submitted answer set.
///
/// # Errors
///
/// Returns an error when an answer has an invalid index, kind, option label,
/// multiplicity, or custom-answer length.
pub fn normalize_questionnaire_submission(
    questionnaire: &QuestionnaireSpecification,
    submission: &QuestionnaireSubmission,
) -> Result<QuestionnaireSubmission, String> {
    validate_questionnaire(questionnaire)?;
    let mut entries = Vec::with_capacity(submission.answers.len());
    let mut seen_indices = BTreeSet::new();
    for entry in &submission.answers {
        let Some(question) = questionnaire.questions.get(entry.question_index) else {
            return Err(format!(
                "question index {} is out of range",
                entry.question_index
            ));
        };
        if !seen_indices.insert(entry.question_index) {
            return Err(format!(
                "question index {} appears more than once",
                entry.question_index
            ));
        }
        let answer = match &entry.answer {
            QuestionnaireAnswer::SingleOption(answer) if question.multi_select => {
                return Err(format!(
                    "question index {} requires multiple-option or custom answer",
                    entry.question_index
                ));
            }
            QuestionnaireAnswer::SingleOption(answer) => {
                validate_text(&answer.label, "single option label", MAX_OPTION_LABEL_CHARS)?;
                if option_position(question, &answer.label).is_none() {
                    return Err(format!(
                        "question index {} contains an unknown option label",
                        entry.question_index
                    ));
                }
                QuestionnaireAnswer::SingleOption(answer.clone())
            }
            QuestionnaireAnswer::Custom(answer) => {
                validate_custom(answer)?;
                QuestionnaireAnswer::Custom(answer.clone())
            }
            QuestionnaireAnswer::MultipleOption(answer) if !question.multi_select => {
                return Err(format!(
                    "question index {} is single-select and cannot accept multiple options",
                    entry.question_index
                ));
            }
            QuestionnaireAnswer::MultipleOption(answer) => {
                if answer.selected.is_empty() {
                    return Err(format!(
                        "question index {} must select at least one option",
                        entry.question_index
                    ));
                }
                let mut selected = BTreeSet::new();
                for label in &answer.selected {
                    validate_text(label, "multiple option label", MAX_OPTION_LABEL_CHARS)?;
                    if !selected.insert(label) {
                        return Err(format!(
                            "question index {} contains a duplicated option label",
                            entry.question_index
                        ));
                    }
                    if option_position(question, label).is_none() {
                        return Err(format!(
                            "question index {} contains an unknown option label",
                            entry.question_index
                        ));
                    }
                }
                let selected = question
                    .options
                    .iter()
                    .filter(|option| selected.contains(&option.label))
                    .map(|option| option.label.clone())
                    .collect();
                QuestionnaireAnswer::MultipleOption(MultipleOptionAnswer { selected })
            }
        };
        entries.push(QuestionnaireAnswerEntry {
            question_index: entry.question_index,
            answer,
        });
    }
    entries.sort_by_key(|entry| entry.question_index);
    Ok(QuestionnaireSubmission { answers: entries })
}

/// Validates a submitted response and turns an empty submission into the
/// explicit decline response used by the durable vocabulary.
///
/// # Errors
///
/// Returns an error when the questionnaire or any submitted answer violates
/// the shared bounded response contract.
pub fn normalize_questionnaire_response(
    questionnaire: &QuestionnaireSpecification,
    response: &QuestionnaireResponse,
) -> Result<QuestionnaireResponse, String> {
    match response {
        QuestionnaireResponse::Declined => {
            validate_questionnaire(questionnaire)?;
            Ok(QuestionnaireResponse::Declined)
        }
        QuestionnaireResponse::Submitted(submission) => {
            let submission = normalize_questionnaire_submission(questionnaire, submission)?;
            if submission.answers.is_empty() {
                Ok(QuestionnaireResponse::Declined)
            } else {
                Ok(QuestionnaireResponse::Submitted(submission))
            }
        }
    }
}

/// Validates the bounded payload of one durable requested subject.
///
/// # Errors
///
/// Returns an error when the subject contains an invalid identifier, digest,
/// reason, or questionnaire specification.
pub fn validate_interaction_subject(subject: &InteractionSubject) -> Result<(), String> {
    match subject {
        InteractionSubject::Approval {
            call_id,
            tool_id,
            tool_name,
            arguments_digest,
            reason,
        } => {
            if call_id.as_str().is_empty() {
                return Err("approval subject must name a non-empty tool call".to_owned());
            }
            if tool_id.as_str().is_empty() {
                return Err("approval subject must name a non-empty tool id".to_owned());
            }
            validate_text(
                tool_name,
                "approval tool name",
                MAX_APPROVAL_TOOL_NAME_CHARS,
            )?;
            if !is_canonical_digest(arguments_digest) {
                return Err(
                    "approval arguments digest must be a lowercase hex SHA-256 digest".to_owned(),
                );
            }
            if reason.chars().count() > MAX_APPROVAL_REQUEST_REASON_CHARS {
                return Err(format!(
                    "approval request reason exceeds {MAX_APPROVAL_REQUEST_REASON_CHARS} characters"
                ));
            }
            Ok(())
        }
        InteractionSubject::Questionnaire { questionnaire } => {
            validate_questionnaire(questionnaire)
        }
    }
}

/// Validates a durable terminal settlement against its exact requested facts.
///
/// # Errors
///
/// Returns an error when the settlement does not match its subject or carries
/// an invalid bounded response.
pub fn validate_interaction_settlement(
    subject: &InteractionSubject,
    settlement: &InteractionSettlement,
) -> Result<(), String> {
    match (subject, settlement) {
        (_, InteractionSettlement::Cancelled { .. })
        | (InteractionSubject::Approval { .. }, InteractionSettlement::Approved) => Ok(()),
        (InteractionSubject::Approval { .. }, InteractionSettlement::Denied { reason }) => {
            if reason.chars().count() > MAX_APPROVAL_DENIAL_REASON_CHARS {
                return Err(format!(
                    "approval denial reason exceeds {MAX_APPROVAL_DENIAL_REASON_CHARS} characters"
                ));
            }
            Ok(())
        }
        (
            InteractionSubject::Questionnaire { questionnaire },
            InteractionSettlement::QuestionnaireSubmitted { submission },
        ) => {
            let normalized = normalize_questionnaire_submission(questionnaire, submission)?;
            if normalized.answers.is_empty() {
                return Err(
                    "an empty questionnaire submission must be settled as questionnaire_declined"
                        .to_owned(),
                );
            }
            if &normalized != submission {
                return Err("questionnaire submission is not canonically ordered".to_owned());
            }
            Ok(())
        }
        (
            InteractionSubject::Questionnaire { questionnaire },
            InteractionSettlement::QuestionnaireDeclined,
        ) => validate_questionnaire(questionnaire),
        _ => Err(
            "the interaction settlement is a terminal its requested subject cannot produce"
                .to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn questionnaire() -> QuestionnaireSpecification {
        QuestionnaireSpecification {
            questions: vec![QuestionSpecification {
                question: "Which target?".to_owned(),
                header: "Target".to_owned(),
                options: vec![
                    OptionSpecification {
                        label: "Staging".to_owned(),
                        description: "A safe test environment.".to_owned(),
                        preview: None,
                    },
                    OptionSpecification {
                        label: "Production".to_owned(),
                        description: "The live environment.".to_owned(),
                        preview: None,
                    },
                ],
                multi_select: false,
            }],
        }
    }

    fn approval(reason: &str) -> InteractionSubject {
        InteractionSubject::Approval {
            call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool.read"),
            tool_name: "read".to_owned(),
            arguments_digest: interaction_arguments_digest(&serde_json::json!({"path": "a"})),
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn questionnaire_bounds_and_reserved_labels_are_enforced() {
        assert!(validate_questionnaire(&questionnaire()).is_ok());
        let mut null_preview = serde_json::to_value(questionnaire()).expect("questionnaire JSON");
        null_preview["questions"][0]["options"][0]["preview"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<QuestionnaireSpecification>(null_preview).is_err());

        let mut invalid = questionnaire();
        invalid.questions[0].options[0].label = "Other".to_owned();
        assert!(validate_questionnaire(&invalid).is_err());
        let mut invalid = questionnaire();
        invalid.questions[0].options[0].description.clear();
        assert!(validate_questionnaire(&invalid).is_err());
        let mut invalid = questionnaire();
        invalid.questions[0].options[0].preview = Some("preview".to_owned());
        invalid.questions[0].multi_select = true;
        assert!(validate_questionnaire(&invalid).is_err());

        let mut invalid = questionnaire();
        invalid.questions[0].question = "duplicate".to_owned();
        invalid.questions.push(invalid.questions[0].clone());
        assert!(validate_questionnaire(&invalid).is_err());

        for reserved in ["Other", "Type something.", "Next"] {
            let mut invalid = questionnaire();
            invalid.questions[0].options[0].label = reserved.to_owned();
            assert!(validate_questionnaire(&invalid).is_err());
        }

        let mut invalid = questionnaire();
        invalid.questions[0].header = "h".repeat(MAX_QUESTION_HEADER_CHARS + 1);
        assert!(validate_questionnaire(&invalid).is_err());
        let mut invalid = questionnaire();
        invalid.questions[0].options[0].description = "d".repeat(MAX_OPTION_DESCRIPTION_CHARS + 1);
        assert!(validate_questionnaire(&invalid).is_err());
        let mut invalid = questionnaire();
        invalid.questions[0].options[0].preview = Some("p".repeat(MAX_OPTION_PREVIEW_CHARS + 1));
        assert!(validate_questionnaire(&invalid).is_err());

        let mut invalid = questionnaire();
        invalid.questions[0].options[0].label = "é".repeat(MAX_OPTION_LABEL_CHARS + 1);
        assert!(validate_questionnaire(&invalid).is_err());

        let mut invalid = questionnaire();
        invalid.questions[0].options[1].label = invalid.questions[0].options[0].label.clone();
        assert!(validate_questionnaire(&invalid).is_err());
    }

    #[test]
    fn submitted_answers_are_ordered_by_question_and_authored_option() {
        let mut spec = questionnaire();
        spec.questions.push(QuestionSpecification {
            question: "Which extras?".to_owned(),
            header: "Extras".to_owned(),
            options: vec![
                OptionSpecification {
                    label: "Charts".to_owned(),
                    description: "Show charts.".to_owned(),
                    preview: None,
                },
                OptionSpecification {
                    label: "Comments".to_owned(),
                    description: "Show comments.".to_owned(),
                    preview: None,
                },
            ],
            multi_select: true,
        });
        let normalized = normalize_questionnaire_submission(
            &spec,
            &QuestionnaireSubmission {
                answers: vec![
                    QuestionnaireAnswerEntry {
                        question_index: 1,
                        answer: QuestionnaireAnswer::MultipleOption(MultipleOptionAnswer {
                            selected: vec!["Comments".to_owned(), "Charts".to_owned()],
                        }),
                    },
                    QuestionnaireAnswerEntry {
                        question_index: 0,
                        answer: QuestionnaireAnswer::SingleOption(SingleOptionAnswer {
                            label: "Staging".to_owned(),
                        }),
                    },
                ],
            },
        )
        .expect("valid response");
        assert_eq!(normalized.answers[0].question_index, 0);
        assert_eq!(
            normalized.answers[1].answer,
            QuestionnaireAnswer::MultipleOption(MultipleOptionAnswer {
                selected: vec!["Charts".to_owned(), "Comments".to_owned()],
            })
        );
    }

    #[test]
    fn empty_submission_becomes_decline_and_durable_empty_submit_is_rejected() {
        let response = normalize_questionnaire_response(
            &questionnaire(),
            &QuestionnaireResponse::Submitted(QuestionnaireSubmission { answers: vec![] }),
        )
        .expect("empty submission is the decline spelling");
        assert_eq!(response, QuestionnaireResponse::Declined);
        assert!(
            validate_interaction_settlement(
                &InteractionSubject::Questionnaire {
                    questionnaire: questionnaire(),
                },
                &InteractionSettlement::QuestionnaireSubmitted {
                    submission: QuestionnaireSubmission { answers: vec![] },
                },
            )
            .is_err()
        );
    }

    #[test]
    fn custom_answers_are_non_empty_and_bounded() {
        let response = QuestionnaireResponse::Submitted(QuestionnaireSubmission {
            answers: vec![QuestionnaireAnswerEntry {
                question_index: 0,
                answer: QuestionnaireAnswer::Custom(CustomAnswer {
                    answer: " ".to_owned(),
                }),
            }],
        });
        assert!(normalize_questionnaire_response(&questionnaire(), &response).is_err());

        let response = QuestionnaireResponse::Submitted(QuestionnaireSubmission {
            answers: vec![QuestionnaireAnswerEntry {
                question_index: 0,
                answer: QuestionnaireAnswer::Custom(CustomAnswer {
                    answer: "x".repeat(MAX_CUSTOM_ANSWER_CHARS + 1),
                }),
            }],
        });
        assert!(normalize_questionnaire_response(&questionnaire(), &response).is_err());
    }

    #[test]
    fn approval_contract_remains_bounded_and_typed() {
        assert!(validate_interaction_subject(&approval("policy asked")).is_ok());
        assert!(
            validate_interaction_subject(&approval(
                &"x".repeat(MAX_APPROVAL_REQUEST_REASON_CHARS + 1)
            ))
            .is_err()
        );
        assert!(
            validate_interaction_settlement(
                &approval("policy asked"),
                &InteractionSettlement::Approved,
            )
            .is_ok()
        );
        assert!(
            validate_interaction_settlement(
                &approval("policy asked"),
                &InteractionSettlement::QuestionnaireDeclined,
            )
            .is_err()
        );
    }
}
