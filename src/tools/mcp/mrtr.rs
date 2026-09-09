//! The MCP multi-round-trip (MRTR, SEP-2322) protocol translation layer
//! (Issue #242).
//!
//! # One invocation, N rounds, one settlement
//!
//! MCP `2026-07-28` lets a server answer `tools/call` with an
//! [`InputRequiredResult`] instead of a `CallToolResult`. That answer is an
//! **intermediate state of the already-admitted rustX `ToolInvocation`**, never
//! a terminal `ToolResult`, never a new invocation, and never a new
//! `ToolExecutionId`:
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
//! means, which MCP input requests rustX can faithfully represent as a
//! bounded runtime Questionnaire, and how a typed Questionnaire response
//! becomes the exact `inputResponses` map the next round must carry. The
//! round driver, the dispatch frontier, cancellation arbitration, and
//! terminal settlement stay in [`super`], where the existing MCP execution
//! semantics already live.
//!
//! # What rustX supports, and why the rest is refused
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
//! Both are refused with a bounded unsupported-feature diagnostic. Nothing is
//! fabricated, no model call is made, and no workspace root is disclosed.
//!
//! Within elicitation, rustX accepts exactly the schema subset its
//! [`QuestionnaireSpecification`] vocabulary can represent **without
//! coercion**: one MCP property becomes one rustX question, and a question is
//! a bounded choice over 2–4 authored options. So `boolean` properties and
//! `enum` properties (single- and multi-select, titled or untitled) are
//! supported, and free-form `string`/`number`/`integer` properties are not:
//! rustX has no faithful bounded representation for them and will not invent
//! two fake options to manufacture one. URL-mode elicitation is likewise
//! refused — it directs a human to an external URL, which is not a
//! questionnaire.
//!
//! A round whose requests mix supported and unsupported kinds fails
//! **before** anything is published, so a user is never shown half a prompt
//! that is about to be abandoned.

use std::collections::BTreeSet;

use rmcp::model::{
    ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationSchema, EnumSchema,
    InputRequest, InputRequests, InputResponses, MultiSelectEnumSchema, PrimitiveSchemaDefinition,
    SingleSelectEnumSchema,
};

