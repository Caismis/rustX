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
//!          Carried over the Runtime Client protocol as the value's own
//!          IEEE-754 bit pattern in canonical hexadecimal text, because a
//!          JSON number cannot carry binary64 identity: a JavaScript client
//!          stringifies the exact binary64 `2^63` as `9223372036854776000`,
//!          a different integer. One authoritative parse:
//!          [`FiniteNumber::from_wire`].
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
/// and a JavaScript `number` is a binary64 as well. Binary64 is therefore not
/// a convenience: it is the widest value *every* stage of the pipeline can
/// hold without rounding, so it is the one domain all of them share.
///
/// ```text
/// MCP NumberSchema bound (f64)
///   -> NumberAnswerSpecification bound (FiniteNumber)
///   -> Runtime Client wire: canonical binary64 text  "43e0000000000000"
///   -> client `number`, reconstructed exactly        (binary64)
///   -> NumberAnswer value          (FiniteNumber)
///   -> authoritative range comparison (FiniteNumber)
///   -> MCP accept.content JSON number (the same FiniteNumber)
/// ```
///
/// No stage holds a wider value than the next one, so there is no widening or
/// narrowing to hide a mismatch in.
///
/// # Why the wire is not a JSON number
///
/// The three things this pipeline keeps apart are easy to conflate:
///
/// ```text
/// human decimal spelling      client-local presentation ("1.5e3")
///   -> finite binary64        the semantic value        (1500.0)
///   -> canonical wire text    an exact encoding of the *value*
/// ```
///
/// The wire carries the **value**, never the spelling. A JSON number cannot
/// do that job, because a JavaScript client serializes a `number` through
/// `JSON.stringify`, which prints the shortest decimal that round-trips —
/// not the exact integer the binary64 denotes. The exact binary64 `2^63` is
/// the mathematical integer
///
/// ```text
/// 9223372036854775808
/// ```
///
/// and `JSON.stringify` emits
///
/// ```text
/// 9223372036854776000
/// ```
///
/// Those are different integers. They happen to parse back to the same
/// binary64, but any reader that treats a JSON integer as an exact decimal
/// integer — as an earlier revision of this type did — sees a value it must
/// refuse, and a question whose only legal answer is `2^63` becomes
/// publishable and unanswerable. Binary64 identity must therefore not depend
/// on a JSON number's decimal spelling at all.
///
/// # The canonical wire form
///
/// A [`FiniteNumber`] crosses the Runtime Client protocol, and is stored in
/// the Event Journal, as its IEEE-754 binary64 bit pattern written as exactly
/// [`FINITE_NUMBER_WIRE_CHARS`] lowercase hexadecimal digits, most significant
/// first:
///
/// ```text
/// 2^63   -> "43e0000000000000"
/// -2^63  -> "c3e0000000000000"
/// 0.1    -> "3fb999999999999a"
/// ```
///
/// The properties that matters are that this is *exact* and *canonical*:
///
/// - **exact** — the encoding is the value's own bits, so
///   `from_wire(to_wire(x)) == x` for every finite binary64 with no decimal
///   parser anywhere in the trust path, and no rounding step to disagree
///   about;
/// - **canonical** — one semantic value has exactly one legal spelling, in
///   both languages, byte for byte. A shortest-round-trip *decimal* string is
///   deterministic within one language but Rust and JavaScript do not format
///   it identically, so it could not carry the "one settled spelling per
///   value" property [`ExactInteger`] already holds rustX to;
/// - **bounded** — always 16 bytes, with no 700-digit expansion for a
///   subnormal and no locale, grouping, or exponent-notation variation;
/// - **closed over the domain** — the wire alphabet *is* the domain. A
///   decimal a binary64 cannot hold, `9007199254740993`, has no wire
///   representation at all rather than one the runtime must detect and
///   refuse. A client's own refusal of such a spelling is a statement of the
///   same fact for the human, not the enforcement of it.
///
/// The representation is internal to the Runtime Client protocol and the
/// durable audit. It is never shown to a human — a client edits and displays
/// ordinary decimals — and it is never what reaches an MCP server, which
/// receives the ordinary JSON number built from the same bits by
/// [`FiniteNumber::to_json_number`].
///
/// # Negative zero
///
/// rustX **canonicalizes** `-0.0` to `+0.0`. IEEE-754 gives the two distinct
/// bit patterns, but every comparison this type takes part in — [`Eq`],
/// [`PartialOrd`], and the authoritative range check — already treats them as
/// one value, so admitting two bit patterns would give one semantic value two
/// canonical wire spellings and make the encoding non-injective. The
/// normalization happens at construction, in [`FiniteNumber::try_new`], so
/// there is no stage at which a `-0.0` exists to be serialized, and
/// `"8000000000000000"` is refused on the wire as a non-canonical spelling of
/// `0.0` exactly as [`ExactInteger`] would refuse `"-0"`.
///
/// NaN and infinity are unrepresentable by construction, which is what makes
/// [`Eq`] sound here: every value this type can hold is reflexive.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct FiniteNumber(f64);

