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
//!
//! # The scalar domains
//!
//! Each scalar shape names exactly one domain, and that same domain is used by
//! the request bound, the Runtime Client wire, the authoritative comparison,
//! and the value finally emitted to a provider. There is no stage that widens
//! or narrows:
//!
//! ```text
//! Number   finite IEEE-754 binary64      [`FiniteNumber`]
//!          bound, wire, comparison, emitted value: all binary64.
//!          A JSON number binary64 cannot hold exactly (`2^53 + 1`) is
//!          refused at the wire, never rounded into range.
//!
//! Integer  exact i64                     [`ExactInteger`]
//!          carried over the Runtime Client protocol as canonical decimal
//!          *text*, so a whole number above `2^53` survives a JavaScript
//!          client unchanged. One authoritative parse: [`ExactInteger::parse`].
//!
//! Text     bounded Unicode scalars       [`TextAnswer`]
//!          an **omitted** answer and an explicit `Text("")` are different
//!          facts. A questionnaire submission may leave a question
//!          unanswered; that is not the same as answering it with the empty
//!          string, which is legal whenever `min_length` is absent or `0` and
//!          which reaches a provider as a real empty value.
//! ```

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

/// The canonical rustX numeric domain: one finite IEEE-754 binary64 value.
///
/// # Why binary64, and why exactly one domain
///
/// MCP types an elicitation `number` bound as a binary64 —
/// [`rmcp::model::NumberSchema`]'s `minimum` and `maximum` are `Option<f64>` —
/// and a Runtime Client's JSON number is a binary64 as well. Binary64 is
/// therefore not a convenience: it is the widest value *every* stage of the
/// pipeline can hold without rounding, so it is the one domain all of them
/// share.
///
/// ```text
/// MCP NumberSchema bound (f64)
///   -> NumberAnswerSpecification bound (FiniteNumber)
///   -> Runtime Client JSON number (binary64)
///   -> NumberAnswer value          (FiniteNumber)
///   -> authoritative range comparison (FiniteNumber)
///   -> MCP accept.content JSON number (the same FiniteNumber)
/// ```
///
/// No stage holds a wider value than the next one, so there is no widening or
/// narrowing to hide a mismatch in.
///
/// # The precision failure this type exists to make impossible
///
/// Storing an answer as an arbitrary [`serde_json::Number`] and *comparing* it
/// as an `f64` is two domains, not one, and it is unsound above `2^53`:
/// `9007199254740992` and `9007199254740993` are distinct JSON numbers with
/// the same `f64`, so an answer could pass a `maximum = 9007199254740992`
/// check and then be emitted to the server unchanged as `9007199254740993`.
///
/// [`FiniteNumber`] closes that by refusing, at the wire boundary, any JSON
/// number binary64 cannot hold **exactly**: `9007199254740993` is rejected as
/// unrepresentable rather than silently rounded down into range. What is
/// validated is thus always bit-identical to what is emitted.
///
/// The rule is stated on the **value**, not on the spelling, so it cannot be
/// evaded by writing the same whole number as `9007199254740993.0`: a whole
/// number at or past `2^53` that a JSON integer could spell is refused
/// whichever way it is written, and this type always *writes* such a number as
/// a JSON integer — the one spelling whose exactness a reader can check. Past
/// the range a JSON integer can spell there is no exact integer form at all,
/// so the nearest binary64 is the only meaning such a literal can carry and it
/// is admitted; that keeps serialization and parsing exact inverses, so a
/// value this type can hold can always be read back.
///
/// Below the frontier every whole number is exact, and a fractional value is
/// the nearest binary64 — which is what JSON numbers mean everywhere, and the
/// domain the MCP server's own parse lands in too.
///
/// NaN and infinity are unrepresentable by construction, which is what makes
/// [`Eq`] sound here: every value this type can hold is reflexive.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct FiniteNumber(f64);

impl Eq for FiniteNumber {}

