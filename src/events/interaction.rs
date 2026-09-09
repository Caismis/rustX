//! The provider-independent durable contract of the native interaction audit.
//!
//! Questionnaire facts live here because the same immutable values are used by
//! the model-facing tool, the live coordinator request, and the Event Journal
//! subject. Keeping the semantic validator at this boundary prevents the
//! registry, coordinator, and durable authority from drifting apart.
//!
//! # The typed question vocabulary (Issue #242)
//!
//! A questionnaire is **not** "authored choices plus an always-present custom
//! row". It is a small, finite, provider-independent vocabulary in which every
//! question declares the exact shape of a legal answer:
//!
//! ```text
//! QuestionSpecification { question, header, answer }
//!                                            |
//!        +----------+----------+-------------+-----------+--------------+
//!        |          |          |             |           |              |
//!      Text      Number     Integer       Boolean   SingleChoice   MultiChoice
//!    bounded    bounded     bounded        true/     options +      options +
//!    length,    range       range          false     allow_custom   bounds +
//!    format                                                        allow_custom
//! ```
//!
//! Two properties follow, and both are load-bearing:
//!
//! - **the request declares the legal answer shape**, so a Runtime Client
//!   never has to guess which answers are legal, and a custom free-text answer
//!   exists only where a question explicitly allows one;
//! - **response validation derives from those immutable request facts**, in
//!   this module, which is the one authority the coordinator, the durable
//!   store, and every producer share. Client-side validation is UX;
//!   [`normalize_questionnaire_submission`] is the authority.
//!
//! Answers address a choice by its **zero-based option index**, never by its
//! display label. A display string is presentation; it can repeat, collide
//! with a client-reserved row, or be forged, and none of that can change which
//! value the runtime settles on.

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize};

#[cfg(test)]
use crate::runtime::identity::ToolCallId;
use crate::runtime::identity::ToolId;
use crate::runtime::types::CancellationReason;
use crate::tools::types::ToolOrigin;

/// Maximum number of questions in one foreground questionnaire.
pub const MAX_QUESTIONNAIRE_QUESTIONS: usize = 4;
/// Maximum number of authored options in one model-facing `ask_user` question.
///
/// This is the **native tool's** authoring bound, not the shared typed-choice
/// bound: see [`MAX_CHOICE_OPTIONS`].
pub const MAX_QUESTIONNAIRE_OPTIONS: usize = 4;
/// Minimum number of authored options in one model-facing `ask_user` question.
pub const MIN_QUESTIONNAIRE_OPTIONS: usize = 2;
/// Minimum number of options any typed choice question may declare.
///
/// One option is representable — a single-member MCP `enum` is a legitimate
/// schema — even though the native `ask_user` tool asks its author for two.
pub const MIN_CHOICE_OPTIONS: usize = 1;
/// Maximum number of options any typed choice question may declare.
pub const MAX_CHOICE_OPTIONS: usize = 12;
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
/// Maximum Unicode scalar count retained for a free-form text answer.
pub const MAX_TEXT_ANSWER_CHARS: usize = 4096;
/// Maximum Unicode scalar count of a requester's model-facing tool name.
pub const MAX_REQUESTER_TOOL_NAME_CHARS: usize = 256;

/// The longest policy explanation an Approval request may carry.
pub const MAX_APPROVAL_REQUEST_REASON_CHARS: usize = 1024;
/// The longest client-facing reason an Approval denial may carry.
pub const MAX_APPROVAL_DENIAL_REASON_CHARS: usize = 1024;
/// The longest model-facing tool name an Approval subject may name.
pub const MAX_APPROVAL_TOOL_NAME_CHARS: usize = 256;

const RESERVED_OPTION_LABELS: [&str; 3] = ["Other", "Type something.", "Next"];
const ARGUMENTS_DIGEST_DOMAIN: &[u8] = b"rustx-interaction-arguments-v1\n";
const DIGEST_HEX_CHARS: usize = 64;

/// The canonical, provider-independent identity of whoever asked the human.
///
/// These are the registry-resolved facts of the tool invocation that reached
/// the interaction boundary — never a display string, never an rmcp value, and
/// never something a Runtime Client infers from the prompt text. The same
/// immutable value flows through the live request, the Event Journal subject,
/// the Runtime Client projection, and the TUI, so an MCP-originated prompt can
/// always name the server that asked:
///
/// ```text
/// ToolOrigin::Mcp { server_id: "github" } + tool_name "create_issue"
///     -> "Requested by MCP server: github / Tool: create_issue"
/// ToolOrigin::Builtin + tool_name "ask_user"
///     -> the native wording, never labelled as MCP
/// ```
///
/// It is deliberately orthogonal to
/// [`InteractionSource`](crate::runtime::interaction::InteractionSource):
/// *where* an interaction came from (primary or subagent) and *who* requested
/// it (native tool or MCP server) are two independent facts and are never
/// collapsed into one field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionRequester {
    /// The canonical registry-resolved tool identity.
    pub tool_id: ToolId,
    /// The safe model-facing tool name.
    pub tool_name: String,
    /// The registry-resolved tool origin, which carries MCP server identity.
    pub origin: ToolOrigin,
}