impl Eq for FiniteNumber {}

/// The exact width of the canonical [`FiniteNumber`] wire spelling.
///
/// A binary64 is 64 bits, which is 16 hexadecimal digits. The width is fixed
/// rather than minimal so that one value has one spelling: a leading zero is
/// significant here, not decoration.
pub const FINITE_NUMBER_WIRE_CHARS: usize = 16;

/// The bit pattern of `-0.0`, which no [`FiniteNumber`] ever holds.
const NEGATIVE_ZERO_BITS: u64 = 0x8000_0000_0000_0000;

impl FiniteNumber {
    /// The finite binary64 value, with `-0.0` canonicalized to `+0.0`.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is NaN or infinite.
    pub fn try_new(value: f64) -> Result<Self, String> {
        if !value.is_finite() {
            return Err("the numeric value is not finite".to_owned());
        }
        // Decided on the bits rather than with `value == 0.0`, because the
        // float comparison this type exists to discipline is exactly the one
        // that cannot tell the two zeros apart.
        if value.to_bits() == NEGATIVE_ZERO_BITS {
            return Ok(Self(0.0));
        }
        Ok(Self(value))
    }

    /// The underlying binary64 value, which is the comparison domain.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }

    /// The canonical Runtime Client spelling of this value.
    ///
    /// Exactly [`FINITE_NUMBER_WIRE_CHARS`] lowercase hexadecimal digits of
    /// the IEEE-754 bit pattern. See the type documentation for why the wire
    /// is not a JSON number.
    #[must_use]
    pub fn to_wire(self) -> String {
        format!(
            "{:0width$x}",
            self.0.to_bits(),
            width = FINITE_NUMBER_WIRE_CHARS
        )
    }

    /// Parses the canonical Runtime Client spelling of a finite binary64.
    ///
    /// This is the **one authoritative parse point** for a `Number` value:
    /// every bound and every answer arrives through it. The accepted syntax is
    /// exactly [`FINITE_NUMBER_WIRE_CHARS`] lowercase hexadecimal digits and
    /// nothing else — no `0x` prefix, no uppercase, no shortened form — so one
    /// value has one spelling and a re-encoded value is byte-identical.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not the canonical width or alphabet,
    /// when the bits name NaN or an infinity, or when they name the
    /// non-canonical `-0.0` this domain normalizes away.
    pub fn from_wire(text: &str) -> Result<Self, String> {
        let malformed = || {
            format!(
                "{text:?} is not a canonical binary64 value: write exactly \
                 {FINITE_NUMBER_WIRE_CHARS} lowercase hexadecimal digits of its \
                 IEEE-754 bit pattern"
            )
        };
        if text.len() != FINITE_NUMBER_WIRE_CHARS
            || !text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(malformed());
        }
        let bits = u64::from_str_radix(text, 16).map_err(|_| malformed())?;
        if bits == NEGATIVE_ZERO_BITS {
            return Err(format!(
                "{text:?} spells negative zero, which this domain canonicalizes to \
                 {:?}",
                Self(0.0).to_wire()
            ));
        }
        let value = f64::from_bits(bits);
        if !value.is_finite() {
            return Err(format!(
                "{text:?} does not name a finite 64-bit binary floating-point number"
            ));
        }
        Ok(Self(value))
    }

    /// The exact JSON number for this value.
    ///
    /// This is the **provider-facing** conversion — an MCP `accept.content`
    /// value is an ordinary JSON number — and never the Runtime Client wire.
    ///
    /// [`serde_json::Number::from_f64`] returns `None` only for NaN and
    /// infinity, neither of which this type can hold, so the conversion is
    /// total in practice. The signature stays honest instead of panicking on
    /// a branch that cannot be reached.
    #[must_use]
    pub fn to_json_number(self) -> Option<serde_json::Number> {
        serde_json::Number::from_f64(self.0)
    }
}

