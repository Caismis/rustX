//! The MCP multi-round-trip (MRTR, SEP-2322) protocol translation layer
//! (Issue #242).
//!
//! # One invocation, N rounds, one settlement
//!
//! MCP `2026-07-28` lets a server answer `tools/call` with an
//! [`InputRequiredResult`](rmcp::model::InputRequiredResult) instead of a
//! `CallToolResult`. That answer is an **intermediate state of the
//! already-admitted rustX `ToolInvocation`**, never a terminal `ToolResult`,
//! never a new invocation, and never a new `ToolExecutionId`:
//!
//! ```text
//! model `ToolCall` A
//!     |
//!     v
//! rustX `ToolInvocation` A ── approval evaluated once ──┐
//!     |                                               |
//!     +--> tools/call round 1 ---------------------+  |
//!     |        |                                   |  |
//!     |        +--> `CallToolResult` ----------------+--+--> ONE terminal
//!     |        |                                   |  |    `ToolExecutionResult`
//!     |        +--> `InputRequiredResult`            |  |
//!     |                 |                          |  |
//!     |                 v                          |  |
//!     |        one runtime-owned Questionnaire     |  |
//!     |        (`InteractionCoordinator`)            |  |
//!     |                 |                          |  |
//!     +--> tools/call round N+1 ------------------+   |
//! ```
//!
//! This module owns **only the translation**: what an `InputRequiredResult`
//! means, how an MCP elicitation schema becomes rustX's provider-independent
//! typed question vocabulary, and how a typed Questionnaire response becomes
//! the exact `inputResponses` map the next round must carry. The round driver,
//! the dispatch frontier, cancellation arbitration, and terminal settlement
//! stay in [`super`], where the existing MCP execution semantics already live.
//!
//! # A strict translation layer, not a schema interpreter
//!
//! The core invariant of this module is:
//!
//! > rustX never emits an MCP `accept` whose `content` has not been validated
//! > against every constraint of the original requested schema that rustX
//! > claims to support — and it never claims to support a schema shape whose
//! > constraints it would then discard.
//!
//! So each supported field of each rmcp schema type is either **preserved and
//! validated**, or its presence **refuses that schema instance**:
//!
//! ```text
//! StringSchema   type, title(header), description(prompt),
//!                minLength / maxLength      -> Text { min_length, max_length }
//!                format date|date-time|uri  -> Text { format }
//!                format email               -> REFUSED (no faithful validator)
//! NumberSchema   minimum / maximum (f64)   -> Number { minimum, maximum }
//!                                               as finite binary64, the
//!                                               identity translation
//! IntegerSchema  minimum / maximum (i64)   -> Integer { minimum, maximum }
//!                                               as exact i64, the identity
//!                                               translation
//! BooleanSchema  (no constraints)           -> Boolean
//! enum (single)  enum | oneOf | enumNames   -> SingleChoice { options,
//!                                                allow_custom: false }
//! enum (multi)   enum | anyOf,
//!                minItems / maxItems        -> MultiChoice { options,
//!                                                min_selected, max_selected,
//!                                                allow_custom: false }
//! ```
//!
//! `default` is deliberately **not** a constraint: it is an authoring hint,
//! and rustX does not pre-fill an answer on a human's behalf, so ignoring it
//! can never produce schema-invalid content. That decision is documented
//! rather than silent.
//!
//! A `title` becomes the question's tab header and a `description` is shown
//! with the prompt, so nothing the server wrote is thrown away unseen.
//!
//! # What is still refused, and why
//!
//! An `InputRequiredResult` may embed three request kinds (rmcp's
//! [`InputRequest`]): `sampling/createMessage`, `elicitation/create`, and
//! `roots/list`. rustX adopts **elicitation only**:
//!
//! - **Sampling** would let an MCP server initiate model execution, which
//!   belongs to the Agent Loop and the `ModelAdapter` alone;
//! - **Roots** would expose host/workspace authority through a deprecated MCP
//!   surface that duplicates rustX's explicit Workspace ownership.
//!
//! URL-mode elicitation is likewise refused — it directs a human to an
//! external site, which is not a bounded rustX Questionnaire.
//!
//! A round whose requests mix supported and unsupported kinds fails **before**
//! anything is published, so a user is never shown half a prompt that is about
//! to be abandoned.
//!
//! # Invalid human input is not an unsupported feature
//!
//! Those refusals are *invocation-level* deterministic failures: the server
//! asked for something rustX cannot represent. They are strictly separate from
//! a human typing `1.5` into an `integer` field, which is an interaction
//! response the runtime refuses while the interaction stays pending. Nothing
//! in this module can turn the second into the first.

use std::collections::BTreeSet;

use rmcp::model::{
    ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationSchema, EnumSchema,
    InputRequest, InputRequests, InputResponses, MultiSelectEnumSchema, PrimitiveSchemaDefinition,
    SingleSelectEnumSchema, StringFormat,
};

use crate::events::interaction::{
    AnswerSpecification, ExactInteger, FiniteNumber, IntegerAnswerSpecification,
    MAX_CHOICE_OPTIONS, MAX_OPTION_LABEL_CHARS, MAX_QUESTION_HEADER_CHARS, MAX_QUESTION_TEXT_CHARS,
    MAX_QUESTIONNAIRE_QUESTIONS, MAX_TEXT_ANSWER_CHARS, MIN_CHOICE_OPTIONS,
    MultiChoiceSpecification, NumberAnswerSpecification, OptionSpecification,
    QuestionSpecification, QuestionnaireAnswer, QuestionnaireResponse, QuestionnaireSpecification,
    SingleChoiceSpecification, TextAnswerSpecification, TextFormat,
};

/// The fixed rustX-owned bound on how many physical `tools/call` rounds one
/// `ToolInvocation` may perform.
///
/// **The initial call is round 1.** A server may therefore drive at most
/// `MCP_MRTR_MAX_ROUNDS - 1` `input_required` continuations before the
/// invocation fails deterministically. The value matches rmcp's own
/// `DEFAULT_MRTR_MAX_ROUNDS`, but the constant is rustX's: it is a runtime
/// safety bound on a server-driven loop, not a re-exported SDK default, and
/// it is deliberately not configurable.
pub(crate) const MCP_MRTR_MAX_ROUNDS: usize = 10;

/// The most input requests one `InputRequiredResult` may carry.
///
/// Every supported request contributes at least one question, and one
/// questionnaire carries at most [`MAX_QUESTIONNAIRE_QUESTIONS`] questions,
/// so a larger request map could never produce a publishable questionnaire.
/// Bounding it here rejects the payload before any translation work happens.
pub(super) const MCP_MRTR_MAX_INPUT_REQUESTS: usize = MAX_QUESTIONNAIRE_QUESTIONS;

/// The most bytes of opaque `requestState` rustX will retain and echo.
///
/// The value is protocol-owned and never parsed, so the only defensible
/// bound is a size bound: an unbounded blob would let a peer grow this
/// invocation's execution-local state without limit.
pub(super) const MCP_MRTR_MAX_REQUEST_STATE_BYTES: usize = 8 * 1024;

/// The most bytes of serialized `inputResponses` one continuation may carry.
pub(super) const MCP_MRTR_MAX_INPUT_RESPONSES_BYTES: usize = 16 * 1024;

/// The most bytes of `elicitation/create` request payload rustX will
/// translate, per round.
pub(super) const MCP_MRTR_MAX_INPUT_REQUESTS_BYTES: usize = 64 * 1024;

/// The opaque protocol continuation state of one in-flight MRTR invocation.
///
/// Both fields are **protocol-owned**: `request_state` is echoed byte for
/// byte and never parsed, and `input_responses` is built only from a typed
/// Questionnaire settlement. The value lives on the executor's stack for the
/// lifetime of one invocation and is dropped at terminal settlement; it never
/// enters canonical history, Goal/Workflow/Subagent durable state, or the
/// model-issued `ToolCall` arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct McpContinuation {
    /// The exact `requestState` string the previous round returned.
    pub(super) request_state: Option<String>,
    /// The client responses to the previous round's `inputRequests`.
    pub(super) input_responses: Option<InputResponses>,
}

impl McpContinuation {
    /// Whether this continuation carries nothing at all, which would make the
    /// next round byte-identical to the previous one.
    pub(super) fn is_empty(&self) -> bool {
        self.request_state.is_none() && self.input_responses.is_none()
    }
}