impl InteractionRequester {
    /// Validates the bounded requester facts.
    ///
    /// # Errors
    ///
    /// Returns an error when the tool identity or name is empty or above its
    /// bound.
    pub fn validate(&self) -> Result<(), String> {
        if self.tool_id.as_str().is_empty() {
            return Err("interaction requester must name a non-empty tool id".to_owned());
        }
        validate_text(
            &self.tool_name,
            "interaction requester tool name",
            MAX_REQUESTER_TOOL_NAME_CHARS,
        )
    }

    /// The MCP server identity, when the requester is an MCP-served tool.
    #[must_use]
    pub fn mcp_server(&self) -> Option<&crate::runtime::identity::McpServerId> {
        match &self.origin {
            ToolOrigin::Mcp { server_id } => Some(server_id),
            ToolOrigin::Builtin => None,
        }
    }
}

/// One authored option in a choice question.
///
/// The label is **presentation**. A response addresses this option by its
/// zero-based position in [`SingleChoiceSpecification::options`] or
/// [`MultiChoiceSpecification::options`], so a duplicated, reserved, or forged
/// display string can never select a different underlying value.
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

/// The bounded set of text shapes rustX can validate deterministically.
///
/// Only formats rustX can prove are supported. A schema asking for a format
/// outside this set is refused by its producer rather than accepted and then
/// silently unvalidated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextFormat {
    /// An exact `YYYY-MM-DD` calendar date.
    Date,
    /// An RFC 3339 timestamp.
    DateTime,
    /// An absolute URI.
    Uri,
}

impl TextFormat {
    fn validate(self, value: &str) -> Result<(), String> {
        let ok = match self {
            Self::Date => chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok(),
            Self::DateTime => chrono::DateTime::parse_from_rfc3339(value).is_ok(),
            Self::Uri => url::Url::parse(value).is_ok(),
        };
        if ok {
            Ok(())
        } else {
            Err(format!(
                "the answer is not a valid {} value",
                match self {
                    Self::Date => "date (YYYY-MM-DD)",
                    Self::DateTime => "RFC 3339 date-time",
                    Self::Uri => "absolute URI",
                }
            ))
        }
    }
}

/// A bounded free-form text question.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextAnswerSpecification {
    /// The inclusive minimum Unicode scalar count, when the producer declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u32>,
    /// The inclusive maximum Unicode scalar count, when the producer declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
    /// The declared text shape, when the producer declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<TextFormat>,
}

/// A finite numeric question.
///
/// The bounds are stored as [`serde_json::Number`], which cannot represent NaN
/// or infinity, so a non-finite bound is unrepresentable rather than merely
/// rejected.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberAnswerSpecification {
    /// The inclusive minimum, when the producer declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<serde_json::Number>,
    /// The inclusive maximum, when the producer declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<serde_json::Number>,
}

/// A whole-number question, semantically distinct from [`NumberAnswerSpecification`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerAnswerSpecification {
    /// The inclusive minimum, when the producer declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<i64>,
    /// The inclusive maximum, when the producer declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<i64>,
}

/// A pick-exactly-one question over declared options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SingleChoiceSpecification {
    /// The finite declared options, addressed by index.
    pub options: Vec<OptionSpecification>,
    /// Whether a free-text answer outside the declared options is legal.
    pub allow_custom: bool,
}

/// A pick-between-`min_selected`-and-`max_selected` question over declared options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiChoiceSpecification {
    /// The finite declared options, addressed by index.
    pub options: Vec<OptionSpecification>,
    /// The inclusive lower bound on how many options a submitted answer selects.
    pub min_selected: u32,
    /// The inclusive upper bound on how many options a submitted answer selects.
    pub max_selected: u32,
    /// Whether a free-text answer outside the declared options is legal.
    pub allow_custom: bool,
}

/// The exact shape of a legal answer to one question.
///
/// This is the whole provider-independent vocabulary. It is deliberately a
/// closed set of concrete cases rather than a JSON Schema interpreter: every
/// producer must map into one of these or refuse, and every consumer can
/// render and validate all of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnswerSpecification {
    /// Bounded free-form text.
    Text(TextAnswerSpecification),
    /// A finite number, integral or fractional.
    Number(NumberAnswerSpecification),
    /// A whole number. `1.5` is never a legal answer.
    Integer(IntegerAnswerSpecification),
    /// A typed boolean. `Yes`/`No` is presentation; `true`/`false` is the value.
    Boolean,
    /// Exactly one declared option, or a custom answer when allowed.
    SingleChoice(SingleChoiceSpecification),
    /// A bounded subset of declared options, or a custom answer when allowed.
    MultiChoice(MultiChoiceSpecification),
}

impl AnswerSpecification {
    /// The declared options of a choice question.
    #[must_use]
    pub fn options(&self) -> &[OptionSpecification] {
        match self {
            Self::SingleChoice(single) => &single.options,
            Self::MultiChoice(multi) => &multi.options,
            Self::Text(_) | Self::Number(_) | Self::Integer(_) | Self::Boolean => &[],
        }
    }