impl FiniteNumber {
    /// The finite binary64 value.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is NaN or infinite.
    pub fn try_new(value: f64) -> Result<Self, String> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err("the numeric value is not finite".to_owned())
        }
    }

    /// The underlying binary64 value, which is the comparison domain.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }

    /// The exact JSON number for this value.
    ///
    /// [`serde_json::Number::from_f64`] returns `None` only for NaN and
    /// infinity, neither of which this type can hold, so the conversion is
    /// total in practice. The signature stays honest instead of panicking on
    /// a branch that cannot be reached.
    #[must_use]
    pub fn to_json_number(self) -> Option<serde_json::Number> {
        serde_json::Number::from_f64(self.0)
    }

    /// The exactly representable binary64 value of one JSON number.
    ///
    /// A JSON integer above `2^53` that binary64 cannot hold exactly is an
    /// error, never a rounded value: see the type documentation.
    ///
    /// # Errors
    ///
    /// Returns an error when the JSON number is not finite or is not exactly
    /// representable in binary64.
    #[allow(
        clippy::cast_precision_loss,
        clippy::float_cmp,
        reason = "each widening is guarded by an exactness proof taken on the integer \
                  bits first, and the whole-number test is an exact comparison by \
                  intent — an approximate one would be the very defect being fixed"
    )]
    pub fn from_json_number(number: &serde_json::Number) -> Result<Self, String> {
        let inexact = || {
            format!(
                "{number} is not exactly representable as a finite 64-bit binary \
                 floating-point number"
            )
        };
        if let Some(value) = number.as_u64() {
            if !is_exact_binary64_whole(value) {
                return Err(inexact());
            }
            return Self::try_new(value as f64);
        }
        if let Some(value) = number.as_i64() {
            if !is_exact_binary64_whole(value.unsigned_abs()) {
                return Err(inexact());
            }
            return Self::try_new(value as f64);
        }
        let value = number.as_f64().ok_or_else(inexact)?;
        // A fractional JSON spelling *is* the nearest binary64 — that is the
        // declared contract, and the server's own parse lands on the same
        // value. But the JSON parser has already rounded it, so a spelling
        // that denotes a **whole** number at or past the precision frontier
        // can no longer be proven to be the decimal the client wrote:
        // `9007199254740993.0` and `9007199254740992.0` are the same bits by
        // the time they arrive. rustX refuses that class rather than assume,
        // so the failing value has no admitting spelling here either.
        //
        // The refusal is scoped to values that *have* a JSON integer spelling,
        // which is exactly the form this type serializes them in and the form
        // whose exactness the paths above can check. Beyond that range no
        // exact integer spelling exists at all, so the nearest binary64 is the
        // only meaning such a literal can carry, and refusing it would make a
        // value this type can hold impossible to read back.
        if integer_spelled(value) && value.abs() >= EXACT_WHOLE_FRONTIER {
            return Err(inexact());
        }
        Self::try_new(value)
    }
}

/// `2^53`, the magnitude at which consecutive whole numbers stop being
/// distinguishable in binary64.
const EXACT_WHOLE_FRONTIER: f64 = 9_007_199_254_740_992.0;
/// `-2^63`, the least value a JSON integer can spell.
const LEAST_SPELLED_INTEGER: f64 = -9_223_372_036_854_775_808.0;
/// `2^64`, one past the greatest value a JSON integer can spell.
const PAST_GREATEST_SPELLED_INTEGER: f64 = 18_446_744_073_709_551_616.0;

/// Whether this value has a canonical JSON **integer** spelling.
///
/// [`serde_json::Number`] holds a whole number exactly when it fits `i64` or
/// `u64`, so this predicate is the exact boundary between the two wire forms
/// [`FiniteNumber`] uses — and it is what makes serialization and
/// deserialization inverses: a whole number inside this range always crosses
/// the wire as a JSON integer and always returns through the exact integer
/// path, while everything else crosses as a JSON float and is never refused
/// by the frontier rule.
#[allow(
    clippy::float_cmp,
    reason = "an exact whole-number test is the intent; a tolerance would be the defect"
)]
fn integer_spelled(value: f64) -> bool {
    value.fract() == 0.0 && (LEAST_SPELLED_INTEGER..PAST_GREATEST_SPELLED_INTEGER).contains(&value)
}