/// How one typed rustX answer becomes the exact JSON value the server's own
/// schema declared.
///
/// The choice variants hold the underlying protocol values **positionally**,
/// indexed exactly like the question's declared options. That is what makes a
/// display label pure presentation: a duplicated title, a client-reserved
/// word, or a forged string cannot select a different value, because the
/// response never carries one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PropertyValues {
    /// A JSON string.
    Text,
    /// A JSON number, integral or fractional.
    Number,
    /// A JSON number with no fractional part.
    Integer,
    /// A JSON boolean.
    Boolean,
    /// One of these values, addressed by option index.
    SingleSelect(Vec<serde_json::Value>),
    /// A JSON array of these values, addressed by option index.
    MultiSelect(Vec<serde_json::Value>),
}

/// One question rustX derived from one MCP elicitation property.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PropertySlot {
    /// Index into [`ElicitationPlan::requests`].
    request_index: usize,
    /// The MCP property name this question answers.
    property: String,
    /// Whether the elicitation schema marks the property required.
    required: bool,
    /// How a typed answer becomes this property's schema-valid JSON value.
    values: PropertyValues,
}

/// One `elicitation/create` request inside the round, in deterministic key
/// order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestPlan {
    /// The server-assigned `inputRequests` key. It is echoed verbatim as the
    /// `inputResponses` key, so the mapping is exact, not positional.
    key: String,
}

/// The complete, validated translation of one `InputRequiredResult`'s
/// elicitation requests.
///
/// Constructing a plan proves the whole payload is representable: nothing is
/// published until the plan exists, so a mixed supported/unsupported request
/// set never produces a partial prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ElicitationPlan {
    questionnaire: QuestionnaireSpecification,
    /// One entry per question, in question order.
    slots: Vec<PropertySlot>,
    requests: Vec<RequestPlan>,
}

impl ElicitationPlan {
    /// The bounded questionnaire to publish through the runtime-owned
    /// interaction coordinator.
    pub(super) fn questionnaire(&self) -> &QuestionnaireSpecification {
        &self.questionnaire
    }

    /// The number of `inputRequests` this plan answers.
    #[cfg(test)]
    pub(super) fn request_count(&self) -> usize {
        self.requests.len()
    }

    /// Builds the exact `inputResponses` map for one typed Questionnaire
    /// settlement.
    ///
    /// The response reaching this function has **already** been validated by
    /// [`normalize_questionnaire_submission`](crate::events::interaction::normalize_questionnaire_submission)
    /// against the very question specifications this plan published, which is
    /// the authoritative validation point for every declared bound: text
    /// length and format, numeric range, integrality, option membership, and
    /// multi-select cardinality. This function performs the remaining
    /// *mapping*, and treats a shape the runtime should already have refused
    /// as a bounded internal error rather than fabricating content.
    ///
    /// The mapping is total over the plan's requests: every key the server
    /// sent gets exactly one `ElicitResult`, because a server that asked N
    /// questions must be answered for all N or none of them is meaningful.
    ///
    /// - an explicit human **decline** answers every request with the
    ///   protocol's own `decline` action — the MCP-defined "the user refused
    ///   to provide this, continue anyway";
    /// - a request whose required properties were all answered is `accept`
    ///   with schema-conforming `content`;
    /// - a request with an unanswered required property is `decline`: rustX
    ///   will not send `accept` with content the server's own schema rejects.
    ///
    /// # Errors
    ///
    /// Returns a bounded diagnostic when an answer cannot be mapped to a
    /// schema-valid MCP value.
    pub(super) fn responses(
        &self,
        response: &QuestionnaireResponse,
    ) -> Result<InputResponses, String> {
        let submission = match response {
            QuestionnaireResponse::Declined => None,
            QuestionnaireResponse::Submitted(submission) => Some(submission),
        };
        // property answers per request index, and whether any required
        // property of that request is still unanswered.
        let mut content: Vec<serde_json::Map<String, serde_json::Value>> =
            vec![serde_json::Map::new(); self.requests.len()];
        let mut answered: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); self.requests.len()];
        if let Some(submission) = submission {
            for entry in &submission.answers {
                let slot = self.slots.get(entry.question_index).ok_or_else(|| {
                    format!(
                        "the questionnaire response answers unknown question index {}",
                        entry.question_index
                    )
                })?;
                let value = slot.value_of(&entry.answer)?;
                content[slot.request_index].insert(slot.property.clone(), value);
                answered[slot.request_index].insert(entry.question_index);
            }
        }
        let mut responses = InputResponses::new();
        for (index, request) in self.requests.iter().enumerate() {
            let missing_required = self.slots.iter().enumerate().any(|(question, slot)| {
                slot.request_index == index && slot.required && !answered[index].contains(&question)
            });
            let result = if submission.is_none() || missing_required || content[index].is_empty() {
                // `decline` is the protocol's own "no information, continue".
                // It is never used for provider absence or cancellation:
                // those never reach this function.
                ElicitResult::new(ElicitationAction::Decline)
            } else {
                ElicitResult::new(ElicitationAction::Accept)
                    .with_content(serde_json::Value::Object(content[index].clone()))
            };
            let encoded = serde_json::to_value(&result)
                .map_err(|error| format!("the elicitation response is unserializable: {error}"))?;
            responses.insert(request.key.clone(), encoded);
        }
        let size = serde_json::to_vec(&responses).map_or(usize::MAX, |bytes| bytes.len());
        if size > MCP_MRTR_MAX_INPUT_RESPONSES_BYTES {
            return Err(format!(
                "the elicitation responses are {size} bytes, above the \
                 {MCP_MRTR_MAX_INPUT_RESPONSES_BYTES}-byte MRTR bound"
            ));
        }
        Ok(responses)
    }
}

impl PropertySlot {
    /// The schema-valid MCP value for one typed answer to this question.
    fn value_of(&self, answer: &QuestionnaireAnswer) -> Result<serde_json::Value, String> {
        let mismatch = || {
            format!(
                "the answer kind does not match MCP elicitation property {:?}",
                self.property
            )
        };
        let out_of_range = || {
            format!(
                "the answer to MCP elicitation property {:?} is not one of its schema values",
                self.property
            )
        };
        match (&self.values, answer) {
            (PropertyValues::Text, QuestionnaireAnswer::Text(text)) => {
                Ok(serde_json::Value::String(text.value.clone()))
            }
            // The emitted number is built from the very `FiniteNumber` the
            // runtime range-checked, so "validated value" and "emitted value"
            // are the same bits, not two conversions of one decimal spelling.
            (PropertyValues::Number, QuestionnaireAnswer::Number(number)) => number
                .value
                .to_json_number()
                .map(serde_json::Value::Number)
                .ok_or_else(|| {
                    format!(
                        "the answer to MCP elicitation property {:?} is not a finite number",
                        self.property
                    )
                }),
            (PropertyValues::Integer, QuestionnaireAnswer::Integer(integer)) => {
                Ok(serde_json::Value::Number(integer.value.get().into()))
            }
            (PropertyValues::Boolean, QuestionnaireAnswer::Boolean(boolean)) => {
                Ok(serde_json::Value::Bool(boolean.value))
            }
            (PropertyValues::SingleSelect(values), QuestionnaireAnswer::Option(option)) => values
                .get(option.option_index)
                .cloned()
                .ok_or_else(out_of_range),
            (PropertyValues::MultiSelect(values), QuestionnaireAnswer::Options(options)) => {
                let mut selected = Vec::with_capacity(options.option_indices.len());
                for index in &options.option_indices {
                    selected.push(values.get(*index).cloned().ok_or_else(out_of_range)?);
                }
                Ok(serde_json::Value::Array(selected))
            }
            // A custom answer has no legal spelling for an MCP question: every
            // question this module publishes declares `allow_custom: false`,
            // so the runtime refuses one before it ever reaches this mapping.
            (_, QuestionnaireAnswer::Custom(_)) => Err(format!(
                "MCP elicitation property {:?} accepts only its declared schema values",
                self.property
            )),
            (_, _) => Err(mismatch()),
        }
    }
}

/// Why one `InputRequiredResult` cannot be served by rustX.
///
/// Every variant is a **deterministic terminal diagnostic** of the owning
/// invocation. None of them publishes an interaction, calls a model, or
/// discloses workspace state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct McpMrtrUnsupported {
    /// The bounded model-facing diagnostic.
    pub(super) diagnostic: String,
}

impl McpMrtrUnsupported {
    fn new(diagnostic: impl Into<String>) -> Self {
        Self {
            diagnostic: diagnostic.into(),
        }
    }
}