    /// Whether this question accepts a free-text answer outside its options.
    #[must_use]
    pub fn allows_custom(&self) -> bool {
        match self {
            Self::SingleChoice(single) => single.allow_custom,
            Self::MultiChoice(multi) => multi.allow_custom,
            Self::Text(_) | Self::Number(_) | Self::Integer(_) | Self::Boolean => false,
        }
    }
}

/// One question in a questionnaire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionSpecification {
    /// The full question shown above the answer surface.
    pub question: String,
    /// The short label used by the question tab.
    pub header: String,
    /// The exact shape of a legal answer.
    pub answer: AnswerSpecification,
}

/// The complete immutable questionnaire shown to a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionnaireSpecification {
    /// One to four related blocking questions.
    pub questions: Vec<QuestionSpecification>,
}

/// A bounded free-form text answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextAnswer {
    /// The bounded user-entered text.
    pub value: String,
}

/// A finite numeric answer, carried as a JSON number rather than a string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberAnswer {
    /// The typed value. `serde_json::Number` cannot be NaN or infinite.
    pub value: serde_json::Number,
}

/// A whole-number answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerAnswer {
    /// The typed value.
    pub value: i64,
}

/// A typed boolean answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BooleanAnswer {
    /// The canonical business value, never a `Yes`/`No` display string.
    pub value: bool,
}

/// One declared option selected for a single-choice question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionAnswer {
    /// The zero-based index into the question's declared options.
    pub option_index: usize,
}

/// A custom answer entered by the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomAnswer {
    /// The bounded user-entered answer.
    pub answer: String,
}

/// Several declared options selected for a multi-choice question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionsAnswer {
    /// Zero-based option indices in ascending canonical order.
    pub option_indices: Vec<usize>,
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
///
/// Which variants are legal for a given question is decided by that question's
/// [`AnswerSpecification`], not by the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum QuestionnaireAnswer {
    /// Bounded free-form text for a [`AnswerSpecification::Text`] question.
    Text(TextAnswer),
    /// A finite number for a [`AnswerSpecification::Number`] question.
    Number(NumberAnswer),
    /// A whole number for an [`AnswerSpecification::Integer`] question.
    Integer(IntegerAnswer),
    /// A typed boolean for an [`AnswerSpecification::Boolean`] question.
    Boolean(BooleanAnswer),
    /// One declared option, addressed by index.
    Option(OptionAnswer),
    /// A bounded set of declared options, addressed by index.
    Options(OptionsAnswer),
    /// One bounded custom answer, legal only where `allow_custom` is set.
    Custom(CustomAnswer),
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
    Review {
        review: super::review::ReviewSpecification,
    },
    /// A tool invocation was held at the pre-tool policy boundary.
    Approval {
        /// Caller-neutral invocation correlation.
        invocation_id: crate::tools::types::ToolInvocationId,
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
        invocation_id: crate::tools::types::ToolInvocationId,
        /// The canonical identity of the tool that asked. Durable evidence of
        /// *who* asked, so an audit of an MCP elicitation names its server.
        requester: InteractionRequester,
        /// The exact immutable facts projected to the Runtime Client.
        questionnaire: QuestionnaireSpecification,
    },
}

/// The distinct durable terminal settlements of one interaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionSettlement {
    Reviewed {
        response: super::review::ReviewResponse,
    },
    ReviewInvalidated,
    /// A native execution deadline interrupted the approval rendezvous.
    DeadlineExpired {
        kind: crate::tools::deadline::ToolDeadlineKind,
    },
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

/// The finite value of a numeric bound or answer.
///
/// `serde_json::Number` cannot hold NaN or infinity, but a `u64` above
/// `2^53` still has no exact `f64`, so the conversion is explicit rather than
/// assumed.
fn finite(number: &serde_json::Number) -> Result<f64, String> {
    number
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| "the numeric value is not finite".to_owned())
}

fn validate_choice_options(
    options: &[OptionSpecification],
    allow_custom: bool,
    multi_select: bool,
    context: &str,
) -> Result<(), String> {
    let count = options.len();
    if !(MIN_CHOICE_OPTIONS..=MAX_CHOICE_OPTIONS).contains(&count) {
        return Err(format!(
            "{context} options must contain {MIN_CHOICE_OPTIONS}–{MAX_CHOICE_OPTIONS} options"
        ));
    }
    let mut labels = BTreeSet::new();
    for (option_index, option) in options.iter().enumerate() {
        validate_text(
            &option.label,
            &format!("{context} option {option_index} label"),
            MAX_OPTION_LABEL_CHARS,
        )?;
        // A client-reserved row only exists when the question offers a custom
        // answer, so the reservation is scoped to exactly that case. An MCP
        // `enum` may legitimately contain the value `Other`.
        if allow_custom && is_reserved_option_label(&option.label) {
            return Err(format!(
                "{context} option label {:?} is reserved for the client",
                option.label
            ));
        }
        // Labels are presentation, and a response addresses an option by
        // index — but two rows a human cannot tell apart are still ambiguous
        // *to the human*, so they are refused deterministically.
        if !labels.insert(&option.label) {
            return Err(format!("{context} option labels must be unique"));
        }
        validate_text(
            &option.description,
            &format!("{context} option {option_index} description"),
            MAX_OPTION_DESCRIPTION_CHARS,
        )?;
        if let Some(preview) = &option.preview {
            validate_text(
                preview,
                &format!("{context} option {option_index} preview"),
                MAX_OPTION_PREVIEW_CHARS,
            )?;
            if multi_select {
                return Err(format!(
                    "{context} multi-select options cannot include previews"
                ));
            }
        }
    }
    Ok(())
}

