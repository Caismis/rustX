//! Finite inspection heads. Original bodies remain behind their advertised readers.
use super::{TraceArtifact, TraceFieldSource, TraceKind, TracePreview, TraceRecord};
use crate::durable::ConversationStoreError;
use crate::durable::observation::{TraceFactData, TraceFactHeader, TraceFactQuery, TraceFactScope};
use crate::runtime_client::checkpoint::CollectionFrontier;
use crate::runtime_client::content::CanonicalContentLocator;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A selected non-Request record's finite inspection evidence, never a complete body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceInspectionHead {
    pub id: String,
    pub kind: TraceKind,
    pub record: TraceRecord,
    pub content: TraceInspectionContent,
}

/// The closed native inspection families. Request bodies use their existing indexed layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TraceInspectionContent {
    Lifecycle,
    Tool {
        definition: TraceToolDefinitionSelection,
    },
    Messages {
        page: TraceInspectionMessages,
    },
}

/// Absence is proved by the original catalog; unavailable means its relationship cannot be read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TraceToolDefinitionSelection {
    Available { field: TraceFieldSource },
    Absent,
    Unavailable,
}

/// A bounded prefix of the selected original adoption, Assistant or Compaction's canonical rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceInspectionMessages {
    pub domain: String,
    pub total: u64,
    pub items: Vec<TraceInspectionMessage>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
}

/// Original C identity, finite presentation and authoritative per-field/per-block read evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceInspectionMessage {
    pub locator: CanonicalContentLocator,
    pub identity: Option<crate::runtime::identity::MessageId>,
    pub preview: Option<TracePreview>,
    pub source: Option<crate::message::types::UserSource>,
    pub ordinary_input: bool,
    pub context_kind: Option<super::TraceContextKind>,
    pub attachments: Vec<TraceArtifact>,
    pub fields: Vec<TraceFieldSource>,
    pub collections: Vec<CollectionFrontier>,
}

impl From<crate::durable::observation::TraceCanonicalSummary> for TraceInspectionMessage {
    fn from(value: crate::durable::observation::TraceCanonicalSummary) -> Self {
        Self {
            locator: value.locator,
            identity: value.identity,
            preview: value.preview,
            source: value.source,
            ordinary_input: value.ordinary_input,
            context_kind: value.context_kind,
            attachments: value.attachments,
            fields: value.fields,
            collections: value.collections,
        }
    }
}

impl super::TraceProjection<'_> {
    pub(crate) fn inspection_anchor(
        &self,
        sequence: u64,
    ) -> Result<TraceFactHeader, ConversationStoreError> {
        let before = sequence
            .checked_add(1)
            .filter(|_| sequence > 0)
            .ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "invalid native Trace inspection anchor".into(),
                )
            })?;
        self.store
            .observation_trace_facts(
                self.cut()?,
                &TraceFactQuery {
                    scope: TraceFactScope::All,
                    kinds: super::anchor_kinds(),
                    before: Some(before),
                    after: sequence - 1,
                    ascending: true,
                    limit: 1,
                },
            )?
            .pop()
            .filter(|header| header.sequence == sequence)
            .ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "native Trace inspection anchor absent at cut".into(),
                )
            })
    }

    pub(crate) fn inspection_head(
        &self,
        anchor: &TraceFactHeader,
    ) -> Result<TraceInspectionHead, ConversationStoreError> {
        let mut record = self.record(anchor)?;
        super::record::bound_record(&mut record);
        let content = match &anchor.fact {
            TraceFactData::ToolStart { occurrence, .. } => TraceInspectionContent::Tool {
                definition: match occurrence {
                    Some(occurrence) => match self
                        .store
                        .observation_trace_tool_definition(self.cut()?, occurrence)
                    {
                        Ok(Some(field)) => TraceToolDefinitionSelection::Available { field },
                        Ok(None) => TraceToolDefinitionSelection::Absent,
                        Err(
                            ConversationStoreError::Storage(_)
                            | ConversationStoreError::ObservationCapacityExceeded,
                        ) => TraceToolDefinitionSelection::Unavailable,
                        Err(error) => return Err(error),
                    },
                    None => TraceToolDefinitionSelection::Unavailable,
                },
            },
            TraceFactData::Adoption { .. }
            | TraceFactData::AssistantCommit { .. }
            | TraceFactData::CompactionStart => TraceInspectionContent::Messages {
                page: self.inspection_messages(anchor.sequence, 0, 8)?,
            },
            TraceFactData::AttemptStart
            | TraceFactData::StepStart
            | TraceFactData::BackgroundStart { .. }
            | TraceFactData::SubagentStart { .. }
            | TraceFactData::WorkflowStart { .. }
            | TraceFactData::InteractionStart { .. } => TraceInspectionContent::Lifecycle,
            TraceFactData::RequestStart { .. }
            | TraceFactData::CanonicalToolCommit { .. }
            | TraceFactData::Terminal(_) => {
                return Err(ConversationStoreError::InvalidReference(
                    "native fact has a different inspection source".into(),
                ));
            }
        };
        Ok(TraceInspectionHead {
            id: record.id.clone(),
            kind: record.kind,
            record,
            content,
        })
    }

    pub(crate) fn inspection_messages(
        &self,
        sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<TraceInspectionMessages, ConversationStoreError> {
        let anchor = self.inspection_anchor(sequence)?;
        let (total, items, next_offset) = match anchor.fact {
            TraceFactData::Adoption { .. } => {
                let page =
                    self.store
                        .observation_trace_adoption(self.cut()?, sequence, offset, limit)?;
                (page.total, page.items, page.next_offset)
            }
            TraceFactData::AssistantCommit { ledger_position } => {
                self.inspection_canonical(Some(ledger_position), offset)?
            }
            TraceFactData::CompactionStart => {
                let terminal = self.ending(
                    sequence,
                    &[
                        crate::durable::observation::TraceFactKind::CompactionCompleted,
                        crate::durable::observation::TraceFactKind::CompactionFailed,
                    ],
                )?;
                let position = terminal.and_then(|terminal| match terminal.fact {
                    TraceFactData::Terminal(terminal) => terminal.summary_ledger_position,
                    _ => None,
                });
                self.inspection_canonical(position, offset)?
            }
            _ => {
                return Err(ConversationStoreError::InvalidReference(
                    "native Trace anchor has no canonical message collection".into(),
                ));
            }
        };
        Ok(TraceInspectionMessages {
            domain: format!("trace.records.{sequence}.detail.messages"),
            total,
            items: items
                .into_iter()
                .map(TraceInspectionMessage::from)
                .collect(),
            next_offset,
        })
    }

    fn inspection_canonical(
        &self,
        position: Option<u64>,
        offset: u64,
    ) -> Result<
        (
            u64,
            Vec<crate::durable::observation::TraceCanonicalSummary>,
            Option<u64>,
        ),
        ConversationStoreError,
    > {
        let total = u64::from(position.is_some());
        let items = if offset == 0 {
            position
                .map(|position| {
                    self.store
                        .observation_trace_canonical_summary(self.cut()?, position)
                })
                .transpose()?
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };
        Ok((total, items, None))
    }
}