/// One MCP property translated into the provider-independent vocabulary.
struct TranslatedProperty {
    /// The declared legal answer shape.
    answer: AnswerSpecification,
    /// How a typed answer to that shape becomes a schema-valid JSON value.
    values: PropertyValues,
    /// The schema's own `title`, used as the question's tab header.
    title: Option<String>,
    /// The schema's own `description`, shown with the prompt.
    description: Option<String>,
}

/// Validates and translates one round's `inputRequests`.
///
/// Returns `Ok(None)` when the round carries no input requests at all — a
/// pure `requestState` continuation, which the MCP spec explicitly allows
/// (load shedding) and which rustX serves by re-dispatching without any
/// human interaction.
///
/// # Errors
///
/// Returns a bounded unsupported-feature diagnostic when any request in the
/// round is not representable. Validation is **whole-payload**: a mixed
/// supported/unsupported set fails here, before any interaction exists.
#[allow(
    clippy::too_many_lines,
    reason = "whole-payload validation and question construction are one contract"
)]
pub(super) fn plan_round(
    requests: Option<&InputRequests>,
) -> Result<Option<ElicitationPlan>, McpMrtrUnsupported> {
    let Some(requests) = requests.filter(|requests| !requests.is_empty()) else {
        return Ok(None);
    };
    if requests.len() > MCP_MRTR_MAX_INPUT_REQUESTS {
        return Err(McpMrtrUnsupported::new(format!(
            "the MCP input_required round carries {} input requests, above the \
             {MCP_MRTR_MAX_INPUT_REQUESTS} rustX bound",
            requests.len()
        )));
    }
    let encoded = serde_json::to_vec(requests).map_err(|error| {
        McpMrtrUnsupported::new(format!("the MCP input requests are unreadable: {error}"))
    })?;
    if encoded.len() > MCP_MRTR_MAX_INPUT_REQUESTS_BYTES {
        return Err(McpMrtrUnsupported::new(format!(
            "the MCP input_required payload is {} bytes, above the \
             {MCP_MRTR_MAX_INPUT_REQUESTS_BYTES}-byte rustX bound",
            encoded.len()
        )));
    }
    // Pass one: every request kind must be supported before any question is
    // built, so an unsupported kind can never produce a partial prompt.
    let mut forms = Vec::with_capacity(requests.len());
    for (key, request) in requests {
        match request {
            InputRequest::Elicitation(elicit) => match &elicit.params {
                ElicitRequestParams::FormElicitationParams {
                    message,
                    requested_schema,
                    ..
                } => forms.push((key.clone(), message.clone(), requested_schema.clone())),
                ElicitRequestParams::UrlElicitationParams { .. } => {
                    return Err(McpMrtrUnsupported::new(format!(
                        "MCP url-mode elicitation ({key:?}) is unsupported: rustX serves only \
                         bounded in-band form elicitation"
                    )));
                }
                _ => {
                    return Err(McpMrtrUnsupported::new(format!(
                        "MCP elicitation request {key:?} uses an unsupported mode"
                    )));
                }
            },
            InputRequest::CreateMessage(_) => {
                return Err(McpMrtrUnsupported::new(format!(
                    "MCP sampling (input request {key:?}) is unsupported: an MCP server may not \
                     initiate rustX model execution"
                )));
            }
            InputRequest::ListRoots(_) => {
                return Err(McpMrtrUnsupported::new(format!(
                    "MCP roots (input request {key:?}) is unsupported: rustX exposes workspace \
                     authority only through its own Workspace contract"
                )));
            }
            _ => {
                return Err(McpMrtrUnsupported::new(format!(
                    "MCP input request {key:?} uses an unknown request kind"
                )));
            }
        }
    }
    // Pass two: build the questionnaire. Question order is
    // (input-request key, schema property order), both deterministic.
    let qualify = forms.len() > 1;
    let mut questions = Vec::new();
    let mut slots = Vec::new();
    let mut plans = Vec::with_capacity(forms.len());
    for (request_index, (key, message, schema)) in forms.iter().enumerate() {
        plans.push(RequestPlan { key: key.clone() });
        let required: BTreeSet<&str> = schema
            .required
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(String::as_str)
            .collect();
        let properties = ordered_properties(schema);
        if properties.is_empty() {
            return Err(McpMrtrUnsupported::new(format!(
                "MCP elicitation request {key:?} declares no properties to ask about"
            )));
        }
        let single_property = !qualify && properties.len() == 1;
        for (property, definition) in properties {
            let translated = translate_property(key, property, definition)?;
            let question = question_text(
                message,
                key,
                property,
                translated.description.as_deref(),
                qualify,
                single_property,
            );
            if question.chars().count() > MAX_QUESTION_TEXT_CHARS {
                return Err(McpMrtrUnsupported::new(format!(
                    "MCP elicitation request {key:?} property {property:?} produces a prompt \
                     above the {MAX_QUESTION_TEXT_CHARS}-character bound"
                )));
            }
            let header = bounded_header(translated.title.as_deref().unwrap_or(property));
            questions.push(QuestionSpecification {
                question,
                header,
                answer: translated.answer,
            });
            slots.push(PropertySlot {
                request_index,
                property: property.to_owned(),
                required: required.contains(property),
                values: translated.values,
            });
            if questions.len() > MAX_QUESTIONNAIRE_QUESTIONS {
                return Err(McpMrtrUnsupported::new(format!(
                    "the MCP input_required round asks more than \
                     {MAX_QUESTIONNAIRE_QUESTIONS} questions, which rustX cannot present as one \
                     bounded questionnaire"
                )));
            }
        }
    }
    let questionnaire = QuestionnaireSpecification { questions };
    // The one shared bounded-questionnaire contract, applied here so an
    // unrepresentable schema becomes an MCP diagnostic rather than an opaque
    // interaction-publication failure. It is the same validator the
    // coordinator and the durable store apply, so a plan that survives it is
    // publishable and settleable by construction.
    crate::events::interaction::validate_questionnaire(&questionnaire).map_err(|error| {
        McpMrtrUnsupported::new(format!(
            "the MCP elicitation requests do not form a valid rustX questionnaire: {error}"
        ))
    })?;
    Ok(Some(ElicitationPlan {
        questionnaire,
        slots,
        requests: plans,
    }))
}

/// Validates one round's opaque `requestState` before it is retained.
///
/// # Errors
///
/// Returns a bounded diagnostic when the state exceeds the retention bound.
pub(super) fn validate_request_state(state: Option<&str>) -> Result<(), McpMrtrUnsupported> {
    let Some(state) = state else {
        return Ok(());
    };
    if state.len() > MCP_MRTR_MAX_REQUEST_STATE_BYTES {
        return Err(McpMrtrUnsupported::new(format!(
            "the MCP requestState is {} bytes, above the \
             {MCP_MRTR_MAX_REQUEST_STATE_BYTES}-byte rustX retention bound",
            state.len()
        )));
    }
    Ok(())
}

/// The schema's properties in its own declared wire order, falling back to
/// the sorted map order rmcp stores.
fn ordered_properties(schema: &ElicitationSchema) -> Vec<(&str, &PrimitiveSchemaDefinition)> {
    match &schema.property_order {
        Some(order) => order
            .iter()
            .filter_map(|name| {
                schema
                    .properties
                    .get_key_value(name)
                    .map(|(name, definition)| (name.as_str(), definition))
            })
            .collect(),
        None => schema
            .properties
            .iter()
            .map(|(name, definition)| (name.as_str(), definition))
            .collect(),
    }
}

/// The bounded prompt text of one derived question.
///
/// A single-property request keeps the server's own message verbatim. Any
/// other shape appends the property path, which is what keeps question texts
/// unique inside one questionnaire — `(input-request key, property name)` is
/// unique by construction, and `validate_questionnaire` requires uniqueness.
/// The schema's own `description`, when it has one, is shown between the two.
fn question_text(
    message: &str,
    key: &str,
    property: &str,
    description: Option<&str>,
    qualify: bool,
    single_property: bool,
) -> String {
    use std::fmt::Write as _;

    let mut text = message.to_owned();
    if let Some(description) = description.map(str::trim).filter(|value| !value.is_empty()) {
        text.push_str("\n\n");
        text.push_str(description);
    }
    if single_property {
        return text;
    }
    if qualify {
        let _ = write!(text, "\n\n[{key}.{property}]");
    } else {
        let _ = write!(text, "\n\n[{property}]");
    }
    text
}