fn validate_answer_specification(
    specification: &AnswerSpecification,
    context: &str,
) -> Result<(), String> {
    match specification {
        AnswerSpecification::Text(text) => {
            if let Some(max) = text.max_length
                && max as usize > MAX_TEXT_ANSWER_CHARS
            {
                return Err(format!(
                    "{context} max_length exceeds the {MAX_TEXT_ANSWER_CHARS}-character bound"
                ));
            }
            if let (Some(min), Some(max)) = (text.min_length, text.max_length)
                && min > max
            {
                return Err(format!("{context} min_length exceeds its max_length"));
            }
            if let Some(min) = text.min_length
                && min as usize > MAX_TEXT_ANSWER_CHARS
            {
                return Err(format!(
                    "{context} min_length exceeds the {MAX_TEXT_ANSWER_CHARS}-character bound"
                ));
            }
            Ok(())
        }
        AnswerSpecification::Number(number) => {
            let minimum = number.minimum.as_ref().map(finite).transpose()?;
            let maximum = number.maximum.as_ref().map(finite).transpose()?;
            if let (Some(min), Some(max)) = (minimum, maximum)
                && min > max
            {
                return Err(format!("{context} minimum exceeds its maximum"));
            }
            Ok(())
        }
        AnswerSpecification::Integer(integer) => {
            if let (Some(min), Some(max)) = (integer.minimum, integer.maximum)
                && min > max
            {
                return Err(format!("{context} minimum exceeds its maximum"));
            }
            Ok(())
        }
        AnswerSpecification::Boolean => Ok(()),
        AnswerSpecification::SingleChoice(single) => {
            validate_choice_options(&single.options, single.allow_custom, false, context)
        }
        AnswerSpecification::MultiChoice(multi) => {
            validate_choice_options(&multi.options, multi.allow_custom, true, context)?;
            if multi.min_selected > multi.max_selected {
                return Err(format!("{context} min_selected exceeds its max_selected"));
            }
            let count = u32::try_from(multi.options.len()).unwrap_or(u32::MAX);
            if multi.max_selected > count {
                return Err(format!(
                    "{context} max_selected exceeds its declared option count"
                ));
            }
            Ok(())
        }
    }
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
        validate_answer_specification(&question.answer, &format!("question {question_index}"))?;
    }
    Ok(())
}

fn validate_custom(answer: &CustomAnswer) -> Result<(), String> {
    validate_text(&answer.answer, "custom answer", MAX_CUSTOM_ANSWER_CHARS)
}