impl std::fmt::Display for FiniteNumber {
    /// The ordinary decimal presentation of the value, which is what a human
    /// reads. This is never the wire form: see [`FiniteNumber::to_wire`].
    ///
    /// A **whole** number prints as its exact integer. The default `f64`
    /// display prints the shortest decimal that *round-trips*, which for a
    /// large whole number is a different integer — `2^63` prints as
    /// `9223372036854776000` — and a reader told that a bound is
    /// `9223372036854776000` would be told a value binary64 does not hold.
    /// A fractional value keeps the shortest round-tripping spelling, which
    /// denotes the same binary64 and is what a reader expects to see.
    #[allow(
        clippy::float_cmp,
        reason = "an exact whole-number test is the intent here, not a tolerance"
    )]
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.fract() == 0.0 {
            return write!(formatter, "{:.0}", self.0);
        }
        write!(formatter, "{}", self.0)
    }
}

impl Serialize for FiniteNumber {
    /// Serializes the canonical binary64 text, never a JSON number: a JSON
    /// number would reintroduce the decimal-spelling ambiguity documented on
    /// the type.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_wire())
    }
}

impl<'de> Deserialize<'de> for FiniteNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Self::from_wire(&text).map_err(serde::de::Error::custom)
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
///
/// The bounds are [`FiniteNumber`]s, so they cross the Runtime Client protocol
/// in the same canonical binary64 text an answer does. Bound domain, answer
/// domain, and comparison domain are one domain with one wire encoding: a
/// client that can hold a bound can always spell an answer at it.
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