/// The bounded question tab label derived from the schema title or property.
fn bounded_header(source: &str) -> String {
    let header: String = source.chars().take(MAX_QUESTION_HEADER_CHARS).collect();
    if header.trim().is_empty() {
        "field".to_owned()
    } else {
        header
    }
}

/// Translates one primitive elicitation property into the provider-independent
/// typed question vocabulary.
///
/// Every branch either carries each supported constraint of the rmcp schema
/// type into the [`AnswerSpecification`] — where the shared runtime validator
/// enforces it — or refuses that schema instance. No supported constraint is
/// accepted and then dropped.
#[allow(
    clippy::too_many_lines,
    reason = "the whole schema-to-vocabulary mapping is one auditable contract"
)]
fn translate_property(
    key: &str,
    property: &str,
    definition: &PrimitiveSchemaDefinition,
) -> Result<TranslatedProperty, McpMrtrUnsupported> {
    let unsupported = |detail: &str| {
        McpMrtrUnsupported::new(format!(
            "MCP elicitation request {key:?} property {property:?} is unsupported: {detail}"
        ))
    };
    match definition {
        PrimitiveSchemaDefinition::Boolean(schema) => Ok(TranslatedProperty {
            answer: AnswerSpecification::Boolean,
            values: PropertyValues::Boolean,
            title: schema.title.as_deref().map(str::to_owned),
            description: schema.description.as_deref().map(str::to_owned),
        }),
        PrimitiveSchemaDefinition::String(schema) => {
            let format = match schema.format {
                None => None,
                Some(StringFormat::Date) => Some(TextFormat::Date),
                Some(StringFormat::DateTime) => Some(TextFormat::DateTime),
                Some(StringFormat::Uri) => Some(TextFormat::Uri),
                // rustX has no deterministic, faithful email validator, and
                // will not claim to support a constraint it cannot enforce.
                Some(StringFormat::Email) => {
                    return Err(unsupported(
                        "rustX cannot faithfully validate the \"email\" string format",
                    ));
                }
                Some(_) => {
                    return Err(unsupported("it declares an unknown string format"));
                }
            };
            let min_length = schema.min_length;
            if let Some(min) = min_length
                && min as usize > MAX_TEXT_ANSWER_CHARS
            {
                return Err(unsupported(&format!(
                    "its minLength is {min}, above the {MAX_TEXT_ANSWER_CHARS}-character rustX \
                     text-answer bound, so no answer rustX can carry would satisfy it"
                )));
            }
            // rustX's own answer bound is *stricter* than an oversized
            // maxLength, so narrowing it keeps every answer schema-valid; the
            // server's constraint is still enforced, never discarded.
            let max_length = Some(match schema.max_length {
                Some(max) => max.min(u32::try_from(MAX_TEXT_ANSWER_CHARS).unwrap_or(u32::MAX)),
                None => u32::try_from(MAX_TEXT_ANSWER_CHARS).unwrap_or(u32::MAX),
            });
            if let (Some(min), Some(max)) = (min_length, max_length)
                && min > max
            {
                return Err(unsupported(
                    "its minLength exceeds its maxLength, so no answer could satisfy it",
                ));
            }
            Ok(TranslatedProperty {
                answer: AnswerSpecification::Text(TextAnswerSpecification {
                    min_length,
                    max_length,
                    format,
                }),
                values: PropertyValues::Text,
                title: schema.title.as_deref().map(str::to_owned),
                description: schema.description.as_deref().map(str::to_owned),
            })
        }
        PrimitiveSchemaDefinition::Number(schema) => {
            // `NumberSchema::minimum`/`maximum` are `f64`, which *is* the
            // canonical rustX `Number` domain, so the translation is the
            // identity on every bound MCP can express. The only unusable bound
            // is a non-finite one, which no answer could satisfy anyway.
            let bound = |value: Option<f64>, name: &str| match value {
                None => Ok(None),
                Some(value) => FiniteNumber::try_new(value)
                    .map(Some)
                    .map_err(|_| unsupported(&format!("its {name} is not a finite number"))),
            };
            let minimum = bound(schema.minimum, "minimum")?;
            let maximum = bound(schema.maximum, "maximum")?;
            if let (Some(min), Some(max)) = (minimum, maximum)
                && min.get() > max.get()
            {
                return Err(unsupported(
                    "its minimum exceeds its maximum, so no answer could satisfy it",
                ));
            }
            Ok(TranslatedProperty {
                answer: AnswerSpecification::Number(NumberAnswerSpecification { minimum, maximum }),
                values: PropertyValues::Number,
                title: schema.title.as_deref().map(str::to_owned),
                description: schema.description.as_deref().map(str::to_owned),
            })
        }
        PrimitiveSchemaDefinition::Integer(schema) => {
            // `IntegerSchema::minimum`/`maximum` are `i64`, which *is* the
            // canonical rustX `Integer` domain. No MCP integer schema can name
            // a bound rustX cannot carry, and because the Runtime Client
            // representation is decimal text rather than a JavaScript number,
            // no MCP integer schema can name a legal answer set that no client
            // could submit either. The "refuse a schema no client can satisfy"
            // rule is therefore vacuous here by construction — the only
            // unsatisfiable interval is an inverted one.
            let minimum = schema.minimum.map(ExactInteger::new);
            let maximum = schema.maximum.map(ExactInteger::new);
            if let (Some(min), Some(max)) = (minimum, maximum)
                && min.get() > max.get()
            {
                return Err(unsupported(
                    "its minimum exceeds its maximum, so no answer could satisfy it",
                ));
            }
            Ok(TranslatedProperty {
                answer: AnswerSpecification::Integer(IntegerAnswerSpecification {
                    minimum,
                    maximum,
                }),
                values: PropertyValues::Integer,
                title: schema.title.as_deref().map(str::to_owned),
                description: schema.description.as_deref().map(str::to_owned),
            })
        }
        PrimitiveSchemaDefinition::Enum(schema) => translate_enum(schema, &unsupported),
        _ => Err(unsupported("it declares an unknown primitive schema kind")),
    }
}