/// Validates and canonicalizes one typed answer against its question's
/// immutable declared answer shape.
///
/// This is the **authoritative** validation point. A Runtime Client may reject
/// obviously invalid input earlier for the user's benefit, but nothing it does
/// or omits changes what the runtime accepts here.
#[allow(
    clippy::too_many_lines,
    reason = "the whole typed answer contract is one auditable match"
)]
fn validate_answer(
    question: &QuestionSpecification,
    question_index: usize,
    answer: &QuestionnaireAnswer,
) -> Result<QuestionnaireAnswer, String> {
    let mismatch =
        || format!("question index {question_index} does not accept an answer of that kind");
    let out_of_range = |detail: &str| format!("question index {question_index}: {detail}");
    match (&question.answer, answer) {
        (AnswerSpecification::Text(specification), QuestionnaireAnswer::Text(text)) => {
            let length = text.value.chars().count();
            if length > MAX_TEXT_ANSWER_CHARS {
                return Err(out_of_range(&format!(
                    "the text answer exceeds {MAX_TEXT_ANSWER_CHARS} characters"
                )));
            }
            if let Some(min) = specification.min_length
                && length < min as usize
            {
                return Err(out_of_range(&format!(
                    "the text answer is shorter than its {min}-character minimum"
                )));
            }
            if let Some(max) = specification.max_length
                && length > max as usize
            {
                return Err(out_of_range(&format!(
                    "the text answer is longer than its {max}-character maximum"
                )));
            }
            if let Some(format) = specification.format {
                format
                    .validate(&text.value)
                    .map_err(|error| out_of_range(&error))?;
            }
            Ok(QuestionnaireAnswer::Text(text.clone()))
        }
        (AnswerSpecification::Number(specification), QuestionnaireAnswer::Number(number)) => {
            let value = finite(&number.value).map_err(|error| out_of_range(&error))?;
            if let Some(minimum) = &specification.minimum {
                let minimum = finite(minimum).map_err(|error| out_of_range(&error))?;
                if value < minimum {
                    return Err(out_of_range("the number is below its declared minimum"));
                }
            }
            if let Some(maximum) = &specification.maximum {
                let maximum = finite(maximum).map_err(|error| out_of_range(&error))?;
                if value > maximum {
                    return Err(out_of_range("the number is above its declared maximum"));
                }
            }
            Ok(QuestionnaireAnswer::Number(number.clone()))
        }
        (AnswerSpecification::Integer(specification), QuestionnaireAnswer::Integer(integer)) => {
            if let Some(minimum) = specification.minimum
                && integer.value < minimum
            {
                return Err(out_of_range("the integer is below its declared minimum"));
            }
            if let Some(maximum) = specification.maximum
                && integer.value > maximum
            {
                return Err(out_of_range("the integer is above its declared maximum"));
            }
            Ok(QuestionnaireAnswer::Integer(*integer))
        }
        (AnswerSpecification::Boolean, QuestionnaireAnswer::Boolean(boolean)) => {
            Ok(QuestionnaireAnswer::Boolean(*boolean))
        }
        (AnswerSpecification::SingleChoice(single), QuestionnaireAnswer::Option(option)) => {
            if option.option_index >= single.options.len() {
                return Err(out_of_range("the selected option index is out of range"));
            }
            Ok(QuestionnaireAnswer::Option(*option))
        }
        (AnswerSpecification::MultiChoice(multi), QuestionnaireAnswer::Options(options)) => {
            let mut selected = BTreeSet::new();
            for index in &options.option_indices {
                if *index >= multi.options.len() {
                    return Err(out_of_range("a selected option index is out of range"));
                }
                if !selected.insert(*index) {
                    return Err(out_of_range("a selected option index is duplicated"));
                }
            }
            let count = u32::try_from(selected.len()).unwrap_or(u32::MAX);
            if count < multi.min_selected {
                return Err(out_of_range(&format!(
                    "the answer selects {count} options, below its {} minimum",
                    multi.min_selected
                )));
            }
            if count > multi.max_selected {
                return Err(out_of_range(&format!(
                    "the answer selects {count} options, above its {} maximum",
                    multi.max_selected
                )));
            }
            // Canonical ascending option order, so the durable settlement of
            // one selection set has exactly one spelling.
            Ok(QuestionnaireAnswer::Options(OptionsAnswer {
                option_indices: selected.into_iter().collect(),
            }))
        }
        (specification, QuestionnaireAnswer::Custom(custom)) => {
            if !specification.allows_custom() {
                return Err(out_of_range(
                    "the question does not allow a free-text answer",
                ));
            }
            validate_custom(custom)?;
            Ok(QuestionnaireAnswer::Custom(custom.clone()))
        }
        _ => Err(mismatch()),
    }
}