impl Serialize for FiniteNumber {
    /// A whole number serializes as a JSON **integer** whenever one can spell
    /// it, because that is the only spelling whose exactness a reader can
    /// check — and this type's own reader refuses a whole number at or past
    /// the `2^53` frontier written any other way. Everything else serializes
    /// as a JSON float.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the cast is guarded by `integer_spelled`, which proves the value is a \
                  whole number inside the target's exact range"
    )]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if integer_spelled(self.0) {
            if self.0 < 0.0 {
                return serializer.serialize_i64(self.0 as i64);
            }
            return serializer.serialize_u64(self.0 as u64);
        }
        serializer.serialize_f64(self.0)
    }
}

/// Whether binary64 holds this whole magnitude exactly.
///
/// A whole number is exactly representable iff its significand fits the 53
/// bits binary64 has — iff the span from its highest set bit down to its
/// lowest is at most 53 bits wide.
///
/// Deciding this on the integer bits is deliberate. The obvious
/// `value as f64 as u64 == value` round trip is **wrong**, because a narrowing
/// `as` cast saturates: `u64::MAX` widens to `2^64` and narrows back to
/// `u64::MAX`, so the inexact value proves itself exact. That is the same
/// class of silent numeric agreement this whole type exists to prevent.
const fn is_exact_binary64_whole(magnitude: u64) -> bool {
    if magnitude == 0 {
        return true;
    }
    let significant = u64::BITS - magnitude.leading_zeros() - magnitude.trailing_zeros();
    significant <= f64::MANTISSA_DIGITS
}

impl std::fmt::Display for FiniteNumber {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for FiniteNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let number = serde_json::Number::deserialize(deserializer)?;
        Self::from_json_number(&number).map_err(serde::de::Error::custom)
    }
}

/// The canonical rustX whole-number domain: one exact `i64`, carried across
/// the Runtime Client protocol as a canonical decimal **string**.
///
/// # Why `i64`, and why a string on the wire
///
/// MCP types an elicitation `integer` bound as an `i64`
/// ([`rmcp::model::IntegerSchema`]'s `minimum` and `maximum` are
/// `Option<i64>`), so `i64` is exactly the domain the protocol hands rustX. No
/// MCP integer schema can name a bound outside it, which is why refusing an
/// out-of-domain integer schema is vacuous here rather than missing.
///
/// The Runtime Client protocol is the stage that cannot hold that domain: a
/// JavaScript `number` is a binary64 and loses whole numbers above `2^53`. A
/// question whose only legal answers are, say, `9007199254740992..=
/// 9007199254740993` would then be publishable by the runtime and
/// *unanswerable* by any client — a published question with no faithful
/// response representation.
///
/// So the value crosses the wire as its canonical decimal text and is parsed
/// back to `i64` exactly once, by the runtime, which stays authoritative:
///
/// ```text
/// MCP IntegerSchema bound (i64)
///   -> IntegerAnswerSpecification bound (ExactInteger)  "9007199254740993"
///   -> Runtime Client JSON string                       "9007199254740993"
///   -> TUI draft, edited as decimal text                 9007199254740993
///   -> ExactInteger::parse — the one authoritative parse (i64)
///   -> authoritative range comparison                    (i64)
///   -> MCP accept.content JSON integer                   9007199254740993
/// ```
///
/// No stage converts through a binary64, so no stage can round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct ExactInteger(i64);