/// Translates one `enum` property, single- or multi-select, into a bounded
/// choice question whose options are addressed by index.
#[allow(
    clippy::too_many_lines,
    reason = "every enum shape and its cardinality contract belong in one place"
)]
fn translate_enum(
    schema: &EnumSchema,
    unsupported: &impl Fn(&str) -> McpMrtrUnsupported,
) -> Result<TranslatedProperty, McpMrtrUnsupported> {
    // (label, protocol value) pairs in the schema's own declared order.
    let (choices, multi, min_items, max_items, title, description) = match schema {
        EnumSchema::Single(SingleSelectEnumSchema::Untitled(single)) => (
            untitled_options(&single.enum_),
            false,
            None,
            None,
            single.title.as_deref().map(str::to_owned),
            single.description.as_deref().map(str::to_owned),
        ),
        EnumSchema::Single(SingleSelectEnumSchema::Titled(single)) => (
            titled_options(&single.one_of),
            false,
            None,
            None,
            single.title.as_deref().map(str::to_owned),
            single.description.as_deref().map(str::to_owned),
        ),
        EnumSchema::Multi(MultiSelectEnumSchema::Untitled(multi)) => (
            untitled_options(&multi.items.enum_),
            true,
            multi.min_items,
            multi.max_items,
            multi.title.as_deref().map(str::to_owned),
            multi.description.as_deref().map(str::to_owned),
        ),
        EnumSchema::Multi(MultiSelectEnumSchema::Titled(multi)) => (
            titled_options(&multi.items.any_of),
            true,
            multi.min_items,
            multi.max_items,
            multi.title.as_deref().map(str::to_owned),
            multi.description.as_deref().map(str::to_owned),
        ),
        EnumSchema::Legacy(legacy) => {
            let choices = match &legacy.enum_names {
                Some(names) if names.len() == legacy.enum_.len() => legacy
                    .enum_
                    .iter()
                    .zip(names)
                    .map(|(value, name)| (name.clone(), serde_json::Value::String(value.clone())))
                    .collect(),
                Some(_) => {
                    return Err(unsupported(
                        "its enumNames do not correspond one-to-one with its enum values",
                    ));
                }
                None => untitled_options(&legacy.enum_),
            };
            (
                choices,
                false,
                None,
                None,
                legacy.title.as_deref().map(str::to_owned),
                legacy.description.as_deref().map(str::to_owned),
            )
        }
        _ => return Err(unsupported("it declares an unknown enum schema shape")),
    };

    if !(MIN_CHOICE_OPTIONS..=MAX_CHOICE_OPTIONS).contains(&choices.len()) {
        return Err(unsupported(&format!(
            "it declares {} choices, and rustX presents \
             {MIN_CHOICE_OPTIONS}–{MAX_CHOICE_OPTIONS}",
            choices.len()
        )));
    }
    let mut labels = BTreeSet::new();
    for (label, _) in &choices {
        if label.trim().is_empty() || label.chars().count() > MAX_OPTION_LABEL_CHARS {
            return Err(unsupported(&format!(
                "the choice label {label:?} is empty or above the \
                 {MAX_OPTION_LABEL_CHARS}-character bound"
            )));
        }
        // Two rows a human cannot tell apart are refused deterministically
        // rather than resolved arbitrarily, even though the *response*
        // addresses an option by index and is never ambiguous to the runtime.
        if !labels.insert(label.clone()) {
            return Err(unsupported(&format!(
                "two of its choices are both presented as {label:?}, which a human could not \
                 distinguish"
            )));
        }
    }

    let options: Vec<OptionSpecification> = choices
        .iter()
        .map(|(label, value)| OptionSpecification {
            label: label.clone(),
            description: format!("MCP value: {value}"),
            preview: None,
        })
        .collect();
    let values: Vec<serde_json::Value> = choices.into_iter().map(|(_, value)| value).collect();
    let count = u32::try_from(values.len()).unwrap_or(u32::MAX);

    let answer = if multi {
        let bound = |value: Option<u64>, name: &str| match value {
            None => Ok(None),
            Some(value) => u32::try_from(value)
                .map(Some)
                .map_err(|_| unsupported(&format!("its {name} is above any answerable count"))),
        };
        let min_selected = bound(min_items, "minItems")?.unwrap_or(0);
        if min_selected > count {
            return Err(unsupported(&format!(
                "its minItems is {min_selected} but it declares only {count} choices, so no \
                 selection could satisfy it"
            )));
        }
        // A maxItems above the member count is vacuous; clamping it keeps the
        // published bound answerable while enforcing the server's constraint.
        let max_selected = bound(max_items, "maxItems")?.map_or(count, |value| value.min(count));
        if min_selected > max_selected {
            return Err(unsupported(
                "its minItems exceeds its maxItems, so no selection could satisfy it",
            ));
        }
        AnswerSpecification::MultiChoice(MultiChoiceSpecification {
            options,
            min_selected,
            max_selected,
            // An MCP `enum` accepts only its declared members, so a free-text
            // answer is not a legal response shape and is not offered at all.
            allow_custom: false,
        })
    } else {
        AnswerSpecification::SingleChoice(SingleChoiceSpecification {
            options,
            allow_custom: false,
        })
    };
    Ok(TranslatedProperty {
        answer,
        values: if multi {
            PropertyValues::MultiSelect(values)
        } else {
            PropertyValues::SingleSelect(values)
        },
        title,
        description,
    })
}

fn untitled_options(values: &[String]) -> Vec<(String, serde_json::Value)> {
    values
        .iter()
        .map(|value| (value.clone(), serde_json::Value::String(value.clone())))
        .collect()
}

fn titled_options(values: &[rmcp::model::ConstTitle]) -> Vec<(String, serde_json::Value)> {
    values
        .iter()
        .map(|entry| {
            (
                entry.title.clone(),
                serde_json::Value::String(entry.const_.clone()),
            )
        })
        .collect()
}

/// A stable, human-meaningful ordering of the plan's request keys, used by
/// diagnostics and tests.
#[cfg(test)]
impl ElicitationPlan {
    fn request_keys(&self) -> Vec<&str> {
        self.requests.iter().map(|plan| plan.key.as_str()).collect()
    }

    fn slot_properties(&self) -> Vec<(&str, &str)> {
        self.slots
            .iter()
            .map(|slot| {
                (
                    self.requests[slot.request_index].key.as_str(),
                    slot.property.as_str(),
                )
            })
            .collect()
    }
}

/// Builds the `BTreeMap` rmcp uses for `inputRequests` from JSON, so tests
/// and fixtures describe requests exactly as they appear on the wire.
#[cfg(test)]
pub(super) fn input_requests_from_json(
    value: serde_json::Value,
) -> Result<InputRequests, serde_json::Error> {
    serde_json::from_value(value)
}

#[cfg(test)]
#[allow(clippy::items_after_statements)]
mod tests {
    use super::*;
    use crate::events::interaction::{
        BooleanAnswer, CustomAnswer, IntegerAnswer, NumberAnswer, OptionAnswer, OptionsAnswer,
        QuestionnaireAnswerEntry, QuestionnaireSubmission, TextAnswer,
        normalize_questionnaire_response,
    };