/// Validates and canonically orders a submitted answer set.
///
/// # Errors
///
/// Returns an error when an answer has an invalid index, kind, option index,
/// multiplicity, bound, or custom-answer length.
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
        let answer = validate_answer(question, entry.question_index, &entry.answer)?;
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
/// reason, requester, or questionnaire specification.
pub fn validate_interaction_subject(subject: &InteractionSubject) -> Result<(), String> {
    match subject {
        InteractionSubject::Review { review } => review.validate(),
        InteractionSubject::Approval {
            invocation_id,
            tool_id,
            tool_name,
            arguments_digest,
            reason,
        } => {
            if invocation_id
                .canonical_call_id()
                .is_some_and(|id| id.as_str().is_empty())
            {
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
        InteractionSubject::Questionnaire {
            requester,
            questionnaire,
            ..
        } => {
            requester.validate()?;
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
        (InteractionSubject::Review { review }, InteractionSettlement::Reviewed { response }) => {
            review.validate_response(response)
        }
        (
            _,
            InteractionSettlement::Cancelled { .. } | InteractionSettlement::DeadlineExpired { .. },
        )
        | (InteractionSubject::Approval { .. }, InteractionSettlement::Approved)
        | (InteractionSubject::Review { .. }, InteractionSettlement::ReviewInvalidated) => Ok(()),
        (InteractionSubject::Approval { .. }, InteractionSettlement::Denied { reason }) => {
            if reason.chars().count() > MAX_APPROVAL_DENIAL_REASON_CHARS {
                return Err(format!(
                    "approval denial reason exceeds {MAX_APPROVAL_DENIAL_REASON_CHARS} characters"
                ));
            }
            Ok(())
        }
        (
            InteractionSubject::Questionnaire { questionnaire, .. },
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
            InteractionSubject::Questionnaire { questionnaire, .. },
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

    pub(crate) fn option(label: &str, description: &str) -> OptionSpecification {
        OptionSpecification {
            label: label.to_owned(),
            description: description.to_owned(),
            preview: None,
        }
    }

    fn questionnaire() -> QuestionnaireSpecification {
        QuestionnaireSpecification {
            questions: vec![QuestionSpecification {
                question: "Which target?".to_owned(),
                header: "Target".to_owned(),
                answer: AnswerSpecification::SingleChoice(SingleChoiceSpecification {
                    options: vec![
                        option("Staging", "A safe test environment."),
                        option("Production", "The live environment."),
                    ],
                    allow_custom: true,
                }),
            }],
        }
    }

    fn requester() -> InteractionRequester {
        InteractionRequester {
            tool_id: ToolId::new("tool-ask-user"),
            tool_name: "ask_user".to_owned(),
            origin: ToolOrigin::Builtin,
        }
    }

    fn questionnaire_subject(questionnaire: QuestionnaireSpecification) -> InteractionSubject {
        InteractionSubject::Questionnaire {
            invocation_id: crate::tools::types::ToolInvocationId::Agent {
                call_id: ToolCallId::new("questionnaire-call"),
            },
            requester: requester(),
            questionnaire,
        }
    }

    fn approval(reason: &str) -> InteractionSubject {
        InteractionSubject::Approval {
            invocation_id: crate::tools::types::ToolInvocationId::Agent {
                call_id: ToolCallId::new("call-1"),
            },
            tool_id: ToolId::new("tool.read"),
            tool_name: "read".to_owned(),
            arguments_digest: interaction_arguments_digest(&serde_json::json!({"path": "a"})),
            reason: reason.to_owned(),
        }
    }

    fn single(index: usize, option_index: usize) -> QuestionnaireAnswerEntry {
        QuestionnaireAnswerEntry {
            question_index: index,
            answer: QuestionnaireAnswer::Option(OptionAnswer { option_index }),
        }
    }

    fn submitted(answers: Vec<QuestionnaireAnswerEntry>) -> QuestionnaireResponse {
        QuestionnaireResponse::Submitted(QuestionnaireSubmission { answers })
    }

    #[test]
    fn questionnaire_bounds_and_reserved_labels_are_enforced() {
        assert!(validate_questionnaire(&questionnaire()).is_ok());
        let mut null_preview = serde_json::to_value(questionnaire()).expect("questionnaire JSON");
        null_preview["questions"][0]["answer"]["options"][0]["preview"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<QuestionnaireSpecification>(null_preview).is_err());

        let mut invalid = questionnaire();
        options_mut(&mut invalid)[0].label = "Other".to_owned();
        assert!(validate_questionnaire(&invalid).is_err());
        let mut invalid = questionnaire();
        options_mut(&mut invalid)[0].description.clear();
        assert!(validate_questionnaire(&invalid).is_err());

        let mut invalid = questionnaire();
        invalid.questions[0].question = "duplicate".to_owned();
        invalid.questions.push(invalid.questions[0].clone());
        assert!(validate_questionnaire(&invalid).is_err());

        for reserved in ["Other", "Type something.", "Next"] {
            let mut invalid = questionnaire();
            options_mut(&mut invalid)[0].label = reserved.to_owned();
            assert!(validate_questionnaire(&invalid).is_err());
        }

        let mut invalid = questionnaire();
        invalid.questions[0].header = "h".repeat(MAX_QUESTION_HEADER_CHARS + 1);
        assert!(validate_questionnaire(&invalid).is_err());
        let mut invalid = questionnaire();
        options_mut(&mut invalid)[0].description = "d".repeat(MAX_OPTION_DESCRIPTION_CHARS + 1);
        assert!(validate_questionnaire(&invalid).is_err());
        let mut invalid = questionnaire();
        options_mut(&mut invalid)[0].preview = Some("p".repeat(MAX_OPTION_PREVIEW_CHARS + 1));
        assert!(validate_questionnaire(&invalid).is_err());

        let mut invalid = questionnaire();
        options_mut(&mut invalid)[0].label = "é".repeat(MAX_OPTION_LABEL_CHARS + 1);
        assert!(validate_questionnaire(&invalid).is_err());

        let mut invalid = questionnaire();
        let label = options_mut(&mut invalid)[0].label.clone();
        options_mut(&mut invalid)[1].label = label;
        assert!(validate_questionnaire(&invalid).is_err());
    }

    fn options_mut(
        questionnaire: &mut QuestionnaireSpecification,
    ) -> &mut Vec<OptionSpecification> {
        match &mut questionnaire.questions[0].answer {
            AnswerSpecification::SingleChoice(single) => &mut single.options,
            AnswerSpecification::MultiChoice(multi) => &mut multi.options,
            _ => panic!("the fixture question is a choice question"),
        }
    }

    /// A reserved client row only exists where a custom answer does, so a
    /// bounded MCP `enum` may legitimately offer the value `Other`.
    #[test]
    fn reserved_labels_are_scoped_to_questions_that_offer_a_custom_answer() {
        let bounded = QuestionnaireSpecification {
            questions: vec![QuestionSpecification {
                question: "Which channel?".to_owned(),
                header: "Channel".to_owned(),
                answer: AnswerSpecification::SingleChoice(SingleChoiceSpecification {
                    options: vec![
                        option("Other", "The other channel."),
                        option("keep", "Keep."),
                    ],
                    allow_custom: false,
                }),
            }],
        };
        assert!(validate_questionnaire(&bounded).is_ok());
    }

    #[test]
    fn submitted_answers_are_ordered_by_question_and_canonical_option_index() {
        let mut spec = questionnaire();
        spec.questions.push(QuestionSpecification {
            question: "Which extras?".to_owned(),
            header: "Extras".to_owned(),
            answer: AnswerSpecification::MultiChoice(MultiChoiceSpecification {
                options: vec![
                    option("Charts", "Show charts."),
                    option("Comments", "Show comments."),
                ],
                min_selected: 1,
                max_selected: 2,
                allow_custom: true,
            }),
        });
        let normalized = normalize_questionnaire_submission(
            &spec,
            &QuestionnaireSubmission {
                answers: vec![
                    QuestionnaireAnswerEntry {
                        question_index: 1,
                        answer: QuestionnaireAnswer::Options(OptionsAnswer {
                            option_indices: vec![1, 0],
                        }),
                    },
                    single(0, 0),
                ],
            },
        )
        .expect("valid response");
        assert_eq!(normalized.answers[0].question_index, 0);
        assert_eq!(
            normalized.answers[1].answer,
            QuestionnaireAnswer::Options(OptionsAnswer {
                option_indices: vec![0, 1],
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
                &questionnaire_subject(questionnaire()),
                &InteractionSettlement::QuestionnaireSubmitted {
                    submission: QuestionnaireSubmission { answers: vec![] },
                },
            )
            .is_err()
        );
    }

    #[test]
    fn custom_answers_are_non_empty_bounded_and_only_where_declared() {
        let response = submitted(vec![QuestionnaireAnswerEntry {
            question_index: 0,
            answer: QuestionnaireAnswer::Custom(CustomAnswer {
                answer: " ".to_owned(),
            }),
        }]);
        assert!(normalize_questionnaire_response(&questionnaire(), &response).is_err());

        let response = submitted(vec![QuestionnaireAnswerEntry {
            question_index: 0,
            answer: QuestionnaireAnswer::Custom(CustomAnswer {
                answer: "x".repeat(MAX_CUSTOM_ANSWER_CHARS + 1),
            }),
        }]);
        assert!(normalize_questionnaire_response(&questionnaire(), &response).is_err());

        // The same custom answer against a question that forbids one.
        let mut bounded = questionnaire();
        match &mut bounded.questions[0].answer {
            AnswerSpecification::SingleChoice(single) => single.allow_custom = false,
            _ => panic!("choice question"),
        }
        let response = submitted(vec![QuestionnaireAnswerEntry {
            question_index: 0,
            answer: QuestionnaireAnswer::Custom(CustomAnswer {
                answer: "nightly".to_owned(),
            }),
        }]);
        let error = normalize_questionnaire_response(&bounded, &response)
            .expect_err("a bounded choice has no custom answer shape");
        assert!(error.contains("free-text"), "{error}");
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one test proves every typed scalar shape against its bounds"
    )]
    fn typed_scalar_answers_are_validated_against_their_declared_bounds() {
        let spec = |answer: AnswerSpecification| QuestionnaireSpecification {
            questions: vec![QuestionSpecification {
                question: "Value?".to_owned(),
                header: "Value".to_owned(),
                answer,
            }],
        };
        let answered = |answer: QuestionnaireAnswer| {
            submitted(vec![QuestionnaireAnswerEntry {
                question_index: 0,
                answer,
            }])
        };

        let text = spec(AnswerSpecification::Text(TextAnswerSpecification {
            min_length: Some(3),
            max_length: Some(6),
            format: None,
        }));
        assert!(
            normalize_questionnaire_response(
                &text,
                &answered(QuestionnaireAnswer::Text(TextAnswer {
                    value: "abcd".to_owned()
                }))
            )
            .is_ok()
        );
        for invalid in ["ab", "abcdefg"] {
            assert!(
                normalize_questionnaire_response(
                    &text,
                    &answered(QuestionnaireAnswer::Text(TextAnswer {
                        value: invalid.to_owned()
                    }))
                )
                .is_err(),
                "{invalid}"
            );
        }
        // A text question has no option/custom answer shape.
        assert!(
            normalize_questionnaire_response(
                &text,
                &answered(QuestionnaireAnswer::Option(OptionAnswer {
                    option_index: 0
                }))
            )
            .is_err()
        );

        let dated = spec(AnswerSpecification::Text(TextAnswerSpecification {
            min_length: None,
            max_length: None,
            format: Some(TextFormat::Date),
        }));
        assert!(
            normalize_questionnaire_response(
                &dated,
                &answered(QuestionnaireAnswer::Text(TextAnswer {
                    value: "2026-09-09".to_owned()
                }))
            )
            .is_ok()
        );
        assert!(
            normalize_questionnaire_response(
                &dated,
                &answered(QuestionnaireAnswer::Text(TextAnswer {
                    value: "09/09/2026".to_owned()
                }))
            )
            .is_err()
        );

        let integer = spec(AnswerSpecification::Integer(IntegerAnswerSpecification {
            minimum: Some(0),
            maximum: Some(150),
        }));
        assert!(
            normalize_questionnaire_response(
                &integer,
                &answered(QuestionnaireAnswer::Integer(IntegerAnswer { value: 42 }))
            )
            .is_ok()
        );
        assert!(
            normalize_questionnaire_response(
                &integer,
                &answered(QuestionnaireAnswer::Integer(IntegerAnswer { value: 151 }))
            )
            .is_err()
        );
        // A fractional value cannot even be spelled as an Integer answer, and
        // a Number answer is the wrong kind for an Integer question.
        assert!(
            serde_json::from_value::<QuestionnaireAnswer>(serde_json::json!({
                "type": "integer",
                "value": {"value": 1.5},
            }))
            .is_err()
        );
        assert!(
            normalize_questionnaire_response(
                &integer,
                &answered(QuestionnaireAnswer::Number(NumberAnswer {
                    value: serde_json::Number::from_f64(1.5).expect("finite")
                }))
            )
            .is_err()
        );

        let number = spec(AnswerSpecification::Number(NumberAnswerSpecification {
            minimum: Some(serde_json::Number::from_f64(-1.5).expect("finite")),
            maximum: Some(serde_json::Number::from_f64(1.5).expect("finite")),
        }));
        assert!(
            normalize_questionnaire_response(
                &number,
                &answered(QuestionnaireAnswer::Number(NumberAnswer {
                    value: serde_json::Number::from_f64(1.5).expect("finite")
                }))
            )
            .is_ok()
        );
        assert!(
            normalize_questionnaire_response(
                &number,
                &answered(QuestionnaireAnswer::Number(NumberAnswer {
                    value: serde_json::Number::from_f64(1.6).expect("finite")
                }))
            )
            .is_err()
        );

        let boolean = spec(AnswerSpecification::Boolean);
        for value in [true, false] {
            let response = normalize_questionnaire_response(
                &boolean,
                &answered(QuestionnaireAnswer::Boolean(BooleanAnswer { value })),
            )
            .expect("a boolean answer");
            assert_eq!(
                response,
                submitted(vec![QuestionnaireAnswerEntry {
                    question_index: 0,
                    answer: QuestionnaireAnswer::Boolean(BooleanAnswer { value }),
                }])
            );
        }
    }

    #[test]
    fn multi_choice_selection_bounds_are_authoritative() {
        let spec = |min_selected: u32, max_selected: u32| QuestionnaireSpecification {
            questions: vec![QuestionSpecification {
                question: "Which regions?".to_owned(),
                header: "Regions".to_owned(),
                answer: AnswerSpecification::MultiChoice(MultiChoiceSpecification {
                    options: vec![option("a", "A."), option("b", "B."), option("c", "C.")],
                    min_selected,
                    max_selected,
                    allow_custom: false,
                }),
            }],
        };
        let selected = |indices: Vec<usize>| {
            submitted(vec![QuestionnaireAnswerEntry {
                question_index: 0,
                answer: QuestionnaireAnswer::Options(OptionsAnswer {
                    option_indices: indices,
                }),
            }])
        };

        let exactly_two = spec(2, 2);
        assert!(normalize_questionnaire_response(&exactly_two, &selected(vec![0])).is_err());
        assert!(normalize_questionnaire_response(&exactly_two, &selected(vec![0, 1])).is_ok());
        assert!(normalize_questionnaire_response(&exactly_two, &selected(vec![0, 1, 2])).is_err());

        let up_to_two = spec(0, 2);
        assert!(normalize_questionnaire_response(&up_to_two, &selected(vec![])).is_ok());
        assert!(normalize_questionnaire_response(&up_to_two, &selected(vec![0, 1, 2])).is_err());

        // Duplicated indices are a malformed answer, not a silent dedupe.
        assert!(normalize_questionnaire_response(&up_to_two, &selected(vec![0, 0])).is_err());
        // Out-of-range indices can never address an option.
        assert!(normalize_questionnaire_response(&up_to_two, &selected(vec![3])).is_err());

        // Impossible bounds are refused at the specification, before publication.
        assert!(validate_questionnaire(&spec(3, 2)).is_err());
        assert!(validate_questionnaire(&spec(0, 4)).is_err());
    }

    #[test]
    fn requester_facts_are_bounded_and_carry_mcp_server_identity() {
        let subject = questionnaire_subject(questionnaire());
        assert!(validate_interaction_subject(&subject).is_ok());

        let InteractionSubject::Questionnaire {
            invocation_id,
            questionnaire,
            ..
        } = subject
        else {
            panic!("questionnaire subject")
        };
        let mcp = InteractionSubject::Questionnaire {
            invocation_id,
            requester: InteractionRequester {
                tool_id: ToolId::new("mcp:github:create_issue"),
                tool_name: "create_issue".to_owned(),
                origin: ToolOrigin::Mcp {
                    server_id: crate::runtime::identity::McpServerId::new("github"),
                },
            },
            questionnaire,
        };
        assert!(validate_interaction_subject(&mcp).is_ok());
        let InteractionSubject::Questionnaire {
            requester: mcp_requester,
            ..
        } = &mcp
        else {
            panic!("questionnaire subject")
        };
        assert_eq!(
            mcp_requester.mcp_server().map(ToString::to_string),
            Some("github".to_owned())
        );
        assert!(requester().mcp_server().is_none());

        let mut invalid = requester();
        invalid.tool_name = String::new();
        assert!(invalid.validate().is_err());
        let mut invalid = requester();
        invalid.tool_name = "n".repeat(MAX_REQUESTER_TOOL_NAME_CHARS + 1);
        assert!(invalid.validate().is_err());
        let mut invalid = requester();
        invalid.tool_id = ToolId::new("");
        assert!(invalid.validate().is_err());
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