use crate::events::interaction::{
    MAX_OPTION_LABEL_CHARS, MAX_QUESTION_HEADER_CHARS, MAX_QUESTION_TEXT_CHARS,
    MAX_QUESTIONNAIRE_OPTIONS, MAX_QUESTIONNAIRE_QUESTIONS, MIN_QUESTIONNAIRE_OPTIONS,
    OptionSpecification, QuestionSpecification, QuestionnaireAnswer, QuestionnaireResponse,
    QuestionnaireSpecification,
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

/// How rustX will render one MCP property as one rustX question, and how the
/// answer maps back to a schema-valid JSON value.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PropertyKind {
    /// A `boolean` property: exactly the two values the schema defines.
    Boolean,
    /// A single-select `enum`: one authored option per enum member.
    SingleSelect,
    /// A multi-select `enum` array: any non-empty subset of the members.
    MultiSelect,
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
    kind: PropertyKind,
    /// Authored option label -> the exact JSON value sent to the server.
    values: Vec<(String, serde_json::Value)>,
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
    ///   will not send `accept` with content the server's own schema rejects;
    /// - a **custom** (free-text) answer to a derived bounded question is a
    ///   deterministic failure. rustX cannot prove an arbitrary string
    ///   satisfies the server's `enum`/`boolean` schema, and fabricating an
    ///   out-of-schema `accept` is exactly the coercion this translation
    ///   layer exists to prevent.
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
        match (&self.kind, answer) {
            (PropertyKind::Boolean | PropertyKind::SingleSelect, QuestionnaireAnswer::SingleOption(single)) => self
                .values
                .iter()
                .find(|(label, _)| *label == single.label)
                .map(|(_, value)| value.clone())
                .ok_or_else(|| {
                    format!(
                        "the answer to MCP elicitation property {:?} is not one of its schema values",
                        self.property
                    )
                }),
            (PropertyKind::MultiSelect, QuestionnaireAnswer::MultipleOption(multiple)) => {
                let mut selected = Vec::with_capacity(multiple.selected.len());
                for label in &multiple.selected {
                    let value = self
                        .values
                        .iter()
                        .find(|(candidate, _)| candidate == label)
                        .map(|(_, value)| value.clone())
                        .ok_or_else(|| {
                            format!(
                                "the answer to MCP elicitation property {:?} is not one of its \
                                 schema values",
                                self.property
                            )
                        })?;
                    selected.push(value);
                }
                Ok(serde_json::Value::Array(selected))
            }
            (_, QuestionnaireAnswer::Custom(_)) => Err(format!(
                "MCP elicitation property {:?} accepts only its declared schema values, so the \
                 free-text answer cannot be sent without violating the server's schema",
                self.property
            )),
            (_, _) => Err(format!(
                "the answer kind does not match MCP elicitation property {:?}",
                self.property
            )),
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
            let (kind, values) = translate_property(key, property, definition)?;
            let question = question_text(message, key, property, qualify, single_property);
            if question.chars().count() > MAX_QUESTION_TEXT_CHARS {
                return Err(McpMrtrUnsupported::new(format!(
                    "MCP elicitation request {key:?} property {property:?} produces a prompt \
                     above the {MAX_QUESTION_TEXT_CHARS}-character bound"
                )));
            }
            let header = bounded_header(property);
            let options = values
                .iter()
                .map(|(label, value)| OptionSpecification {
                    label: label.clone(),
                    description: format!("MCP value: {value}"),
                    preview: None,
                })
                .collect();
            questions.push(QuestionSpecification {
                question,
                header,
                options,
                multi_select: kind == PropertyKind::MultiSelect,
            });
            slots.push(PropertySlot {
                request_index,
                property: property.to_owned(),
                required: required.contains(property),
                kind,
                values,
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
    // interaction-publication failure.
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
fn question_text(
    message: &str,
    key: &str,
    property: &str,
    qualify: bool,
    single_property: bool,
) -> String {
    if single_property {
        return message.to_owned();
    }
    if qualify {
        format!("{message}\n\n[{key}.{property}]")
    } else {
        format!("{message}\n\n[{property}]")
    }
}

/// The bounded question tab label derived from the MCP property name.
fn bounded_header(property: &str) -> String {
    let header: String = property.chars().take(MAX_QUESTION_HEADER_CHARS).collect();
    if header.trim().is_empty() {
        "field".to_owned()
    } else {
        header
    }
}

/// Translates one primitive elicitation property into the bounded rustX
/// question vocabulary.
fn translate_property(
    key: &str,
    property: &str,
    definition: &PrimitiveSchemaDefinition,
) -> Result<(PropertyKind, Vec<(String, serde_json::Value)>), McpMrtrUnsupported> {
    let unsupported = |detail: &str| {
        McpMrtrUnsupported::new(format!(
            "MCP elicitation request {key:?} property {property:?} is unsupported: {detail}"
        ))
    };
    let (kind, values) = match definition {
        PrimitiveSchemaDefinition::Boolean(_) => (
            PropertyKind::Boolean,
            vec![
                ("Yes".to_owned(), serde_json::Value::Bool(true)),
                ("No".to_owned(), serde_json::Value::Bool(false)),
            ],
        ),
        PrimitiveSchemaDefinition::Enum(schema) => match schema {
            EnumSchema::Single(SingleSelectEnumSchema::Untitled(single)) => {
                (PropertyKind::SingleSelect, untitled_options(&single.enum_))
            }
            EnumSchema::Single(SingleSelectEnumSchema::Titled(single)) => {
                (PropertyKind::SingleSelect, titled_options(&single.one_of))
            }
            EnumSchema::Multi(MultiSelectEnumSchema::Untitled(multi)) => (
                PropertyKind::MultiSelect,
                untitled_options(&multi.items.enum_),
            ),
            EnumSchema::Multi(MultiSelectEnumSchema::Titled(multi)) => (
                PropertyKind::MultiSelect,
                titled_options(&multi.items.any_of),
            ),
            EnumSchema::Legacy(legacy) => match &legacy.enum_names {
                Some(names) if names.len() == legacy.enum_.len() => (
                    PropertyKind::SingleSelect,
                    legacy
                        .enum_
                        .iter()
                        .zip(names)
                        .map(|(value, name)| {
                            (name.clone(), serde_json::Value::String(value.clone()))
                        })
                        .collect(),
                ),
                Some(_) => {
                    return Err(unsupported(
                        "its enumNames do not correspond one-to-one with its enum values",
                    ));
                }
                None => (PropertyKind::SingleSelect, untitled_options(&legacy.enum_)),
            },
            _ => return Err(unsupported("it declares an unknown enum schema shape")),
        },
        PrimitiveSchemaDefinition::String(_) => {
            return Err(unsupported(
                "rustX questions are bounded choices, and a free-form string has no faithful \
                 bounded representation",
            ));
        }
        PrimitiveSchemaDefinition::Number(_) | PrimitiveSchemaDefinition::Integer(_) => {
            return Err(unsupported(
                "rustX questions are bounded choices, and a free-form number has no faithful \
                 bounded representation",
            ));
        }
        _ => return Err(unsupported("it declares an unknown primitive schema kind")),
    };
    if !(MIN_QUESTIONNAIRE_OPTIONS..=MAX_QUESTIONNAIRE_OPTIONS).contains(&values.len()) {
        return Err(unsupported(&format!(
            "it declares {} choices, and rustX presents \
             {MIN_QUESTIONNAIRE_OPTIONS}–{MAX_QUESTIONNAIRE_OPTIONS}",
            values.len()
        )));
    }
    for (label, _) in &values {
        if label.trim().is_empty() || label.chars().count() > MAX_OPTION_LABEL_CHARS {
            return Err(unsupported(&format!(
                "the choice label {label:?} is empty or above the \
                 {MAX_OPTION_LABEL_CHARS}-character bound"
            )));
        }
    }
    Ok((kind, values))
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
        CustomAnswer, MultipleOptionAnswer, QuestionnaireAnswerEntry, QuestionnaireSubmission,
        SingleOptionAnswer,
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

    fn submitted(entries: Vec<QuestionnaireAnswerEntry>) -> QuestionnaireResponse {
        QuestionnaireResponse::Submitted(QuestionnaireSubmission { answers: entries })
    }

    fn single(index: usize, label: &str) -> QuestionnaireAnswerEntry {
        QuestionnaireAnswerEntry {
            question_index: index,
            answer: QuestionnaireAnswer::SingleOption(SingleOptionAnswer {
                label: label.to_owned(),
            }),
        }
    }

    /// A single-property elicitation keeps the server's own message verbatim
    /// and maps the answer onto the exact enum value.
    #[test]
    fn one_supported_elicitation_becomes_one_question_and_one_accept() {
        let plan = plan_round(Some(&requests(serde_json::json!({
            "release": elicitation("Which channel?", &enum_schema()),
        }))))
        .expect("supported")
        .expect("a questionnaire");
        assert_eq!(plan.questionnaire().questions.len(), 1);
        assert_eq!(plan.questionnaire().questions[0].question, "Which channel?");
        assert_eq!(
            plan.questionnaire().questions[0]
                .options
                .iter()
                .map(|option| option.label.as_str())
                .collect::<Vec<_>>(),
            vec!["stable", "beta"]
        );
        let responses = plan
            .responses(&submitted(vec![single(0, "beta")]))
            .expect("responses");
        assert_eq!(
            responses,
            InputResponses::from([(
                "release".to_owned(),
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
        let responses = plan
            .responses(&submitted(vec![single(0, "stable"), single(1, "beta")]))
            .expect("responses");
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
        let plan = plan_round(Some(&requests(serde_json::json!({
            "release": elicitation("Which channel?", &enum_schema()),
        }))))
        .expect("supported")
        .expect("a questionnaire");
        let responses = plan
            .responses(&QuestionnaireResponse::Declined)
            .expect("responses");
        assert_eq!(
            responses["release"],
            serde_json::json!({"action": "decline"})
        );
    }

    /// An unanswered required property cannot produce a schema-valid accept,
    /// so the request declines rather than sending partial content.
    #[test]
    fn an_unanswered_required_property_declines_instead_of_sending_partial_content() {
        let plan = plan_round(Some(&requests(serde_json::json!({
            "release": elicitation("Which channel?", &enum_schema()),
        }))))
        .expect("supported")
        .expect("a questionnaire");
        let responses = plan.responses(&submitted(Vec::new())).expect("responses");
        assert_eq!(
            responses["release"],
            serde_json::json!({"action": "decline"})
        );
    }

    /// A free-text answer to a bounded schema choice is refused rather than
    /// coerced into an out-of-schema `accept`.
    #[test]
    fn a_custom_answer_to_a_bounded_choice_is_refused() {
        let plan = plan_round(Some(&requests(serde_json::json!({
            "release": elicitation("Which channel?", &enum_schema()),
        }))))
        .expect("supported")
        .expect("a questionnaire");
        let error = plan
            .responses(&submitted(vec![QuestionnaireAnswerEntry {
                question_index: 0,
                answer: QuestionnaireAnswer::Custom(CustomAnswer {
                    answer: "nightly".to_owned(),
                }),
            }]))
            .expect_err("a free-text answer is not a schema value");
        assert!(error.contains("declared schema values"), "{error}");
    }

    /// Booleans and multi-select enums round-trip to their exact JSON shapes.
    #[test]
    fn booleans_and_multi_select_enums_map_to_their_schema_values() {
        let plan = plan_round(Some(&requests(serde_json::json!({
            "options": elicitation(
                "Configure",
                &serde_json::json!({
                    "type": "object",
                    "properties": {
                        "notify": {"type": "boolean"},
                        "regions": {
                            "type": "array",
                            "items": {"type": "string", "enum": ["eu", "us"]},
                        },
                    },
                    "required": ["notify", "regions"],
                }),
            ),
        }))))
        .expect("supported")
        .expect("a questionnaire");
        assert_eq!(plan.questionnaire().questions.len(), 2);
        assert!(!plan.questionnaire().questions[0].multi_select);
        assert!(plan.questionnaire().questions[1].multi_select);
        let responses = plan
            .responses(&submitted(vec![
                single(0, "Yes"),
                QuestionnaireAnswerEntry {
                    question_index: 1,
                    answer: QuestionnaireAnswer::MultipleOption(MultipleOptionAnswer {
                        selected: vec!["eu".to_owned(), "us".to_owned()],
                    }),
                },
            ]))
            .expect("responses");
        assert_eq!(
            responses["options"],
            serde_json::json!({
                "action": "accept",
                "content": {"notify": true, "regions": ["eu", "us"]},
            })
        );
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

    /// Free-form strings and numbers have no faithful bounded rustX
    /// representation and are refused instead of coerced.
    #[test]
    fn free_form_scalar_properties_are_refused_rather_than_coerced() {
        for schema in [
            serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}}}),
            serde_json::json!({"type": "object", "properties": {"age": {"type": "integer"}}}),
        ] {
            let error = plan_round(Some(&requests(serde_json::json!({
                "ask": elicitation("Tell me", &schema),
            }))))
            .expect_err("free-form scalars are unsupported");
            assert!(error.diagnostic.contains("bounded"), "{error:?}");
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
        let error = plan_round(Some(&requests(serde_json::json!({
            "ask": elicitation(
                "Many",
                &serde_json::json!({"type": "object", "properties": properties}),
            ),
        }))))
        .expect_err("above the question bound");
        assert!(
            error.diagnostic.contains("bounded questionnaire"),
            "{error:?}"
        );
    }

    /// A schema whose choices collide with the client's own reserved rows is
    /// refused through the one shared questionnaire contract.
    #[test]
    fn schema_values_reserved_by_the_client_are_refused() {
        let error = plan_round(Some(&requests(serde_json::json!({
            "ask": elicitation(
                "Pick",
                &serde_json::json!({
                    "type": "object",
                    "properties": {"pick": {"type": "string", "enum": ["Other", "keep"]}},
                }),
            ),
        }))))
        .expect_err("reserved labels are refused");
        assert!(error.diagnostic.contains("reserved"), "{error:?}");
    }
}