impl ExactInteger {
    /// The exact whole number.
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// The underlying `i64`, which is the comparison domain.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Parses the canonical decimal spelling of a whole number.
    ///
    /// This is the **one authoritative parse point** for an integer answer.
    /// The accepted syntax is an optional `-` followed by one or more ASCII
    /// digits, and nothing else: `1.5`, `1e3`, `+1`, `-`, `NaN`, `Infinity`,
    /// and `12abc` are all refused, as is any value outside `i64`.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not a decimal integer or names a
    /// value outside the supported 64-bit range.
    pub fn parse(text: &str) -> Result<Self, String> {
        let digits = text.strip_prefix('-').unwrap_or(text);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!(
                "{text:?} is not a decimal integer: write an optional \"-\" followed by digits"
            ));
        }
        text.parse::<i64>()
            .map(Self)
            .map_err(|_| format!("{text:?} is outside the supported 64-bit whole-number range"))
    }
}

impl From<i64> for ExactInteger {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for ExactInteger {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl Serialize for ExactInteger {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // `i64::to_string` is already canonical: no `+`, no leading zeros, and
        // no `-0`, so one value has exactly one settled spelling.
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for ExactInteger {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// A finite numeric question over the canonical [`FiniteNumber`] domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberAnswerSpecification {
    /// The inclusive minimum, when the producer declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<FiniteNumber>,
    /// The inclusive maximum, when the producer declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<FiniteNumber>,
}

/// A whole-number question, semantically distinct from [`NumberAnswerSpecification`].
///
/// The bounds are [`ExactInteger`]s, so they cross the Runtime Client protocol
/// as decimal text and never through a binary64.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerAnswerSpecification {
    /// The inclusive minimum, when the producer declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<ExactInteger>,
    /// The inclusive maximum, when the producer declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<ExactInteger>,
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
///
/// The value is a [`FiniteNumber`], the same domain the question's bounds and
/// the emitted MCP content use, so what the runtime validates is bit-identical
/// to what it sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberAnswer {
    /// The typed value, exactly representable as a finite binary64.
    pub value: FiniteNumber,
}

/// A whole-number answer.
///
/// The value is an [`ExactInteger`], which crosses the Runtime Client protocol
/// as canonical decimal text, so an answer above the binary64 integer frontier
/// round-trips exactly instead of being rounded by a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerAnswer {
    /// The typed value.
    pub value: ExactInteger,
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
            // Both bounds are already finite binary64 by construction, so the
            // only remaining question is whether they describe a satisfiable
            // interval — compared in that same one domain.
            if let (Some(min), Some(max)) = (number.minimum, number.maximum)
                && min.get() > max.get()
            {
                return Err(format!("{context} minimum exceeds its maximum"));
            }
            Ok(())
        }
        AnswerSpecification::Integer(integer) => {
            if let (Some(min), Some(max)) = (integer.minimum, integer.maximum)
                && min.get() > max.get()
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
            // The answer, both bounds, and the value later written into MCP
            // `accept.content` are all the same `FiniteNumber`. A value that
            // binary64 cannot hold exactly never reaches here: it is refused
            // when the response is decoded, not rounded into range.
            let value = number.value.get();
            if let Some(minimum) = specification.minimum
                && value < minimum.get()
            {
                return Err(out_of_range("the number is below its declared minimum"));
            }
            if let Some(maximum) = specification.maximum
                && value > maximum.get()
            {
                return Err(out_of_range("the number is above its declared maximum"));
            }
            Ok(QuestionnaireAnswer::Number(*number))
        }
        (AnswerSpecification::Integer(specification), QuestionnaireAnswer::Integer(integer)) => {
            // Exact `i64` comparison, on the value the one authoritative parse
            // point produced. Nothing here has passed through a binary64.
            let value = integer.value.get();
            if let Some(minimum) = specification.minimum
                && value < minimum.get()
            {
                return Err(out_of_range("the integer is below its declared minimum"));
            }
            if let Some(maximum) = specification.maximum
                && value > maximum.get()
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
            minimum: Some(ExactInteger::new(0)),
            maximum: Some(ExactInteger::new(150)),
        }));
        assert!(
            normalize_questionnaire_response(
                &integer,
                &answered(QuestionnaireAnswer::Integer(IntegerAnswer {
                    value: ExactInteger::new(42)
                }))
            )
            .is_ok()
        );
        assert!(
            normalize_questionnaire_response(
                &integer,
                &answered(QuestionnaireAnswer::Integer(IntegerAnswer {
                    value: ExactInteger::new(151)
                }))
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
                    value: FiniteNumber::try_new(1.5).expect("finite")
                }))
            )
            .is_err()
        );

        let number = spec(AnswerSpecification::Number(NumberAnswerSpecification {
            minimum: Some(FiniteNumber::try_new(-1.5).expect("finite")),
            maximum: Some(FiniteNumber::try_new(1.5).expect("finite")),
        }));
        assert!(
            normalize_questionnaire_response(
                &number,
                &answered(QuestionnaireAnswer::Number(NumberAnswer {
                    value: FiniteNumber::try_new(1.5).expect("finite")
                }))
            )
            .is_ok()
        );
        assert!(
            normalize_questionnaire_response(
                &number,
                &answered(QuestionnaireAnswer::Number(NumberAnswer {
                    value: FiniteNumber::try_new(1.6).expect("finite")
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

    /// One question of the given shape, and one answer to it.
    fn scalar(answer: AnswerSpecification) -> QuestionnaireSpecification {
        QuestionnaireSpecification {
            questions: vec![QuestionSpecification {
                question: "What value?".to_owned(),
                header: "Value".to_owned(),
                answer,
            }],
        }
    }

    fn only(answer: QuestionnaireAnswer) -> QuestionnaireResponse {
        submitted(vec![QuestionnaireAnswerEntry {
            question_index: 0,
            answer,
        }])
    }

    /// The single settled answer of an accepted response.
    fn settled(response: &QuestionnaireResponse) -> QuestionnaireAnswer {
        match response {
            QuestionnaireResponse::Submitted(submission) => submission.answers[0].answer.clone(),
            QuestionnaireResponse::Declined => panic!("expected a submitted response"),
        }
    }

    /// The exact JSON spellings around the binary64 integer frontier.
    const TWO_POW_53_MINUS_1: &str = "9007199254740991";
    const TWO_POW_53: &str = "9007199254740992";
    const TWO_POW_53_PLUS_1: &str = "9007199254740993";

    #[test]
    fn a_number_answer_is_the_finite_binary64_domain_end_to_end() {
        let number = |value: f64| {
            QuestionnaireAnswer::Number(NumberAnswer {
                value: FiniteNumber::try_new(value).expect("finite"),
            })
        };
        let bounded = scalar(AnswerSpecification::Number(NumberAnswerSpecification {
            minimum: Some(FiniteNumber::try_new(-1.5).expect("finite")),
            maximum: Some(FiniteNumber::try_new(2.25).expect("finite")),
        }));

        // Exact bounds, ordinary fractional values, and negatives.
        for inside in [-1.5, -0.75, 0.0, 1.0, 2.25] {
            let accepted = normalize_questionnaire_response(&bounded, &only(number(inside)))
                .unwrap_or_else(|error| panic!("{inside} must be in range: {error}"));
            // Requirement 6: the accepted answer is the same semantic value,
            // never a re-derived one.
            assert_eq!(settled(&accepted), number(inside));
        }
        for outside in [
            -1.500_000_000_000_000_2,
            2.250_000_000_000_000_4,
            -100.0,
            100.0,
        ] {
            assert!(
                normalize_questionnaire_response(&bounded, &only(number(outside))).is_err(),
                "{outside} is outside the declared range"
            );
        }
        // Only the declared side is bounded when only one bound is declared.
        let at_least = scalar(AnswerSpecification::Number(NumberAnswerSpecification {
            minimum: Some(FiniteNumber::try_new(0.0).expect("finite")),
            maximum: None,
        }));
        assert!(normalize_questionnaire_response(&at_least, &only(number(f64::MAX))).is_ok());
        assert!(normalize_questionnaire_response(&at_least, &only(number(-0.000_1))).is_err());

        // A non-finite value is unconstructible, so it can never be answered.
        assert!(FiniteNumber::try_new(f64::NAN).is_err());
        assert!(FiniteNumber::try_new(f64::INFINITY).is_err());
        assert!(FiniteNumber::try_new(f64::NEG_INFINITY).is_err());
        // An inverted interval is refused at the specification, before publication.
        assert!(
            validate_questionnaire(&scalar(AnswerSpecification::Number(
                NumberAnswerSpecification {
                    minimum: Some(FiniteNumber::try_new(1.0).expect("finite")),
                    maximum: Some(FiniteNumber::try_new(0.0).expect("finite")),
                }
            )))
            .is_err()
        );
    }

    /// The precision regression this whole domain exists for.
    ///
    /// With an arbitrary-precision answer and an `f64` comparison,
    /// `9007199254740993` would pass a `maximum` of `9007199254740992` and
    /// then be emitted unchanged. Here it is refused at the wire, so there is
    /// no value that rustX can validate and then emit differently.
    #[test]
    fn a_number_binary64_cannot_hold_exactly_is_refused_not_rounded() {
        let decode = |json: &str| -> Result<QuestionnaireAnswer, String> {
            serde_json::from_str::<QuestionnaireAnswer>(json).map_err(|error| error.to_string())
        };
        let answer = |number: &str| format!(r#"{{"type":"number","value":{{"value":{number}}}}}"#);

        // `2^53 - 1` and `2^53` are exact binary64 integers, so they decode.
        for exact in [TWO_POW_53_MINUS_1, TWO_POW_53] {
            let decoded = decode(&answer(exact)).unwrap_or_else(|error| panic!("{exact}: {error}"));
            let QuestionnaireAnswer::Number(number) = decoded else {
                panic!("expected a number answer");
            };
            assert_eq!(number.value.get().to_string(), exact);
        }
        // `2^53 + 1` is not, and is refused rather than silently becoming `2^53`.
        let refused = decode(&answer(TWO_POW_53_PLUS_1)).expect_err("must be refused");
        assert!(refused.contains("exactly representable"), "{refused}");
        // The same holds at the negative frontier and at the i64/u64 extremes,
        // whose exactness is decided by the value itself and not by its width:
        // `-2^63` is a power of two and survives, `2^63 - 1` and `2^64 - 1` do
        // not.
        for inexact in [
            "-9007199254740993",
            "9223372036854775807",
            "18446744073709551615",
        ] {
            assert!(
                decode(&answer(inexact)).is_err(),
                "{inexact} is not an exact binary64"
            );
        }
        assert!(
            decode(&answer("-9223372036854775808")).is_ok(),
            "-2^63 is an exact binary64 and is admitted on its own merit"
        );

        // A whole number cannot smuggle itself past the frontier by wearing a
        // fractional spelling either: the JSON parser has already rounded it,
        // so rustX cannot prove which decimal was written and refuses the
        // whole class rather than assume one.
        for spelled_as_fraction in [
            "9007199254740993.0",
            "9007199254740992.0",
            "-9007199254740993.0",
            "9.007199254740993e15",
        ] {
            assert!(
                decode(&answer(spelled_as_fraction)).is_err(),
                "{spelled_as_fraction} is a whole number at or past the frontier"
            );
        }
        // The rule is scoped to whole numbers a JSON integer can spell, which
        // is the form this domain serializes them in. Past that range no exact
        // integer spelling exists at all, so refusing these would make values
        // the domain can legitimately hold — an MCP `maximum` of `1e300`, say —
        // impossible to read back from the wire they were written to.
        for beyond_any_integer_spelling in ["1e300", "-1e300", "-1e19"] {
            assert!(
                decode(&answer(beyond_any_integer_spelling)).is_ok(),
                "{beyond_any_integer_spelling} has no exact integer spelling to check"
            );
        }
        // Whole numbers below the frontier and ordinary fractions are
        // unaffected, whichever way they are spelled.
        for ordinary in [
            "0",
            "0.0",
            "-1.5",
            "9007199254740991",
            "9007199254740991.0",
            "1.5e3",
        ] {
            assert!(decode(&answer(ordinary)).is_ok(), "{ordinary}");
        }

        // And the end-to-end statement: no answer passes a `2^53` maximum and
        // then settles as a numerically different value.
        let capped = scalar(AnswerSpecification::Number(NumberAnswerSpecification {
            minimum: None,
            maximum: Some(
                FiniteNumber::from_json_number(
                    &TWO_POW_53.parse::<serde_json::Number>().expect("number"),
                )
                .expect("exact"),
            ),
        }));
        let at_cap = decode(&answer(TWO_POW_53)).expect("exact");
        let accepted = normalize_questionnaire_response(&capped, &only(at_cap.clone()))
            .expect("2^53 is at the declared maximum");
        assert_eq!(settled(&accepted), at_cap);
        // The value that used to slip through cannot even be spelled as an
        // answer, so it cannot reach the range check at all.
        assert!(decode(&answer(TWO_POW_53_PLUS_1)).is_err());
    }

    /// Every value the canonical `Number` domain can hold must survive its own
    /// wire form. Serialization and parsing are inverses, so an accepted
    /// answer or a declared bound can always be read back — from the Event
    /// Journal, from a reconnecting client, from anywhere.
    #[test]
    fn every_representable_number_survives_its_own_wire_form() {
        for value in [
            0.0_f64,
            -0.0,
            1.0,
            -1.0,
            0.1,
            -2.5,
            1.0e-300,
            // Whole numbers a JSON integer can spell, on both sides of the
            // precision frontier and at the spellable extremes.
            9_007_199_254_740_991.0,
            9_007_199_254_740_992.0,
            -9_007_199_254_740_992.0,
            9_223_372_036_854_775_808.0,
            -9_223_372_036_854_775_808.0,
            // Whole numbers no JSON integer can spell: the frontier rule must
            // not make these unreadable, or a legal schema bound could be
            // stored and never loaded again.
            1.0e300,
            -1.0e300,
            -1.0e19,
            f64::MAX,
            f64::MIN,
        ] {
            let subject = FiniteNumber::try_new(value).expect("finite");
            let encoded = serde_json::to_string(&subject).expect("encodes");
            let decoded: FiniteNumber = serde_json::from_str(&encoded)
                .unwrap_or_else(|error| panic!("{value} encoded as {encoded}: {error}"));
            assert_eq!(decoded, subject, "{value} encoded as {encoded}");
            // And the wire form is stable, so a re-encoded value is byte-equal.
            assert_eq!(serde_json::to_string(&decoded).expect("encodes"), encoded);
        }
    }

    #[test]
    fn an_integer_crosses_the_wire_as_exact_decimal_text() {
        let integer = |value: i64| {
            QuestionnaireAnswer::Integer(IntegerAnswer {
                value: ExactInteger::new(value),
            })
        };
        // The Runtime Client representation is decimal text, so a whole number
        // above the JavaScript safe-integer range round-trips exactly.
        for value in [
            0_i64,
            42,
            -5,
            9_007_199_254_740_993,
            -9_007_199_254_740_993,
            i64::MIN,
            i64::MAX,
        ] {
            let encoded = serde_json::to_value(integer(value)).expect("encodes");
            assert_eq!(
                encoded["value"]["value"],
                serde_json::Value::String(value.to_string()),
                "the wire form is canonical decimal text"
            );
            let decoded: QuestionnaireAnswer =
                serde_json::from_value(encoded).expect("round-trips");
            assert_eq!(decoded, integer(value), "{value} survived the wire exactly");
        }

        // The full i64 domain is answerable when the question declares it.
        let widest = scalar(AnswerSpecification::Integer(IntegerAnswerSpecification {
            minimum: Some(ExactInteger::new(i64::MIN)),
            maximum: Some(ExactInteger::new(i64::MAX)),
        }));
        for value in [i64::MIN, -1, 0, 9_007_199_254_740_993, i64::MAX] {
            assert!(normalize_questionnaire_response(&widest, &only(integer(value))).is_ok());
        }

        // Exact bound comparison outside the JavaScript safe range: one step
        // out on either side is refused, with no rounding to blur the edge.
        let frontier = scalar(AnswerSpecification::Integer(IntegerAnswerSpecification {
            minimum: Some(ExactInteger::new(9_007_199_254_740_992)),
            maximum: Some(ExactInteger::new(9_007_199_254_740_993)),
        }));
        for inside in [9_007_199_254_740_992_i64, 9_007_199_254_740_993] {
            assert!(normalize_questionnaire_response(&frontier, &only(integer(inside))).is_ok());
        }
        for outside in [9_007_199_254_740_991_i64, 9_007_199_254_740_994] {
            assert!(
                normalize_questionnaire_response(&frontier, &only(integer(outside))).is_err(),
                "{outside}"
            );
        }
    }

    #[test]
    fn the_one_authoritative_integer_parse_refuses_non_decimal_syntax() {
        for accepted in [
            "0",
            "42",
            "-5",
            TWO_POW_53_PLUS_1,
            "-9223372036854775808",
            "9223372036854775807",
        ] {
            let parsed =
                ExactInteger::parse(accepted).unwrap_or_else(|error| panic!("{accepted}: {error}"));
            assert_eq!(parsed.to_string(), accepted);
        }
        // Fractional, exponential, signed-plus, malformed, and empty syntax.
        for refused in [
            "1.5",
            "1e3",
            "1E3",
            "NaN",
            "Infinity",
            "+",
            "-",
            "+1",
            "12abc",
            "",
            " 1",
            "1 ",
            "0x10",
            "1_000",
            "１２３",
        ] {
            assert!(ExactInteger::parse(refused).is_err(), "{refused:?}");
        }
        // Overflow past the 64-bit domain is refused, not truncated.
        for overflow in [
            "9223372036854775808",
            "-9223372036854775809",
            "1000000000000000000000",
        ] {
            let error = ExactInteger::parse(overflow).expect_err("overflow");
            assert!(error.contains("64-bit"), "{error}");
        }
        // The wire refuses the same syntax, and refuses a JSON number outright:
        // an integer never crosses this protocol as a JavaScript number.
        assert!(
            serde_json::from_str::<QuestionnaireAnswer>(
                r#"{"type":"integer","value":{"value":3}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<QuestionnaireAnswer>(
                r#"{"type":"integer","value":{"value":"1.5"}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn an_explicit_empty_text_answer_is_not_an_omitted_answer() {
        let text = |min_length: Option<u32>| {
            scalar(AnswerSpecification::Text(TextAnswerSpecification {
                min_length,
                max_length: None,
                format: None,
            }))
        };
        let empty = QuestionnaireAnswer::Text(TextAnswer {
            value: String::new(),
        });

        // An explicit empty string is a legal answer with no minimum, and with
        // an explicit minimum of zero.
        for permitted in [None, Some(0)] {
            let question = text(permitted);
            let accepted = normalize_questionnaire_response(&question, &only(empty.clone()))
                .unwrap_or_else(|error| panic!("{permitted:?}: {error}"));
            assert_eq!(
                settled(&accepted),
                empty,
                "the empty string survives intact"
            );
            // And it is a *submission*, not a decline: the two are distinct
            // terminal responses and an answered question is not an absence.
            assert!(matches!(accepted, QuestionnaireResponse::Submitted(_)));
        }

        // A positive minimum refuses it, exactly as it refuses any short answer.
        assert!(normalize_questionnaire_response(&text(Some(1)), &only(empty.clone())).is_err());

        // Omission is the other fact, and it is not the empty string: the
        // submission carries no entry for the question at all, which
        // normalizes to the explicit decline response.
        let omitted = normalize_questionnaire_response(
            &text(None),
            &QuestionnaireResponse::Submitted(QuestionnaireSubmission { answers: vec![] }),
        )
        .expect("an empty submission is legal");
        assert_eq!(omitted, QuestionnaireResponse::Declined);
        assert_ne!(
            omitted,
            normalize_questionnaire_response(&text(None), &only(empty)).expect("legal")
        );
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
