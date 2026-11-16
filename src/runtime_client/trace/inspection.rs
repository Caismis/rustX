//! Finite inspection heads. Original bodies remain behind their advertised readers.
use crate::durable::ConversationStoreError;
use crate::durable::observation::{TraceFactData, TraceFactHeader, TraceFactQuery, TraceFactScope};
use crate::observation::content::CanonicalContentLocator;
use crate::observation::cut::CollectionFrontier;
use crate::observation::trace::{
    TraceArtifact, TraceFieldSource, TraceKind, TracePreview, TraceRecord,
};
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
    Compaction {
        page: TraceInspectionCompactionCalls,
    },
    Tool {
        definition: TraceToolDefinitionSelection,
    },
    Messages {
        page: TraceInspectionMessages,
    },
    Workflow {
        run_id: crate::runtime::workflow::WorkflowRunId,
        workflow_id: crate::runtime::workflow::WorkflowId,
    },
}

/// An operation-owned page; continuation is an exclusive Journal coordinate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceInspectionCompactionCalls {
    pub domain: String,
    pub total: u64,
    pub items: Vec<TraceInspectionCompactionCall>,
    pub next_offset: Option<u64>,
}

/// A Call's exact native causality; physical requests have their own page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceInspectionCompactionCall {
    pub call_id: crate::runtime::identity::AuxiliaryCallId,
    pub started_sequence: u64,
    pub settled_sequence: Option<u64>,
    pub outcome: Option<crate::model::auxiliary::AuxiliaryCallOutcome>,
    pub preceding_call: Option<crate::model::auxiliary::AuxiliaryCallRef>,
    pub previous_summary: Option<crate::context::checkpoint::SummaryGenerationRef>,
    pub strategy: Option<crate::model::structured::StructuredOutputStrategy>,
    pub requests_domain: String,
    pub request_count: u64,
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
    pub context_kind: Option<crate::observation::trace::TraceContextKind>,
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
            TraceFactData::Adoption { .. } | TraceFactData::AssistantCommit { .. } => {
                TraceInspectionContent::Messages {
                    page: self.inspection_messages(anchor.sequence, 0, 8)?,
                }
            }
            TraceFactData::WorkflowStart { .. } => {
                let nodes =
                    self.store
                        .observation_workflow_nodes(self.cut()?, anchor.sequence, 0, 0)?;
                TraceInspectionContent::Workflow {
                    run_id: nodes.run_id,
                    workflow_id: nodes.workflow_id,
                }
            }
            TraceFactData::CompactionStart {
                native_id: Some(_), ..
            } => TraceInspectionContent::Compaction {
                page: self.inspection_compaction_calls(anchor.sequence, 0, 4)?,
            },
            TraceFactData::CompactionStart {
                native_id: None, ..
            }
            | TraceFactData::AttemptStart
            | TraceFactData::StepStart
            | TraceFactData::BackgroundStart { .. }
            | TraceFactData::SubagentStart { .. }
            | TraceFactData::InteractionStart { .. }
            | TraceFactData::Configuration { .. } => TraceInspectionContent::Lifecycle,
            TraceFactData::RequestStart { .. }
            | TraceFactData::CanonicalToolCommit { .. }
            | TraceFactData::CompactionStatus { .. }
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

    fn compaction_operation(
        &self,
        sequence: u64,
    ) -> Result<crate::runtime::identity::CompactionOperationId, ConversationStoreError> {
        let anchor = self.inspection_anchor(sequence)?;
        match anchor.fact {
            TraceFactData::CompactionStart {
                native_id: Some(id),
                ..
            } => Ok(crate::runtime::identity::CompactionOperationId::new(id)),
            _ => Err(ConversationStoreError::InvalidReference(
                "Trace anchor has no bounded Compaction identity".into(),
            )),
        }
    }

    pub(crate) fn inspection_compaction_calls(
        &self,
        sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<TraceInspectionCompactionCalls, ConversationStoreError> {
        let operation = self.compaction_operation(sequence)?;
        let cut = self.cut()?;
        let source = self.store.observation_compaction_call_ids(
            cut,
            &operation,
            (offset > 0).then_some(offset),
            limit.min(8),
        )?;
        let audit_cut = crate::model::auxiliary::AuxiliaryAuditCut {
            journal: cut.read.journal,
        };
        let mut items = Vec::with_capacity(source.items.len());
        for (started_sequence, call_id) in source.items {
            let call = self
                .store
                .load_auxiliary_call(&call_id, audit_cut)?
                .filter(|call| call.started_sequence == started_sequence)
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "Compaction Call audit missing at cut".into(),
                    )
                })?;
            let preceding_call = match call.business_source {
                crate::model::auxiliary::AuxiliaryBusinessSource::Compaction {
                    operation_id,
                    preceding_call,
                } if operation_id == operation => preceding_call,
                _ => {
                    return Err(ConversationStoreError::InvalidReference(
                        "foreign Compaction Call".into(),
                    ));
                }
            };
            let (previous_summary, strategy) = match call.input {
                crate::model::auxiliary::AuxiliaryInputEvidence::Compaction {
                    previous_summary,
                    output_strategy,
                    ..
                } => (previous_summary, Some(output_strategy)),
                _ => (None, None),
            };
            let requests = self
                .store
                .read_auxiliary_call_requests(&call_id, audit_cut, None, 3)?;
            items.push(TraceInspectionCompactionCall {
                requests_domain: format!(
                    "trace.records.{sequence}.detail.calls.{call_id}.requests"
                ),
                request_count: requests.len() as u64,
                call_id,
                started_sequence,
                settled_sequence: call.settled_sequence,
                outcome: call.outcome,
                preceding_call,
                previous_summary,
                strategy,
            });
        }
        Ok(TraceInspectionCompactionCalls {
            domain: format!("trace.records.{sequence}.detail.calls"),
            total: source.total,
            items,
            next_offset: source.next_offset,
        })
    }

    pub(crate) fn inspection_compaction_requests(
        &self,
        sequence: u64,
        call_id: &crate::runtime::identity::AuxiliaryCallId,
    ) -> Result<Vec<crate::model::auxiliary::AuxiliaryRequestAudit>, ConversationStoreError> {
        let operation = self.compaction_operation(sequence)?;
        let audit_cut = crate::model::auxiliary::AuxiliaryAuditCut {
            journal: self.cut()?.read.journal,
        };
        let call = self
            .store
            .load_auxiliary_call(call_id, audit_cut)?
            .ok_or_else(|| {
                ConversationStoreError::InvalidReference("Compaction Call is absent at cut".into())
            })?;
        if !matches!(call.business_source, crate::model::auxiliary::AuxiliaryBusinessSource::Compaction { operation_id, .. } if operation_id == operation)
        {
            return Err(ConversationStoreError::InvalidReference(
                "foreign Compaction request Call".into(),
            ));
        }
        self.store
            .read_auxiliary_call_requests(call_id, audit_cut, None, 3)
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
