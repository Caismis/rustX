//! Immutable allowlisted Trace request source encoding at the native request commit.
//! Reads later use indexed byte ranges; no observation read reconstructs the Surface.
use super::{
    TraceMessageRole, TraceRequestDetail, TraceRequestMessage, TraceRequestPredecessor, TraceText,
};
use crate::model::RequestSnapshot;
use crate::model::input::ModelInputMessage;
pub(crate) fn canonical_message(
    header: &crate::durable::observation::MessageHeader,
) -> TraceRequestMessage {
    let mut value = match &header.display {
        Some(value) => {
            super::request::request_message(&ModelInputMessage::Canonical(value.clone()))
        }
        None => TraceRequestMessage {
            role: match header.role.as_str() {
                "user" => TraceMessageRole::User,
                "tool" => TraceMessageRole::Tool,
                _ => TraceMessageRole::Assistant,
            },
            message_id: header.identity.clone(),
            source: header.source_label.clone(),
            blocks: Vec::new(),
            truncated: true,
        },
    };
    value.truncated |= header.deferred;
    value
}
/// This positive allowlist is shared with Trace display semantics. Provider
/// continuation, private configuration and nonallowlisted request options
/// never acquire a public read index.
pub(crate) struct RequestInspection<'a, M> {
    pub snapshot: &'a RequestSnapshot,
    pub messages: M,
}
impl<M> schemars::JsonSchema for RequestInspection<'_, M> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        TraceRequestDetail::schema_name()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        TraceRequestDetail::json_schema(generator)
    }
}
impl<M: serde::Serialize> serde::Serialize for RequestInspection<'_, M> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;
        let snapshot = self.snapshot;
        let options = Options(snapshot);
        let mut value = serializer.serialize_struct("TraceRequestDetail", 27)?;
        value.serialize_field("contributions", &Contributions(snapshot))?;
        value.serialize_field("request_id", &snapshot.request_id)?;
        value.serialize_field("attempt_id", &snapshot.identity.attempt_id)?;
        value.serialize_field("step_id", &snapshot.identity.turn)?;
        value.serialize_field("retry_number", &snapshot.identity.retry_number)?;
        value.serialize_field("assistant_message_id", &snapshot.provisional_message_id)?;
        value.serialize_field("model", &snapshot.invocation.model)?;
        value.serialize_field(
            "protocol",
            super::request::protocol_label(snapshot.invocation.protocol),
        )?;
        value.serialize_field("max_output_tokens", &snapshot.invocation.max_output_tokens)?;
        value.serialize_field("context_window_tokens", &snapshot.context_window_tokens)?;
        value.serialize_field("reasoning_enabled", &snapshot.reasoning_enabled)?;
        value.serialize_field(
            "profile",
            &snapshot
                .profile
                .as_ref()
                .map(crate::model::catalog::ModelProfileId::as_str),
        )?;
        value.serialize_field("options", &options)?;
        value.serialize_field(
            "omitted_option_count",
            &(snapshot.invocation.request_params.len() - options.count()),
        )?;
        value.serialize_field(
            "effective_system_prompt",
            &BorrowedText {
                text: &snapshot.effective_system_prompt,
                truncated: false,
            },
        )?;
        value.serialize_field("predecessor", &TraceRequestPredecessor::NotApplicable)?;
        value.serialize_field("previous_system_prompt", &None::<TraceText>)?;
        value.serialize_field("messages", &self.messages)?;
        value.serialize_field("messages_truncated", &true)?;
        value.serialize_field("tools", &Tools(snapshot))?;
        value.serialize_field("tools_truncated", &false)?;
        value.serialize_field("usage", &None::<crate::model::ModelUsage>)?;
        value.serialize_field("failure", &None::<super::TraceRequestFailure>)?;
        value.serialize_field("generation", &None::<super::TraceGeneration>)?;
        value.end()
    }
}
#[derive(serde::Serialize, schemars::JsonSchema)]
struct BorrowedText<'a> {
    text: &'a str,
    truncated: bool,
}
#[derive(serde::Serialize)]
struct BorrowedJson<'a> {
    value: &'a serde_json::Value,
    truncated: bool,
}
struct Options<'a>(&'a RequestSnapshot);
impl Options<'_> {
    fn count(&self) -> usize {
        self.0
            .invocation
            .request_params
            .keys()
            .filter(|name| {
                crate::model::inspection::REQUEST_OPTION_ALLOWLIST.contains(&name.as_str())
            })
            .count()
    }
}
impl serde::Serialize for Options<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq as _;
        #[derive(serde::Serialize)]
        struct OptionView<'a> {
            name: &'a str,
            value: BorrowedJson<'a>,
        }
        let mut sequence = s.serialize_seq(Some(self.count()))?;
        for (name, value) in &self.0.invocation.request_params {
            if crate::model::inspection::REQUEST_OPTION_ALLOWLIST.contains(&name.as_str()) {
                sequence.serialize_element(&OptionView {
                    name,
                    value: BorrowedJson {
                        value,
                        truncated: false,
                    },
                })?;
            }
        }
        sequence.end()
    }
}
struct Tools<'a>(&'a RequestSnapshot);
impl serde::Serialize for Tools<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq as _;
        #[derive(serde::Serialize)]
        struct ToolView<'a> {
            tool_id: &'a crate::runtime::identity::ToolId,
            name: &'a str,
            description: BorrowedText<'a>,
            input_schema: BorrowedJson<'a>,
        }
        let mut sequence = s.serialize_seq(Some(self.0.tool_definitions.len()))?;
        for tool in &self.0.tool_definitions {
            sequence.serialize_element(&ToolView {
                tool_id: &tool.id,
                name: &tool.name,
                description: BorrowedText {
                    text: &tool.description,
                    truncated: false,
                },
                input_schema: BorrowedJson {
                    value: &tool.input_schema,
                    truncated: false,
                },
            })?;
        }
        sequence.end()
    }
}
struct Contributions<'a>(&'a RequestSnapshot);
impl serde::Serialize for Contributions<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error as _, SerializeSeq as _};
        #[derive(serde::Serialize)]
        struct ContributionView<'a> {
            message_id: &'a crate::runtime::identity::MessageId,
            producer: &'a crate::runtime::identity::ContextContributorIdentity,
            metadata: &'a crate::message::ContextKind,
            presentation: Option<super::types::TraceContributionPresentation>,
        }
        let mut sequence = s.serialize_seq(Some(self.0.contributions.len()))?;
        for record in &self.0.contributions {
            let presentation = match &record.presentation {
                Some(crate::context::contribution::ContributionPresentation::AgentStatus(
                    status,
                )) => Some(super::types::TraceContributionPresentation::AgentStatus(
                    crate::runtime_client::projection::prepared_status(
                        &self.0.identity.attempt_id,
                        self.0
                            .identity
                            .turn
                            .as_str()
                            .parse()
                            .map_err(S::Error::custom)?,
                        &record.message_id,
                        &record.opportunities,
                        record.post_tool_batch_anchor,
                        status,
                        None,
                    ),
                )),
                None => None,
            };
            sequence.serialize_element(&ContributionView {
                message_id: &record.message_id,
                producer: &record.producer,
                metadata: &record.metadata,
                presentation,
            })?;
        }
        sequence.end()
    }
}
pub(crate) fn request_detail<M>(
    snapshot: &RequestSnapshot,
    messages: M,
) -> RequestInspection<'_, M> {
    RequestInspection { snapshot, messages }
}