/// A finite numeric answer, carried as canonical binary64 **text**.
///
/// The value is a [`FiniteNumber`], the same domain the question's bounds and
/// the emitted MCP content use, so what the runtime validates is bit-identical
/// to what it sends — and the wire encoding is the value's own bits, so it is
/// bit-identical to what the client selected too. See [`FiniteNumber`] for why
/// a JSON number could not carry that identity.
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
    /// then be emitted unchanged. The canonical wire closes that class by
    /// construction: the wire alphabet **is** the domain, so a decimal
    /// binary64 cannot hold has no wire representation to arrive in.
    #[test]
    fn a_number_binary64_cannot_hold_exactly_has_no_wire_representation() {
        let decode = |json: &str| -> Result<QuestionnaireAnswer, String> {
            serde_json::from_str::<QuestionnaireAnswer>(json).map_err(|error| error.to_string())
        };
        let answer = |wire: &str| format!(r#"{{"type":"number","value":{{"value":"{wire}"}}}}"#);
        let wire_of = |decimal: &str| {
            FiniteNumber::try_new(decimal.parse::<f64>().expect("a decimal")).expect("finite")
        };

        // `2^53 - 1` and `2^53` are exact binary64 integers, so they decode to
        // themselves and present as the very decimals they were written as.
        for exact in [TWO_POW_53_MINUS_1, TWO_POW_53] {
            let decoded = decode(&answer(&wire_of(exact).to_wire()))
                .unwrap_or_else(|error| panic!("{exact}: {error}"));
            let QuestionnaireAnswer::Number(number) = decoded else {
                panic!("expected a number answer");
            };
            assert_eq!(number.value.to_string(), exact);
        }

        // `2^53 + 1` is not an exact binary64. There is no wire spelling that
        // names it: the nearest binary64 is `2^53`, whose canonical spelling
        // is `2^53`'s and not this one, so the value cannot arrive at all —
        // neither rounded into range nor as an integer to be refused.
        let plus_one = wire_of(TWO_POW_53_PLUS_1);
        assert_eq!(plus_one.to_wire(), wire_of(TWO_POW_53).to_wire());
        assert_eq!(plus_one.to_string(), TWO_POW_53);

        // The wire is not a JSON number, and a JSON number is not accepted as
        // one: the ambiguity has no way back in through a lenient reader.
        for json_number in [
            r#"{"type":"number","value":{"value":9007199254740992}}"#,
            r#"{"type":"number","value":{"value":9007199254740993}}"#,
            r#"{"type":"number","value":{"value":1.5}}"#,
        ] {
            assert!(decode(json_number).is_err(), "{json_number}");
        }

        // And the end-to-end statement: no answer passes a `2^53` maximum and
        // then settles as a numerically different value.
        let capped = scalar(AnswerSpecification::Number(NumberAnswerSpecification {
            minimum: None,
            maximum: Some(wire_of(TWO_POW_53)),
        }));
        let at_cap = decode(&answer(&wire_of(TWO_POW_53).to_wire())).expect("exact");
        let accepted = normalize_questionnaire_response(&capped, &only(at_cap.clone()))
            .expect("2^53 is at the declared maximum");
        assert_eq!(settled(&accepted), at_cap);
    }

    /// The invariant the Runtime Client transport rests on:
    ///
    /// ```text
    /// decode(encode(x)) == x
    /// ```
    ///
    /// for every supported `FiniteNumber`, with no dependence on any language
    /// choosing a particular decimal spelling for the value. Serialization and
    /// parsing are inverses, so an accepted answer or a declared bound can
    /// always be read back — from the Event Journal, from a reconnecting
    /// client, from anywhere.
    #[test]
    fn every_representable_number_survives_its_own_wire_form() {
        // The table below spells its values as decimals, which is convenient
        // to read and is exactly the kind of thing this domain refuses to
        // trust. Pin the ones whose identity the decimal cannot make obvious,
        // so a mistyped literal is a failure rather than a weaker test.
        for (label, expected) in [
            ("2^53", "4340000000000000"),
            ("2^53 + 2", "4340000000000001"),
            ("2^54", "4350000000000000"),
            ("2^63", "43e0000000000000"),
            ("-2^63", "c3e0000000000000"),
            ("2^1000", "7e70000000000000"),
            ("f64::MAX", "7fefffffffffffff"),
            ("f64::MIN_POSITIVE subnormal", "0000000000000001"),
        ] {
            let (_, value) = REPRESENTATIVE_NUMBERS
                .into_iter()
                .find(|(name, _)| *name == label)
                .unwrap_or_else(|| panic!("{label} is a representative value"));
            assert_eq!(
                FiniteNumber::try_new(value).expect("finite").to_wire(),
                expected,
                "{label} is not the value its decimal spelling claims"
            );
        }

        for (label, value) in REPRESENTATIVE_NUMBERS {
            let subject = FiniteNumber::try_new(value).expect("finite");
            let encoded = serde_json::to_string(&subject).expect("encodes");
            let decoded: FiniteNumber = serde_json::from_str(&encoded)
                .unwrap_or_else(|error| panic!("{label} encoded as {encoded}: {error}"));
            assert_eq!(decoded, subject, "{label} encoded as {encoded}");
            // Bit equality, not just `==`: the two zeros compare equal, so a
            // canonicalization failure has to be caught on the bits.
            assert_eq!(
                decoded.get().to_bits(),
                subject.get().to_bits(),
                "{label} must round-trip bit for bit"
            );
            // The spelling is settled, so a re-encoded value is byte-equal...
            assert_eq!(serde_json::to_string(&decoded).expect("encodes"), encoded);
            // ...and it is the canonical text form, never a JSON number.
            assert_eq!(encoded, format!("\"{}\"", subject.to_wire()), "{label}");
            assert_eq!(subject.to_wire().len(), FINITE_NUMBER_WIRE_CHARS, "{label}");
        }
    }

    /// The representative `FiniteNumber` values every layer is proven against:
    /// the zeros, small magnitudes, ordinary fractions, both sides of the
    /// `2^53` precision frontier, `±2^63`, and the extremes of the finite
    /// range. The TypeScript client asserts the same property over the same
    /// values in `tui/test/number-wire.test.ts`.
    const REPRESENTATIVE_NUMBERS: [(&str, f64); 18] = [
        ("0", 0.0),
        ("-0", -0.0),
        ("1", 1.0),
        ("-1", -1.0),
        ("0.1", 0.1),
        ("1.5", 1.5),
        ("-2.75", -2.75),
        ("1e-300", 1.0e-300),
        ("2^53 - 1", 9_007_199_254_740_991.0),
        ("2^53", 9_007_199_254_740_992.0),
        ("2^53 + 2", 9_007_199_254_740_994.0),
        ("2^54", 18_014_398_509_481_984.0),
        ("2^63", 9_223_372_036_854_775_808.0),
        ("-2^63", -9_223_372_036_854_775_808.0),
        ("2^1000", 1.071_508_607_186_267_3e301),
        ("f64::MAX", f64::MAX),
        ("-f64::MAX", f64::MIN),
        ("f64::MIN_POSITIVE subnormal", 5.0e-324),
    ];

    /// The concrete cross-language failure the canonical wire exists to close.
    ///
    /// `2^63` is an exact binary64 — it is a power of two — but a JavaScript
    /// client's `JSON.stringify` prints the shortest decimal that *round-trips*
    /// it, which is a different mathematical integer. Binary64 identity must
    /// therefore not travel as a JSON number.
    #[test]
    fn the_canonical_wire_pins_the_value_a_json_number_could_not_carry() {
        let two_pow_63 = FiniteNumber::try_new(9_223_372_036_854_775_808.0).expect("finite");
        assert_eq!(two_pow_63.to_wire(), "43e0000000000000");
        assert_eq!(FiniteNumber::from_wire("43e0000000000000"), Ok(two_pow_63));
        // Stated as the reviewer's invariant, on the value itself:
        assert_eq!(
            FiniteNumber::from_wire(&two_pow_63.to_wire()).expect("round trip"),
            two_pow_63
        );

        let negative = FiniteNumber::try_new(-9_223_372_036_854_775_808.0).expect("finite");
        assert_eq!(negative.to_wire(), "c3e0000000000000");
        assert_eq!(FiniteNumber::from_wire("c3e0000000000000"), Ok(negative));

        // The two decimals a reader could see for that one value. rustX shows
        // the exact one and never depends on either.
        assert_eq!(two_pow_63.to_string(), "9223372036854775808");
        let shortest_round_trip: f64 = "9223372036854776000".parse().expect("a decimal");
        #[allow(
            clippy::float_cmp,
            reason = "the point is that two different decimals name the same binary64"
        )]
        {
            assert_eq!(shortest_round_trip, two_pow_63.get());
        }
        assert_ne!("9223372036854776000", two_pow_63.to_string());
    }

    /// One semantic value, one legal spelling. A canonical wire that admitted
    /// a second spelling would not be canonical, and the durable audit would
    /// hold two byte forms of one fact.
    #[test]
    fn the_one_authoritative_number_parse_refuses_every_non_canonical_spelling() {
        for malformed in [
            "",
            "0",
            "43E0000000000000",   // uppercase is a second spelling of one value
            "0x43e0000000000000", // no prefix
            "43e000000000000",    // 15 digits
            "43e00000000000000",  // 17 digits
            " 43e0000000000000",
            "43e000000000000g",
            "9223372036854775808", // the decimal, not the wire form
        ] {
            let error = FiniteNumber::from_wire(malformed).expect_err(malformed);
            assert!(error.contains("canonical binary64 value"), "{error}");
        }
        for non_finite in [
            "7ff0000000000000", // +Infinity
            "fff0000000000000", // -Infinity
            "7ff8000000000000", // NaN
            "7fffffffffffffff", // a signalling NaN payload
        ] {
            let error = FiniteNumber::from_wire(non_finite).expect_err(non_finite);
            assert!(error.contains("finite"), "{error}");
        }
        assert!(FiniteNumber::try_new(f64::NAN).is_err());
        assert!(FiniteNumber::try_new(f64::INFINITY).is_err());
        assert!(FiniteNumber::try_new(f64::NEG_INFINITY).is_err());
    }

    /// rustX has one semantic zero.
    ///
    /// IEEE-754 gives `-0.0` its own bit pattern, but every comparison this
    /// domain takes part in already treats it as `0.0`, so admitting both
    /// patterns would give one value two canonical spellings. The
    /// normalization happens at construction, and the `-0.0` spelling is
    /// refused on the wire the way `ExactInteger` refuses `"-0"`.
    #[test]
    fn negative_zero_is_canonicalized_rather_than_carried_as_a_second_spelling() {
        let negative = FiniteNumber::try_new(-0.0).expect("finite");
        let positive = FiniteNumber::try_new(0.0).expect("finite");
        assert_eq!(negative, positive);
        assert_eq!(negative.get().to_bits(), positive.get().to_bits());
        assert!(!negative.get().is_sign_negative());
        assert_eq!(negative.to_wire(), "0000000000000000");
        assert_eq!(FiniteNumber::from_wire("0000000000000000"), Ok(positive));
        let error = FiniteNumber::from_wire("8000000000000000").expect_err("non-canonical");
        assert!(error.contains("negative zero"), "{error}");
        // And the provider-facing conversion agrees: the emitted JSON number
        // is `0.0`, so the MCP server never sees a sign this domain dropped.
        assert_eq!(
            negative.to_json_number().expect("finite"),
            positive.to_json_number().expect("finite")
        );
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
