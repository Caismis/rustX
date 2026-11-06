//! Borrowed allowlisted bodies of actual noncanonical frozen request items.
//! Original canonical items keep their existing canonical byte bindings.
use crate::message::types::{AssistantContentBlock, UserContentBlock};
use crate::model::ModelInputMessage;
use crate::model::projected::{DerivedModelMessage, ProjectedMessage};
use crate::observation::trace::{TraceRequestMessage, TraceToolOutcome};
use serde::Serialize;

pub(super) struct Input<'a> {
    pub(super) original: &'a ModelInputMessage,
    pub(super) view: &'a TraceRequestMessage,
}
impl Serialize for Input<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;
        let mut value = serializer.serialize_struct("TraceRequestMessage", 8)?;
        value.serialize_field("request_occurrence", &self.view.request_occurrence)?;
        match self.original {
            ModelInputMessage::Derived(original) => {
                value.serialize_field("provenance", &Provenance::of(original))?;
            }
            _ => value.serialize_field("provenance", &self.view.provenance)?,
        }
        value.serialize_field("provenance_source", &self.view.provenance_source)?;
        value.serialize_field("role", &self.view.role)?;
        value.serialize_field("message_id", &self.view.message_id)?;
        value.serialize_field("source", &self.view.source)?;
        value.serialize_field("blocks", &Blocks(self.original))?;
        value.serialize_field("truncated", &false)?;
        value.end()
    }
}
#[derive(Serialize)]
struct Provenance<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    sources: &'a [crate::model::projected::CanonicalMessageRef],
    checkpoint_sources: &'a [crate::model::projected::CheckpointSourceRef],
    transformations: &'a [crate::model::projected::SemanticTransformation],
    truncated: bool,
}
impl<'a> Provenance<'a> {
    fn of(original: &'a DerivedModelMessage) -> Self {
        Self {
            kind: "derived",
            sources: &original.sources,
            checkpoint_sources: &original.checkpoint_sources,
            transformations: &original.transformations,
            truncated: false,
        }
    }
}
#[derive(Serialize)]
struct Text<'a> {
    text: &'a str,
    truncated: bool,
}
#[derive(Serialize)]
struct Json<'a> {
    value: &'a serde_json::Value,
    truncated: bool,
}
#[derive(Serialize)]
struct ArtifactView<'a> {
    artifact_id: &'a crate::runtime::ArtifactId,
    image: bool,
    name: Option<&'a str>,
    mime_type: Option<&'a str>,
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block<'a> {
    Text {
        text: Text<'a>,
    },
    Reasoning {
        text: Text<'a>,
    },
    Refusal {
        text: Text<'a>,
    },
    Json {
        value: Json<'a>,
    },
    ToolCall {
        call_id: &'a crate::runtime::ToolCallId,
        tool_id: &'a crate::runtime::ToolId,
        name: &'a str,
        arguments: Json<'a>,
    },
    ToolResult {
        call_id: &'a crate::runtime::ToolCallId,
        tool_id: &'a crate::runtime::ToolId,
        outcome: TraceToolOutcome,
        blocks: ResultBlocks<'a>,
        truncated: bool,
    },
    Image {
        artifact: ArtifactView<'a>,
        alt: Option<&'a str>,
    },
    File {
        artifact: ArtifactView<'a>,
    },
    Upload {
        name: &'a str,
    },
}
fn text(value: &str) -> Text<'_> {
    Text {
        text: value,
        truncated: false,
    }
}
fn image(value: &crate::message::content::ImageReference) -> Block<'_> {
    Block::Image {
        artifact: ArtifactView {
            artifact_id: &value.artifact_id,
            image: true,
            name: None,
            mime_type: None,
        },
        alt: value.alt.as_deref(),
    }
}
fn file(value: &crate::message::content::FileReference) -> Block<'_> {
    Block::File {
        artifact: ArtifactView {
            artifact_id: &value.artifact_id,
            image: false,
            name: value.name.as_deref(),
            mime_type: value.mime_type.as_deref(),
        },
    }
}
struct Blocks<'a>(&'a ModelInputMessage);
impl Serialize for Blocks<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error as _, SerializeSeq as _};
        let mut seq = serializer.serialize_seq(None)?;
        match self.0 {
            ModelInputMessage::Derived(value) => match &value.message {
                ProjectedMessage::User { content, .. } => {
                    for block in content {
                        let block = match block {
                            UserContentBlock::Text(value) => Block::Text {
                                text: text(&value.text),
                            },
                            UserContentBlock::Image(value) => image(value),
                            UserContentBlock::File(value) => file(value),
                            UserContentBlock::UploadedFile(value) => {
                                Block::Upload { name: &value.name }
                            }
                        };
                        seq.serialize_element(&block)?;
                    }
                }
                ProjectedMessage::Assistant { content } => {
                    for block in content {
                        let block = match block {
                            AssistantContentBlock::Text(value) => Block::Text {
                                text: text(&value.text),
                            },
                            AssistantContentBlock::Reasoning(value) => {
                                let Some(value) = value.text.as_deref() else {
                                    continue;
                                };
                                Block::Reasoning { text: text(value) }
                            }
                            AssistantContentBlock::Refusal(value) => Block::Refusal {
                                text: text(&value.text),
                            },
                            AssistantContentBlock::ToolCall(value) => Block::ToolCall {
                                call_id: &value.id,
                                tool_id: &value.tool_id,
                                name: &value.name,
                                arguments: Json {
                                    value: &value.arguments,
                                    truncated: false,
                                },
                            },
                            AssistantContentBlock::Image(value) => image(value),
                        };
                        seq.serialize_element(&block)?;
                    }
                }
                ProjectedMessage::Tool {
                    tool_call_id,
                    tool_id,
                    result,
                } => {
                    use crate::tools::types::ToolExecutionStatus as T;
                    let outcome = match result.status {
                        T::Success => TraceToolOutcome::Success,
                        T::Failed { .. } => TraceToolOutcome::Failed,
                        T::Denied { .. } => TraceToolOutcome::Denied,
                        T::Cancelled { .. } => TraceToolOutcome::Cancelled,
                        T::TimedOut => TraceToolOutcome::TimedOut,
                        T::OutcomeUnknown { .. } => TraceToolOutcome::OutcomeUnknown,
                    };
                    seq.serialize_element(&Block::ToolResult {
                        call_id: tool_call_id,
                        tool_id,
                        outcome,
                        blocks: ResultBlocks(&result.content),
                        truncated: false,
                    })?;
                }
            },
            ModelInputMessage::RequestOnly(value) => seq.serialize_element(&Block::Text {
                text: text(&value.render()),
            })?,
            ModelInputMessage::Canonical(_) => {
                return Err(S::Error::custom(
                    "canonical item requires original canonical bindings",
                ));
            }
        }
        seq.end()
    }
}
struct ResultBlocks<'a>(&'a [crate::tools::types::ToolResultContent]);
impl Serialize for ResultBlocks<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq as _;
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for value in self.0 {
            use crate::tools::types::ToolResultContent as T;
            let block = match value {
                T::Text(value) => Block::Text {
                    text: text(&value.text),
                },
                T::Json { value } => Block::Json {
                    value: Json {
                        value,
                        truncated: false,
                    },
                },
                T::Image(value) => image(value),
                T::File(value) => file(value),
            };
            seq.serialize_element(&block)?;
        }
        seq.end()
    }
}