#[derive(serde::Serialize, schemars::JsonSchema)]
pub(crate) struct BorrowedFailure<'a> {
    kind: &'a crate::model::ModelErrorKind,
    message: BorrowedText<'a>,
}
impl BorrowedFailure<'_> {
    pub(crate) fn message(&self) -> &str {
        self.message.text
    }
    pub(crate) fn header(&self) -> super::TraceRequestFailure {
        super::TraceRequestFailure {
            kind: self.kind.clone(),
            message: TraceText {
                text: self.message.text.chars().take(256).collect(),
                truncated: self.message.text.chars().nth(256).is_some(),
            },
        }
    }
}
#[derive(serde::Serialize, schemars::JsonSchema)]
pub(crate) struct RequestOutcomeSource<'a> {
    pub usage: &'a Option<crate::model::ModelUsage>,
    pub failure: Option<BorrowedFailure<'a>>,
    pub generation: Option<super::TraceGeneration>,
}
pub(crate) fn request_outcome(
    event: &crate::events::types::RuntimeEvent,
) -> Option<RequestOutcomeSource<'_>> {
    use crate::events::types::RuntimeEvent;
    let (usage, generation, failure) = match event {
        RuntimeEvent::ModelRequestCompleted {
            usage, generation, ..
        } => (usage, generation, None),
        RuntimeEvent::ModelRequestFailed {
            usage,
            generation,
            error,
            ..
        } => (
            usage,
            generation,
            Some(BorrowedFailure {
                kind: &error.kind,
                message: BorrowedText {
                    text: &error.message,
                    truncated: false,
                },
            }),
        ),
        _ => return None,
    };
    Some(RequestOutcomeSource {
        usage,
        failure,
        generation: generation
            .map(|value| super::record::generation_metrics(value, usage.as_ref())),
    })
}