    fn elicitation(message: &str, schema: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "method": "elicitation/create",
            "params": {"message": message, "requestedSchema": schema.clone()},
        })
    }

    fn enum_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "channel": {"type": "string", "enum": ["stable", "beta"]},
            },
            "required": ["channel"],
        })
    }

    fn requests(value: serde_json::Value) -> InputRequests {
        input_requests_from_json(value).expect("input requests")
    }

    fn plan_of(schema: &serde_json::Value) -> ElicitationPlan {
        plan_round(Some(&requests(serde_json::json!({
            "ask": elicitation("Tell me", schema),
        }))))
        .expect("supported")
        .expect("a questionnaire")
    }

    fn refusal(schema: &serde_json::Value) -> String {
        plan_round(Some(&requests(serde_json::json!({
            "ask": elicitation("Tell me", schema),
        }))))
        .expect_err("unsupported")
        .diagnostic
    }

    fn submitted(entries: Vec<QuestionnaireAnswerEntry>) -> QuestionnaireResponse {
        QuestionnaireResponse::Submitted(QuestionnaireSubmission { answers: entries })
    }

    fn entry(index: usize, answer: QuestionnaireAnswer) -> QuestionnaireAnswerEntry {
        QuestionnaireAnswerEntry {
            question_index: index,
            answer,
        }
    }

    fn single(index: usize, option_index: usize) -> QuestionnaireAnswerEntry {
        entry(
            index,
            QuestionnaireAnswer::Option(OptionAnswer { option_index }),
        )
    }

    /// The one authoritative validation point, applied exactly as the runtime
    /// applies it, so a test can never accept a response the runtime would
    /// refuse.
    fn accepted(
        plan: &ElicitationPlan,
        response: &QuestionnaireResponse,
    ) -> Result<InputResponses, String> {
        let normalized = normalize_questionnaire_response(plan.questionnaire(), response)?;
        plan.responses(&normalized)
    }

    /// A single-property elicitation keeps the server's own message verbatim
    /// and maps the answer onto the exact enum value by index.
    #[test]
    fn one_supported_elicitation_becomes_one_question_and_one_accept() {
        let plan = plan_of(&enum_schema());
        assert_eq!(plan.questionnaire().questions.len(), 1);
        assert_eq!(plan.questionnaire().questions[0].question, "Tell me");
        let AnswerSpecification::SingleChoice(single_choice) =
            &plan.questionnaire().questions[0].answer
        else {
            panic!("an enum is a single choice")
        };
        assert!(!single_choice.allow_custom);
        assert_eq!(
            single_choice
                .options
                .iter()
                .map(|option| option.label.as_str())
                .collect::<Vec<_>>(),
            vec!["stable", "beta"]
        );
        let responses = accepted(&plan, &submitted(vec![single(0, 1)])).expect("responses");
        assert_eq!(
            responses,
            InputResponses::from([(
                "ask".to_owned(),
                serde_json::json!({"action": "accept", "content": {"channel": "beta"}}),
            )])
        );
    }

    /// Several input requests in one round stay separately addressed: the
    /// mapping is by server key, never positional.
    #[test]
    fn several_input_requests_map_by_key_not_by_position() {
        let plan = plan_round(Some(&requests(serde_json::json!({
            "zeta": elicitation("Second?", &enum_schema()),
            "alpha": elicitation("First?", &enum_schema()),
        }))))
        .expect("supported")
        .expect("a questionnaire");
        // BTreeMap order is the deterministic wire order.
        assert_eq!(plan.request_keys(), vec!["alpha", "zeta"]);
        assert_eq!(
            plan.slot_properties(),
            vec![("alpha", "channel"), ("zeta", "channel")]
        );
        assert_eq!(plan.questionnaire().questions.len(), 2);
        let responses =
            accepted(&plan, &submitted(vec![single(0, 0), single(1, 1)])).expect("responses");
        assert_eq!(
            responses["alpha"],
            serde_json::json!({"action": "accept", "content": {"channel": "stable"}})
        );
        assert_eq!(
            responses["zeta"],
            serde_json::json!({"action": "accept", "content": {"channel": "beta"}})
        );
    }

    /// An explicit human decline becomes the protocol's own decline action,
    /// never a fabricated answer and never a cancellation.
    #[test]
    fn an_explicit_decline_becomes_the_protocol_decline_action() {
        let plan = plan_of(&enum_schema());
        let responses = accepted(&plan, &QuestionnaireResponse::Declined).expect("responses");
        assert_eq!(responses["ask"], serde_json::json!({"action": "decline"}));
    }

    /// An unanswered required property cannot produce a schema-valid accept,
    /// so the request declines rather than sending partial content.
    #[test]
    fn an_unanswered_required_property_declines_instead_of_sending_partial_content() {
        let plan = plan_of(&enum_schema());
        let responses = plan.responses(&submitted(Vec::new())).expect("responses");
        assert_eq!(responses["ask"], serde_json::json!({"action": "decline"}));
    }

    /// An MCP choice has no custom-answer shape at all: the specification
    /// forbids it, the runtime refuses it, and the mapping refuses it too.
    #[test]
    fn an_mcp_choice_has_no_custom_answer_path() {
        let plan = plan_of(&enum_schema());
        let custom = submitted(vec![entry(
            0,
            QuestionnaireAnswer::Custom(CustomAnswer {
                answer: "nightly".to_owned(),
            }),
        )]);
        let error = accepted(&plan, &custom).expect_err("a bounded choice takes no free text");
        assert!(error.contains("free-text"), "{error}");
        // Even bypassing the runtime validator, the mapping refuses it.
        let error = plan
            .responses(&custom)
            .expect_err("the mapping refuses it too");
        assert!(error.contains("declared schema values"), "{error}");
    }

    /// A free-form string form completes end to end, with the schema's own
    /// title and description carried into the prompt.
    #[test]
    fn a_free_form_string_becomes_a_typed_text_question() {
        let plan = plan_of(&serde_json::json!({
            "type": "object",
            "properties": {
                "operator": {
                    "type": "string",
                    "title": "GitHub user",
                    "description": "What is your GitHub username?",
                    "minLength": 1,
                    "maxLength": 39,
                },
            },
            "required": ["operator"],
        }));
        let question = &plan.questionnaire().questions[0];
        assert_eq!(question.header, "GitHub user");
        assert!(
            question.question.contains("What is your GitHub username?"),
            "{}",
            question.question
        );
        assert_eq!(
            question.answer,
            AnswerSpecification::Text(TextAnswerSpecification {
                min_length: Some(1),
                max_length: Some(39),
                format: None,
            })
        );
        let responses = accepted(
            &plan,
            &submitted(vec![entry(
                0,
                QuestionnaireAnswer::Text(TextAnswer {
                    value: "octocat".to_owned(),
                }),
            )]),
        )
        .expect("responses");
        assert_eq!(
            responses["ask"],
            serde_json::json!({"action": "accept", "content": {"operator": "octocat"}})
        );

        // Every declared bound is enforced by the runtime validator, and an
        // out-of-bound answer never reaches an MCP continuation.
        for invalid in ["", &"x".repeat(40)] {
            assert!(
                accepted(
                    &plan,
                    &submitted(vec![entry(
                        0,
                        QuestionnaireAnswer::Text(TextAnswer {
                            value: invalid.to_owned()
                        }),
                    )]),
                )
                .is_err(),
                "{invalid:?}"
            );
        }
    }

    /// Numbers and integers stay semantically distinct, and both round-trip
    /// as JSON numbers rather than strings.
    #[test]
    fn numbers_and_integers_are_typed_and_range_checked() {
        let plan = plan_of(&serde_json::json!({
            "type": "object",
            "properties": {"age": {"type": "integer", "minimum": 0, "maximum": 150}},
            "required": ["age"],
        }));
        assert_eq!(
            plan.questionnaire().questions[0].answer,
            AnswerSpecification::Integer(crate::events::interaction::IntegerAnswerSpecification {
                minimum: Some(ExactInteger::new(0)),
                maximum: Some(ExactInteger::new(150)),
            })
        );
        let responses = accepted(
            &plan,
            &submitted(vec![entry(
                0,
                QuestionnaireAnswer::Integer(IntegerAnswer {
                    value: ExactInteger::new(42),
                }),
            )]),
        )
        .expect("responses");
        assert_eq!(
            responses["ask"],
            serde_json::json!({"action": "accept", "content": {"age": 42}})
        );
        // A fractional value is not an integer answer at all.
        assert!(
            accepted(
                &plan,
                &submitted(vec![entry(
                    0,
                    QuestionnaireAnswer::Number(NumberAnswer {
                        value: FiniteNumber::try_new(1.5).expect("finite")
                    }),
                )]),
            )
            .is_err()
        );
        // The declared range is authoritative.
        assert!(
            accepted(
                &plan,
                &submitted(vec![entry(
                    0,
                    QuestionnaireAnswer::Integer(IntegerAnswer {
                        value: ExactInteger::new(151)
                    }),
                )]),
            )
            .is_err()
        );

        let plan = plan_of(&serde_json::json!({
            "type": "object",
            "properties": {"ratio": {"type": "number", "minimum": 0.0, "maximum": 1.0}},
            "required": ["ratio"],
        }));
        let responses = accepted(
            &plan,
            &submitted(vec![entry(
                0,
                QuestionnaireAnswer::Number(NumberAnswer {
                    value: FiniteNumber::try_new(0.25).expect("finite"),
                }),
            )]),
        )
        .expect("responses");
        assert_eq!(
            responses["ask"],
            serde_json::json!({"action": "accept", "content": {"ratio": 0.25}})
        );
        assert!(
            accepted(
                &plan,
                &submitted(vec![entry(
                    0,
                    QuestionnaireAnswer::Number(NumberAnswer {
                        value: FiniteNumber::try_new(1.25).expect("finite")
                    }),
                )]),
            )
            .is_err()
        );
    }

    /// The `2^53 + 1` failure class: the value cannot even be spelled as an
    /// answer, so it can never be validated against the `2^53` maximum and
    /// then emitted as something else.
    ///
    /// The Runtime Client `Number` wire is the canonical binary64 text, whose
    /// alphabet *is* the domain, so a decimal binary64 cannot hold has no
    /// representation to arrive in — and a raw JSON number is not a `Number`
    /// answer at all.
    fn assert_the_frontier_has_one_spelling() {
        for json_number in [
            r#"{"type":"number","value":{"value":9007199254740993}}"#,
            r#"{"type":"number","value":{"value":9007199254740992}}"#,
        ] {
            assert!(
                serde_json::from_str::<QuestionnaireAnswer>(json_number).is_err(),
                "{json_number}: a JSON number is not a Number answer"
            );
        }
        // The exact value has exactly one spelling, and it decodes to the very
        // bits the runtime validates and emits.
        let frontier = FiniteNumber::try_new(9_007_199_254_740_992.0).expect("finite");
        assert_eq!(
            serde_json::from_str::<QuestionnaireAnswer>(&format!(
                r#"{{"type":"number","value":{{"value":"{}"}}}}"#,
                frontier.to_wire()
            ))
            .expect("the canonical spelling decodes"),
            QuestionnaireAnswer::Number(NumberAnswer { value: frontier })
        );
    }

    /// The emitted `accept.content` value is the very value the runtime
    /// range-checked — for every scalar shape, including the values that used
    /// to slip through the old `serde_json::Number` / `f64` split.
    #[test]
    fn every_accepted_scalar_answer_is_emitted_exactly_as_validated() {
        // A Number question wide enough to hold the precision frontier.
        let number_plan = plan_of(&serde_json::json!({
            "type": "object",
            "properties": {
                "amount": {
                    "type": "number",
                    "minimum": -1.0e18,
                    "maximum": 9_007_199_254_740_992.0,
                },
            },
            "required": ["amount"],
        }));
        for value in [
            0.0_f64,
            -0.5,
            0.1,
            2.5,
            -1.0e18,
            9_007_199_254_740_991.0,
            9_007_199_254_740_992.0,
        ] {
            let answer = QuestionnaireAnswer::Number(NumberAnswer {
                value: FiniteNumber::try_new(value).expect("finite"),
            });
            let normalized = normalize_questionnaire_response(
                number_plan.questionnaire(),
                &submitted(vec![entry(0, answer.clone())]),
            )
            .unwrap_or_else(|error| panic!("{value} must validate: {error}"));
            // What validation settled on...
            let QuestionnaireResponse::Submitted(settled) = &normalized else {
                panic!("expected a submission");
            };
            assert_eq!(settled.answers[0].answer, answer);
            // ...is bit-for-bit what the server is sent.
            let emitted = number_plan.responses(&normalized).expect("responses");
            let content = &emitted["ask"]["content"]["amount"];
            // An *exact* comparison is the assertion: a tolerance here would
            // hide precisely the divergence this test exists to forbid.
            #[allow(clippy::float_cmp, reason = "bit-equality is the property under test")]
            {
                assert_eq!(
                    content.as_f64().expect("a JSON number"),
                    value,
                    "the validated value and the emitted value are the same number"
                );
            }
        }

        assert_the_frontier_has_one_spelling();

        // An Integer question whose entire legal answer set lies above the
        // JavaScript safe-integer range is publishable *and* answerable,
        // because the Runtime Client representation is decimal text.
        let integer_plan = plan_of(&serde_json::json!({
            "type": "object",
            "properties": {
                "ledger": {
                    "type": "integer",
                    "minimum": 9_007_199_254_740_992_i64,
                    "maximum": 9_223_372_036_854_775_807_i64,
                },
            },
            "required": ["ledger"],
        }));
        for value in [9_007_199_254_740_992_i64, 9_007_199_254_740_993, i64::MAX] {
            let answer = QuestionnaireAnswer::Integer(IntegerAnswer {
                value: ExactInteger::new(value),
            });
            // The wire form is the exact decimal text, never a JSON number.
            let wire = serde_json::to_value(&answer).expect("encodes");
            assert_eq!(wire["value"]["value"], serde_json::json!(value.to_string()));
            let decoded: QuestionnaireAnswer = serde_json::from_value(wire).expect("decodes");
            assert_eq!(decoded, answer);

            let normalized = normalize_questionnaire_response(
                integer_plan.questionnaire(),
                &submitted(vec![entry(0, decoded)]),
            )
            .unwrap_or_else(|error| panic!("{value} must validate: {error}"));
            let emitted = integer_plan.responses(&normalized).expect("responses");
            assert_eq!(
                emitted["ask"]["content"]["ledger"],
                serde_json::json!(value),
                "the exact whole number reaches MCP unrounded"
            );
        }
        // Exact bound comparison at that magnitude, with no rounding to blur it.
        for outside in [9_007_199_254_740_991_i64, i64::MIN] {
            assert!(
                accepted(
                    &integer_plan,
                    &submitted(vec![entry(
                        0,
                        QuestionnaireAnswer::Integer(IntegerAnswer {
                            value: ExactInteger::new(outside),
                        }),
                    )]),
                )
                .is_err(),
                "{outside}"
            );
        }
    }

    /// An explicit empty string is a real answer, and reaches the server as
    /// `""` — not as the decline that an *omitted* required property produces.
    #[test]
    fn an_explicit_empty_text_answer_accepts_where_an_omission_declines() {
        let plan = plan_of(&serde_json::json!({
            "type": "object",
            "properties": {"note": {"type": "string", "minLength": 0}},
            "required": ["note"],
        }));
        let empty = QuestionnaireAnswer::Text(TextAnswer {
            value: String::new(),
        });

        // Answered with the empty string: `accept`, with a real empty value.
        let answered = accepted(&plan, &submitted(vec![entry(0, empty.clone())])).expect("accepts");
        assert_eq!(
            answered["ask"],
            serde_json::json!({"action": "accept", "content": {"note": ""}})
        );

        // Not answered at all: the required property is missing, so rustX
        // declines rather than sending content the server's schema rejects.
        // The two responses are different facts about the same question.
        let omitted = accepted(&plan, &submitted(vec![])).expect("declines");
        assert_eq!(omitted["ask"], serde_json::json!({"action": "decline"}));
        assert_ne!(answered["ask"], omitted["ask"]);

        // A positive minLength still refuses the empty string, and the refusal
        // is a rejected interaction response, never an emitted `accept`.
        let bounded = plan_of(&serde_json::json!({
            "type": "object",
            "properties": {"note": {"type": "string", "minLength": 1}},
            "required": ["note"],
        }));
        assert!(accepted(&bounded, &submitted(vec![entry(0, empty)])).is_err());
    }

    /// Booleans round-trip as JSON booleans in both directions; `Yes`/`No` is
    /// never the business value.
    #[test]
    fn booleans_round_trip_as_json_booleans() {
        let plan = plan_of(&serde_json::json!({
            "type": "object",
            "properties": {"notify": {"type": "boolean"}},
            "required": ["notify"],
        }));
        assert_eq!(
            plan.questionnaire().questions[0].answer,
            AnswerSpecification::Boolean
        );
        for value in [true, false] {
            let responses = accepted(
                &plan,
                &submitted(vec![entry(
                    0,
                    QuestionnaireAnswer::Boolean(BooleanAnswer { value }),
                )]),
            )
            .expect("responses");
            assert_eq!(
                responses["ask"],
                serde_json::json!({"action": "accept", "content": {"notify": value}})
            );
        }
    }

    /// A titled enum presents its titles and sends the exact `const` values.
    #[test]
    fn titled_enums_present_titles_and_send_their_const_values() {
        let plan = plan_of(&serde_json::json!({
            "type": "object",
            "properties": {
                "channel": {
                    "type": "string",
                    "oneOf": [
                        {"const": "ga", "title": "General availability"},
                        {"const": "rc", "title": "Release candidate"},
                    ],
                },
            },
            "required": ["channel"],
        }));
        let AnswerSpecification::SingleChoice(choice) = &plan.questionnaire().questions[0].answer
        else {
            panic!("a titled enum is a single choice")
        };
        assert_eq!(choice.options[1].label, "Release candidate");
        let responses = accepted(&plan, &submitted(vec![single(0, 1)])).expect("responses");
        assert_eq!(
            responses["ask"],
            serde_json::json!({"action": "accept", "content": {"channel": "rc"}})
        );
    }

    /// Two choices a human cannot tell apart are refused deterministically.
    #[test]
    fn ambiguous_duplicate_titles_are_refused_rather_than_resolved() {
        let error = refusal(&serde_json::json!({
            "type": "object",
            "properties": {
                "channel": {
                    "type": "string",
                    "oneOf": [
                        {"const": "ga", "title": "Stable"},
                        {"const": "lts", "title": "Stable"},
                    ],
                },
            },
        }));
        assert!(error.contains("could not distinguish"), "{error}");
    }

    /// The legacy `enumNames` form maps names to their exact enum values.
    #[test]
    fn the_legacy_enum_names_form_maps_names_to_values() {
        let plan = plan_of(&serde_json::json!({
            "type": "object",
            "properties": {
                "channel": {
                    "type": "string",
                    "enum": ["ga", "rc"],
                    "enumNames": ["General availability", "Release candidate"],
                },
            },
            "required": ["channel"],
        }));
        let responses = accepted(&plan, &submitted(vec![single(0, 0)])).expect("responses");
        assert_eq!(
            responses["ask"],
            serde_json::json!({"action": "accept", "content": {"channel": "ga"}})
        );
        let error = refusal(&serde_json::json!({
            "type": "object",
            "properties": {
                "channel": {"type": "string", "enum": ["ga", "rc"], "enumNames": ["Only one"]},
            },
        }));
        assert!(error.contains("one-to-one"), "{error}");
    }

    /// A multi-select enum without explicit bounds accepts any non-empty
    /// subset up to its member count, in canonical order.
    #[test]
    fn multi_select_without_bounds_accepts_any_subset() {
        let plan = plan_of(&serde_json::json!({
            "type": "object",
            "properties": {
                "regions": {"type": "array", "items": {"type": "string", "enum": ["eu", "us"]}},
            },
            "required": ["regions"],
        }));
        let AnswerSpecification::MultiChoice(multi) = &plan.questionnaire().questions[0].answer
        else {
            panic!("an array enum is a multi choice")
        };
        assert_eq!((multi.min_selected, multi.max_selected), (0, 2));
        assert!(!multi.allow_custom);
        let responses = accepted(
            &plan,
            &submitted(vec![entry(
                0,
                QuestionnaireAnswer::Options(OptionsAnswer {
                    option_indices: vec![1, 0],
                }),
            )]),
        )
        .expect("responses");
        assert_eq!(
            responses["ask"],
            serde_json::json!({
                "action": "accept",
                "content": {"regions": ["eu", "us"]},
            })
        );
    }

    /// `minItems` and `maxItems` are preserved into the typed question and
    /// enforced by the authoritative runtime validator.
    #[test]
    fn multi_select_cardinality_bounds_are_preserved_and_enforced() {
        let bounded = |min: serde_json::Value, max: serde_json::Value| {
            let mut items = serde_json::json!({
                "type": "array",
                "items": {"type": "string", "enum": ["a", "b", "c"]},
            });
            if !min.is_null() {
                items["minItems"] = min;
            }
            if !max.is_null() {
                items["maxItems"] = max;
            }
            plan_of(&serde_json::json!({
                "type": "object",
                "properties": {"pick": items},
                "required": ["pick"],
            }))
        };
        let selection = |indices: Vec<usize>| {
            submitted(vec![entry(
                0,
                QuestionnaireAnswer::Options(OptionsAnswer {
                    option_indices: indices,
                }),
            )])
        };

        let at_least_two = bounded(serde_json::json!(2), serde_json::Value::Null);
        let AnswerSpecification::MultiChoice(multi) =
            &at_least_two.questionnaire().questions[0].answer
        else {
            panic!("multi choice")
        };
        assert_eq!((multi.min_selected, multi.max_selected), (2, 3));
        assert!(accepted(&at_least_two, &selection(vec![0])).is_err());
        assert!(accepted(&at_least_two, &selection(vec![0, 1])).is_ok());

        let at_most_two = bounded(serde_json::Value::Null, serde_json::json!(2));
        assert!(accepted(&at_most_two, &selection(vec![0, 1])).is_ok());
        assert!(accepted(&at_most_two, &selection(vec![0, 1, 2])).is_err());

        let exactly_two = bounded(serde_json::json!(2), serde_json::json!(2));
        assert!(accepted(&exactly_two, &selection(vec![0])).is_err());
        let responses = accepted(&exactly_two, &selection(vec![0, 1])).expect("responses");
        assert_eq!(
            responses["ask"],
            serde_json::json!({"action": "accept", "content": {"pick": ["a", "b"]}})
        );
        assert!(accepted(&exactly_two, &selection(vec![0, 1, 2])).is_err());
    }

    /// Impossible or inconsistent cardinality is refused before any
    /// interaction is published.
    #[test]
    fn impossible_cardinality_is_refused_before_publication() {
        let schema = |min: u64, max: serde_json::Value| {
            let mut items = serde_json::json!({
                "type": "array",
                "items": {"type": "string", "enum": ["a", "b"]},
                "minItems": min,
            });
            if !max.is_null() {
                items["maxItems"] = max;
            }
            serde_json::json!({"type": "object", "properties": {"pick": items}})
        };
        let error = refusal(&schema(3, serde_json::Value::Null));
        assert!(error.contains("minItems is 3"), "{error}");
        let error = refusal(&schema(2, serde_json::json!(1)));
        assert!(error.contains("exceeds its maxItems"), "{error}");
    }

    /// Sampling and roots are refused deterministically, and a mixed set is
    /// refused as a whole so no partial prompt can be published.
    #[test]
    fn sampling_roots_and_mixed_sets_are_refused_before_any_prompt() {
        let sampling = serde_json::json!({
            "method": "sampling/createMessage",
            "params": {
                "messages": [{"role": "user", "content": {"type": "text", "text": "hi"}}],
                "maxTokens": 8,
            },
        });
        let roots = serde_json::json!({"method": "roots/list"});
        let error = plan_round(Some(&requests(
            serde_json::json!({"ask": sampling.clone()}),
        )))
        .expect_err("sampling is unsupported");
        assert!(error.diagnostic.contains("sampling"), "{error:?}");
        let error = plan_round(Some(&requests(serde_json::json!({"ask": roots.clone()}))))
            .expect_err("roots is unsupported");
        assert!(error.diagnostic.contains("roots"), "{error:?}");
        let error = plan_round(Some(&requests(serde_json::json!({
            "supported": elicitation("Which channel?", &enum_schema()),
            "unsupported": sampling,
        }))))
        .expect_err("a mixed set is unsupported");
        assert!(error.diagnostic.contains("sampling"), "{error:?}");
    }

    /// A constraint rustX cannot faithfully enforce refuses only that schema
    /// form, never every free-form string.
    #[test]
    fn only_the_unvalidatable_string_format_is_refused() {
        let error = refusal(&serde_json::json!({
            "type": "object",
            "properties": {"contact": {"type": "string", "format": "email"}},
        }));
        assert!(error.contains("email"), "{error}");

        for (format, valid, invalid) in [
            ("date", "2026-09-09", "09/09/2026"),
            ("date-time", "2026-09-09T10:11:12Z", "yesterday"),
            ("uri", "https://example.test/x", "not a uri"),
        ] {
            let plan = plan_of(&serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "string", "format": format}},
                "required": ["value"],
            }));
            assert!(
                accepted(
                    &plan,
                    &submitted(vec![entry(
                        0,
                        QuestionnaireAnswer::Text(TextAnswer {
                            value: valid.to_owned()
                        }),
                    )]),
                )
                .is_ok(),
                "{format} accepts {valid}"
            );
            assert!(
                accepted(
                    &plan,
                    &submitted(vec![entry(
                        0,
                        QuestionnaireAnswer::Text(TextAnswer {
                            value: invalid.to_owned()
                        }),
                    )]),
                )
                .is_err(),
                "{format} refuses {invalid}"
            );
        }
    }

    /// A pure `requestState` round asks nothing and simply continues.
    #[test]
    fn a_state_only_round_publishes_no_questionnaire() {
        assert!(plan_round(None).expect("supported").is_none());
        assert!(
            plan_round(Some(&InputRequests::new()))
                .expect("supported")
                .is_none()
        );
    }

    /// Oversized protocol state is rejected before it can be retained.
    #[test]
    fn oversized_request_state_is_rejected_before_retention() {
        let state = "x".repeat(MCP_MRTR_MAX_REQUEST_STATE_BYTES + 1);
        let error = validate_request_state(Some(&state)).expect_err("oversized");
        assert!(error.diagnostic.contains("retention bound"), "{error:?}");
        validate_request_state(Some(&"x".repeat(MCP_MRTR_MAX_REQUEST_STATE_BYTES)))
            .expect("at the bound");
    }

    /// Too many input requests, and too many derived questions, are both
    /// rejected before any interaction exists.
    #[test]
    fn oversized_input_request_sets_are_rejected_before_any_interaction() {
        let mut map = serde_json::Map::new();
        for index in 0..=MCP_MRTR_MAX_INPUT_REQUESTS {
            map.insert(
                format!("ask-{index}"),
                elicitation(&format!("Question {index}?"), &enum_schema()),
            );
        }
        let error = plan_round(Some(&requests(serde_json::Value::Object(map))))
            .expect_err("above the request bound");
        assert!(error.diagnostic.contains("input requests"), "{error:?}");

        let mut properties = serde_json::Map::new();
        for index in 0..=MAX_QUESTIONNAIRE_QUESTIONS {
            properties.insert(
                format!("field{index}"),
                serde_json::json!({"type": "string", "enum": ["a", "b"]}),
            );
        }
        let error = refusal(&serde_json::json!({
            "type": "object",
            "properties": properties,
        }));
        assert!(error.contains("bounded questionnaire"), "{error}");
    }

    /// A bounded MCP choice offers no client-reserved row, so a schema value
    /// such as `Other` is an ordinary choice rather than a collision.
    #[test]
    fn client_reserved_words_are_ordinary_values_for_a_bounded_choice() {
        let plan = plan_of(&serde_json::json!({
            "type": "object",
            "properties": {"pick": {"type": "string", "enum": ["Other", "keep"]}},
            "required": ["pick"],
        }));
        let responses = accepted(&plan, &submitted(vec![single(0, 0)])).expect("responses");
        assert_eq!(
            responses["ask"],
            serde_json::json!({"action": "accept", "content": {"pick": "Other"}})
        );
    }

    /// An enum above the bounded choice count is refused with its own
    /// diagnostic rather than an opaque questionnaire failure.
    #[test]
    fn an_enum_above_the_choice_bound_is_refused() {
        let values: Vec<String> = (0..=MAX_CHOICE_OPTIONS).map(|i| format!("v{i}")).collect();
        let error = refusal(&serde_json::json!({
            "type": "object",
            "properties": {"pick": {"type": "string", "enum": values}},
        }));
        assert!(error.contains("choices"), "{error}");
    }
}
