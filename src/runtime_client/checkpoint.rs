//! Bounded native observation cuts. Capture copies finite display descriptors;
//! source-owned bytes are materialized only after releasing the Projection lock.
use super::snapshot::RuntimeClientSnapshot;
use super::types::RuntimeClientCursor;
use crate::observation::bounded::{
    CHECKPOINT_ITEM_BYTES, CHECKPOINT_PAGE_ITEMS, CHECKPOINT_TARGET_BYTES, bounded_bytes,
    bounded_item_bytes, bounded_json_value,
};
use crate::observation::content::ContentRef;
use crate::observation::cut::{
    CheckpointPageItem, CheckpointReadCut, CollectionFrontier, ContentBinding,
};
use serde::{Deserialize, Serialize};

mod prepared_trace;
pub(super) use prepared_trace::PreparedTraceHead;

/// Each indexed native row includes bodies and locator metadata within16KiB.
/// Separate8-row current heads reserve256KiB before other domains/annotations.
pub(crate) const CHECKPOINT_NATIVE_HEAD_ITEMS: usize = 8;
pub const CHECKPOINT_ATTEMPTS: usize = 2;
pub const CHECKPOINT_MATERIALIZATION_SECONDS: u64 = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectionMetadata {
    pub read_cut: CheckpointReadCut,
    pub collections: Vec<CollectionFrontier>,
    pub content: Vec<ContentBinding>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientCheckpoint {
    pub snapshot: RuntimeClientSnapshot,
    pub cursor: RuntimeClientCursor,
    pub read_cut: CheckpointReadCut,
    pub content: Vec<ContentBinding>,
    pub collections: Vec<CollectionFrontier>,
}
/// Final complete checkpoint budget, after off-lock source enrichment. Only
/// loaded read caches are evicted; authoritative cuts, descriptors, current
/// execution and interaction authority remain in the candidate.
pub(crate) fn fit_checkpoint(
    checkpoint: &mut RuntimeClientCheckpoint,
) -> Result<bool, super::types::RuntimeClientError> {
    fn fits(checkpoint: &RuntimeClientCheckpoint) -> bool {
        let schema = crate::observation::structural_json::root::<RuntimeClientCheckpoint>();
        let encoded = crate::observation::structural_json::Encoded {
            value: checkpoint,
            schema: &schema,
            root: &schema,
        };
        let limit = CHECKPOINT_TARGET_BYTES - CHECKPOINT_ITEM_BYTES;
        crate::observation::preflight::bounded_fit(&encoded, limit)
            .unwrap_or_else(|| bounded_bytes(&encoded, limit).is_some())
    }
    fn loaded(
        checkpoint: &mut RuntimeClientCheckpoint,
        domain: &str,
        count: u64,
        next: Option<u64>,
    ) {
        if let Some(frontier) = checkpoint
            .collections
            .iter_mut()
            .find(|frontier| frontier.domain == domain)
        {
            frontier.loaded = count;
            frontier.next_offset = next;
        }
    }
    checkpoint
        .collections
        .sort_by(|left, right| left.domain.cmp(&right.domain));
    checkpoint
        .content
        .sort_by(|left, right| left.path.cmp(&right.path));
    if fits(checkpoint) {
        return Ok(false);
    }
    defer_pending_bodies(checkpoint);
    if fits(checkpoint) {
        return Ok(true);
    }
    checkpoint.snapshot.trace.records.clear();
    checkpoint.snapshot.trace.updates.clear();
    for domain in ["trace.records", "trace.updates"] {
        loaded(checkpoint, domain, 0, Some(0));
    }
    if fits(checkpoint) {
        return Ok(true);
    }
    checkpoint.snapshot.messages.clear();
    checkpoint.content.retain(|binding| {
        binding
            .path
            .first()
            .is_none_or(|domain| domain != "messages")
    });
    loaded(checkpoint, "messages", 0, Some(0));
    if fits(checkpoint) {
        return Ok(true);
    }
    checkpoint.snapshot.transcript.entries.clear();
    checkpoint.snapshot.transcript.content.retain(|binding| {
        binding
            .path
            .first()
            .is_none_or(|domain| domain != "entries")
    });
    checkpoint
        .snapshot
        .transcript
        .collections
        .retain(|frontier| !frontier.domain.starts_with("transcript.entries."));
    checkpoint.content.retain(|binding| {
        binding
            .path
            .first()
            .is_none_or(|domain| domain != "transcript")
            || binding.path.get(1).is_none_or(|domain| domain != "entries")
    });
    // Zero means the newest page at this captured cut. A positive offset is
    // an exclusive transcript cursor and would skip the newest evicted item.
    loaded(checkpoint, "transcript.entries", 0, Some(0));
    if fits(checkpoint) {
        return Ok(true);
    }
    Err(super::types::RuntimeClientError::ObservationUnavailable {
        message: "finite authoritative checkpoint descriptors exceed the native checkpoint budget"
            .into(),
    })
}

/// Evict optional body caches while retaining every native decision header.
fn defer_pending_bodies(checkpoint: &mut RuntimeClientCheckpoint) {
    // Headers keep every actionable identity and native source ordinal. Only
    // the optional loaded request body is evicted; the exact captured domain
    // remains readable with the same interaction/cut evidence.
    let mut deferred = Vec::new();
    for (index, interaction) in checkpoint
        .snapshot
        .pending_interactions
        .iter_mut()
        .enumerate()
    {
        if interaction.body.take().is_some() {
            interaction.body_complete = false;
            deferred.push(format!("pending_interactions.{index}.body"));
        }
    }
    for domain in deferred {
        if let Some(frontier) = checkpoint
            .collections
            .iter_mut()
            .find(|frontier| frontier.domain == domain)
        {
            frontier.loaded = 0;
            frontier.next_offset = Some(0);
        } else {
            checkpoint.collections.push(CollectionFrontier {
                domain,
                revision: checkpoint
                    .read_cut
                    .domain_revisions
                    .get("pending_interactions")
                    .copied()
                    .unwrap_or(checkpoint.read_cut.revision),
                total: 1,
                loaded: 0,
                next_offset: Some(0),
            });
        }
    }
    checkpoint
        .collections
        .sort_by(|left, right| left.domain.cmp(&right.domain));
    for binding in &mut checkpoint.content {
        if binding
            .path
            .first()
            .is_some_and(|domain| domain == "pending_interactions")
            && binding.path.get(2).is_some_and(|field| field == "body")
        {
            binding.deferred = true;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointPage {
    /// Nested advancing domain frontiers represented by this exact page cut.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collections: Vec<CollectionFrontier>,
    pub cursor: RuntimeClientCursor,
    pub read_cut: CheckpointReadCut,
    pub domain: String,
    pub total: u64,
    pub items: Vec<CheckpointPageItem>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointValueRepresentation {
    Utf8Text,
    CanonicalJsonV1,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointValueChunk {
    pub representation: CheckpointValueRepresentation,
    pub cut: CheckpointReadCut,
    pub domain: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub item_offset: u64,
    pub path: Vec<String>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub offset: u64,
    pub bytes_base64: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub readable_end: u64,
}

pub(crate) fn page<T: Serialize + schemars::JsonSchema>(
    domain: &str,
    cut: CheckpointReadCut,
    cursor: RuntimeClientCursor,
    items: &[T],
    offset: u64,
    limit: usize,
    identity: impl Fn(&T) -> String,
) -> Result<CheckpointPage, super::types::RuntimeClientError> {
    if limit == 0 || limit > CHECKPOINT_PAGE_ITEMS || offset > items.len() as u64 {
        return Err(super::types::RuntimeClientError::InvalidRequest {
            message: "invalid checkpoint page bounds".into(),
        });
    }
    let start =
        usize::try_from(offset).map_err(|_| super::types::RuntimeClientError::InvalidRequest {
            message: "invalid page offset".into(),
        })?;
    let mut page = Vec::new();
    let mut bytes = 0;
    for (index, item) in items[start..].iter().enumerate().take(limit) {
        let root = crate::observation::structural_json::root::<T>();
        let encoded = bounded_item_bytes(
            &crate::observation::structural_json::Encoded {
                value: item,
                schema: &root,
                root: &root,
            },
            CHECKPOINT_ITEM_BYTES,
        );
        let weight = encoded.as_ref().map_or(512, Vec::len);
        if bytes + weight > CHECKPOINT_TARGET_BYTES / 2 {
            break;
        }
        bytes += weight;
        let value = encoded.and_then(|bytes| serde_json::from_slice(&bytes).ok());
        page.push(CheckpointPageItem {
            body_complete: None,
            key: None,
            offset: (start + index) as u64,
            identity: identity(item),
            deferred: value.is_none(),
            value,
            content: Vec::new(),
        });
    }
    let next = offset + page.len() as u64;
    Ok(CheckpointPage {
        collections: Vec::new(),
        cursor,
        read_cut: cut,
        domain: domain.into(),
        total: items.len() as u64,
        items: page,
        next_offset: (next < items.len() as u64).then_some(next),
    })
}

/// Fixed-size source metadata is fetched before the Projection lock is taken.
/// Lookup reads indexes only; it never reads or serializes canonical bodies.
pub(crate) fn message_metadata(
    store: &dyn crate::durable::ObservationStore,
    message: &crate::message::types::MessageBlock,
) -> Vec<ContentRef> {
    use crate::message::types::{AssistantContentBlock, MessageBlock, UserContentBlock};
    use crate::observation::content::{ContentRepresentation as Rep, ContentSource};
    let reps: Vec<_> = match message {
        MessageBlock::User(user) => user
            .content
            .iter()
            .take(CHECKPOINT_PAGE_ITEMS)
            .map(|block| match block {
                UserContentBlock::Text(_) => Rep::Utf8Text,
                _ => Rep::CanonicalJsonV1,
            })
            .collect(),
        MessageBlock::Assistant(assistant) => assistant
            .content
            .iter()
            .take(CHECKPOINT_PAGE_ITEMS)
            .map(|block| match block {
                AssistantContentBlock::Text(_)
                | AssistantContentBlock::Reasoning(_)
                | AssistantContentBlock::Refusal(_) => Rep::Utf8Text,
                _ => Rep::CanonicalJsonV1,
            })
            .collect(),
        MessageBlock::Tool(tool) => tool
            .result
            .content
            .iter()
            .take(CHECKPOINT_PAGE_ITEMS)
            .map(|block| match block {
                crate::tools::types::ToolResultContent::Text(_) => Rep::Utf8Text,
                _ => Rep::CanonicalJsonV1,
            })
            .collect(),
    };
    reps.into_iter()
        .enumerate()
        .flat_map(|(index, representation)| {
            let source = ContentSource::CanonicalMessageBlock {
                component: crate::observation::content::CanonicalContentComponent::Payload,
                locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
                    message_id: message.id().clone(),
                },
                block_index: crate::message::types::ContentBlockIndex::new(
                    u32::try_from(index).expect("bounded block index"),
                ),
            };
            let mut references = Vec::new();
            let metadata_source = ContentSource::CanonicalMessageBlock {
                component: crate::observation::content::CanonicalContentComponent::BlockMetadata,
                locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
                    message_id: message.id().clone(),
                },
                block_index: crate::message::types::ContentBlockIndex::new(
                    u32::try_from(index).expect("bounded block index"),
                ),
            };
            if let Some(reference) = store
                .content_descriptor(&metadata_source, Rep::CanonicalJsonV1)
                .ok()
                .flatten()
            {
                references.push(reference);
            }
            if let Some(reference) = store
                .content_descriptor(&source, representation)
                .ok()
                .flatten()
            {
                references.push(reference);
            }
            if let Some(reference) = store
                .content_descriptor(&source, Rep::RawJsonUtf8)
                .ok()
                .flatten()
            {
                references.push(reference);
            }
            references
        })
        .collect()
}
pub(crate) fn observation_metadata(
    store: &dyn crate::durable::ObservationStore,
    observation: &crate::runtime::observation::ConversationObservation,
) -> Vec<ContentRef> {
    use crate::runtime::observation::ConversationObservation as Observation;
    #[cfg(not(test))]
    let _ = store;
    observation
        .facts()
        .flat_map(|fact| match fact {
            Observation::CanonicalCommitted { receipt, .. } => receipt
                .observation
                .head
                .iter()
                .flat_map(|header| &header.content)
                .map(|binding| binding.content_ref.clone())
                .collect::<Vec<_>>(),
            Observation::AuditSettled { receipt, .. } => receipt
                .header
                .content
                .iter()
                .map(|binding| binding.content_ref.clone())
                .collect::<Vec<_>>(),
            #[cfg(test)]
            Observation::Committed { block, .. } => message_metadata(store, block),
            _ => Vec::new(),
        })
        .take(CHECKPOINT_PAGE_ITEMS * CHECKPOINT_PAGE_ITEMS)
        .collect()
}

/// Finite native frame read evidence; computed before acquiring Projection synchronization.
#[derive(Clone)]
pub(crate) struct PreparedPublicationContent {
    pub suffix: bool,
    pub base_length: Option<u64>,
    pub stream_id: crate::runtime::identity::PublicationStreamId,
    pub block_index: crate::message::types::ContentBlockIndex,
    pub revision: u64,
    pub component: crate::observation::content::PublicationContentComponent,
    pub representation: crate::observation::content::ContentRepresentation,
    pub readable_end: u64,
}

pub(crate) fn publication_content_evidence(
    observation: &crate::runtime::observation::ConversationObservation,
) -> Vec<PreparedPublicationContent> {
    observation
        .facts()
        .flat_map(|fact| match fact {
            crate::runtime::observation::ConversationObservation::Publication { frame, .. } => {
                publication_frame_evidence(frame)
            }
            _ => Vec::new(),
        })
        .take(1024)
        .collect()
}

#[derive(serde::Serialize)]
struct PublicationMetadata<'a> {
    r#type: &'static str,
    id: &'a crate::runtime::identity::ToolCallId,
    tool_id: &'a crate::runtime::identity::ToolId,
    name: &'a str,
}
struct PublicationLength(u64);
impl std::io::Write for PublicationLength {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len() as u64)
            .filter(|length| *length <= 9_007_199_254_740_991)
            .ok_or_else(|| {
                std::io::Error::other("publication content exceeds native byte range")
            })?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn publication_encoded_length(value: &impl serde::Serialize) -> Option<u64> {
    let mut writer = PublicationLength(0);
    serde_json::to_writer(&mut writer, value).ok()?;
    Some(writer.0)
}

fn publication_frame_evidence(
    frame: &crate::publication::PublicationFrame,
) -> Vec<PreparedPublicationContent> {
    use crate::publication::PublicationPayload;
    let suffix = match &frame.payload {
        PublicationPayload::TextSuffix {
            block_index,
            suffix,
        }
        | PublicationPayload::ReasoningSuffix {
            block_index,
            suffix,
        }
        | PublicationPayload::RefusalSuffix {
            block_index,
            suffix,
        } => Some((
            *block_index,
            suffix,
            crate::observation::content::ContentRepresentation::Utf8Text,
        )),
        PublicationPayload::ProposedToolCallArgumentsSuffix {
            block_index,
            suffix,
            ..
        } => Some((
            *block_index,
            suffix,
            crate::observation::content::ContentRepresentation::RawJsonUtf8,
        )),
        _ => None,
    };
    if let Some((block_index, suffix, representation)) = suffix {
        return vec![PreparedPublicationContent {
            suffix: true,
            base_length: None,
            stream_id: frame.stream_id.clone(),
            block_index,
            revision: frame.sequence + 1,
            component: crate::observation::content::PublicationContentComponent::Payload,
            representation,
            readable_end: suffix.len() as u64,
        }];
    }
    let (block, call_id, tool_id, name, value) = match &frame.payload {
        PublicationPayload::ProposedToolCallStarted { block_index, call } => (
            *block_index,
            &call.id,
            &call.tool_id,
            call.name.as_str(),
            None,
        ),
        PublicationPayload::ProposedToolCallCompleted { block_index, call } => (
            *block_index,
            &call.id,
            &call.tool_id,
            call.name.as_str(),
            Some(&call.arguments),
        ),
        _ => return Vec::new(),
    };
    let make = |component, readable_end| PreparedPublicationContent {
        suffix: false,
        base_length: None,
        stream_id: frame.stream_id.clone(),
        block_index: block,
        revision: frame
            .sequence
            .checked_add(1)
            .expect("native frame revision"),
        component,
        representation: crate::observation::content::ContentRepresentation::CanonicalJsonV1,
        readable_end,
    };
    let mut evidence = Vec::with_capacity(2);
    if let Some(end) = publication_encoded_length(&PublicationMetadata {
        r#type: "tool_call",
        id: call_id,
        tool_id,
        name,
    }) {
        evidence.push(make(
            crate::observation::content::PublicationContentComponent::BlockMetadata,
            end,
        ));
    }
    if let Some(value) = value.and_then(publication_encoded_length) {
        evidence.push(make(
            crate::observation::content::PublicationContentComponent::Payload,
            value,
        ));
    }
    evidence
}

/// Copies only the finite display prefix, leaving native Frame bodies source-owned.
pub(crate) fn bounded_publication_frame(
    frame: &crate::publication::PublicationFrame,
) -> crate::publication::PublicationFrame {
    use crate::publication::PublicationPayload as P;
    let prefix = |text: &str| {
        text.chars()
            .take(CHECKPOINT_ITEM_BYTES / 4)
            .collect::<String>()
    };
    let payload = match &frame.payload {
        P::TextSuffix {
            block_index,
            suffix,
        } => P::TextSuffix {
            block_index: *block_index,
            suffix: prefix(suffix),
        },
        P::ReasoningSuffix {
            block_index,
            suffix,
        } => P::ReasoningSuffix {
            block_index: *block_index,
            suffix: prefix(suffix),
        },
        P::RefusalSuffix {
            block_index,
            suffix,
        } => P::RefusalSuffix {
            block_index: *block_index,
            suffix: prefix(suffix),
        },
        P::ProposedToolCallArgumentsSuffix {
            block_index,
            call_id,
            suffix,
        } => P::ProposedToolCallArgumentsSuffix {
            block_index: *block_index,
            call_id: call_id.clone(),
            suffix: prefix(suffix),
        },
        P::ProposedToolCallStarted { block_index, call } => P::ProposedToolCallStarted {
            block_index: *block_index,
            call: crate::tools::types::ToolCallStart {
                id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                name: prefix(&call.name),
            },
        },
        P::ProposedToolCallCompleted { block_index, call } => P::ProposedToolCallCompleted {
            block_index: *block_index,
            call: crate::tools::types::ToolCall {
                id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                name: prefix(&call.name),
                arguments: bounded_json_value(&call.arguments, 1024)
                    .unwrap_or(serde_json::Value::Null),
            },
        },
        P::TerminalOnly => P::TerminalOnly,
    };
    crate::publication::PublicationFrame {
        payload,
        stream_id: frame.stream_id.clone(),
        message_id: frame.message_id.clone(),
        sequence: frame.sequence,
    }
}

pub(crate) fn bound_publication_payload(
    observation: &mut crate::runtime::observation::ConversationObservation,
) {
    use crate::publication::PublicationPayload as P;
    use crate::runtime::observation::ConversationObservation as O;
    let prefix = |text: &mut String| {
        if text.len() > CHECKPOINT_ITEM_BYTES {
            let mut end = CHECKPOINT_ITEM_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
    };
    match observation {
        O::Publication { frame, .. } => match &mut frame.payload {
            P::TextSuffix { suffix, .. }
            | P::ReasoningSuffix { suffix, .. }
            | P::RefusalSuffix { suffix, .. }
            | P::ProposedToolCallArgumentsSuffix { suffix, .. } => prefix(suffix),
            P::ProposedToolCallStarted { call, .. } => prefix(&mut call.name),
            P::ProposedToolCallCompleted { call, .. } => {
                prefix(&mut call.name);
                call.arguments =
                    bounded_json_value(&call.arguments, 1024).unwrap_or(serde_json::Value::Null);
            }
            P::TerminalOnly => {}
        },
        O::Published { observation, .. } => {
            bound_publication_payload(observation);
        }
        O::JournalBatch { observations, .. } => {
            observations.iter_mut().for_each(bound_publication_payload);
        }
        _ => {}
    }
}

#[cfg(test)]
mod prepared_capability_tests;

#[cfg(test)]
mod budget_tests {
    use super::*;
    use crate::observation::interaction::interaction_request_header;

    fn aggregate_checkpoint() -> RuntimeClientCheckpoint {
        let mut checkpoint = super::super::projection::RuntimeClientProjection::new(
            crate::runtime::identity::ConversationId::new(
                "conv_77621431-ad5b-7155-9eca-9f21014c0bc9",
            ),
            Vec::new(),
            serde_json::from_value(serde_json::json!({"configured_tools":[],"revision":1}))
                .unwrap(),
            None,
            64,
        )
        .checkpoint_candidate()
        .unwrap();
        let mut request: crate::runtime::interaction::InteractionRequest = serde_json::from_str(
            include_str!("../../tests/fixtures/runtime-client/review-v21.json"),
        )
        .unwrap();
        let crate::runtime::interaction::InteractionKind::Review { review, .. } = &mut request.kind
        else {
            panic!("native review fixture");
        };
        review.context = vec![serde_json::Value::String("x".repeat(14_000))];
        for ordinal in 0..CHECKPOINT_PAGE_ITEMS {
            request.id = crate::runtime::identity::InteractionId::new(format!("review-{ordinal}"));
            let mut header = interaction_request_header(
                &request,
                crate::runtime::interaction::InteractionSource::Primary,
            );
            header.source_offset = ordinal as u64;
            header.body = Some(request.clone());
            header.body_complete = true;
            assert!(
                bounded_bytes(&header, CHECKPOINT_ITEM_BYTES).is_some(),
                "individual header has {} actual JSON bytes",
                serde_json::to_vec(&header).unwrap().len()
            );
            checkpoint.snapshot.pending_interactions.push(header);
            checkpoint
                .snapshot
                .messages
                .push(crate::message::types::MessageBlock::Assistant(
                    crate::message::types::AssistantMessageBlock {
                        id: crate::runtime::identity::MessageId::new(format!("message-{ordinal}")),
                        content: vec![crate::message::types::AssistantContentBlock::Text(
                            crate::message::content::TextBlock {
                                text: "y".repeat(14_000),
                            },
                        )],
                    },
                ));
        }
        checkpoint.collections.push(CollectionFrontier {
            domain: "pending_interactions".into(),
            revision: 0,
            total: 32,
            loaded: 32,
            next_offset: None,
        });
        checkpoint
    }

    #[test]
    fn t14_t15_aggregate_budget_defers_bodies_without_losing_actionable_headers_or_cut() {
        let mut checkpoint = aggregate_checkpoint();
        let before = checkpoint.read_cut.clone();
        let identities: Vec<_> = checkpoint
            .snapshot
            .pending_interactions
            .iter()
            .map(|row| {
                (
                    row.interaction.clone(),
                    row.source_offset,
                    row.workflow_node.clone(),
                    row.workflow_node_deferred,
                    row.workflow_node_path.clone(),
                )
            })
            .collect();
        assert!(
            fit_checkpoint(&mut checkpoint).unwrap(),
            "aggregate bytes require one explicit cache replacement"
        );
        assert_eq!(checkpoint.read_cut, before);
        assert_eq!(
            checkpoint
                .snapshot
                .pending_interactions
                .iter()
                .map(|row| (
                    row.interaction.clone(),
                    row.source_offset,
                    row.workflow_node.clone(),
                    row.workflow_node_deferred,
                    row.workflow_node_path.clone()
                ))
                .collect::<Vec<_>>(),
            identities
        );
        assert!(
            checkpoint
                .snapshot
                .pending_interactions
                .iter()
                .all(|row| row.body.is_none() && !row.body_complete)
        );
        assert_eq!(
            checkpoint.snapshot.messages.len(),
            32,
            "canonical loaded bodies fit after deferring request bodies"
        );
        assert_eq!(
            checkpoint
                .collections
                .iter()
                .filter(
                    |frontier| frontier.domain.starts_with("pending_interactions.")
                        && frontier.loaded == 0
                        && frontier.next_offset == Some(0)
                )
                .count(),
            32
        );
        let schema = crate::observation::structural_json::root::<RuntimeClientCheckpoint>();
        assert!(
            bounded_bytes(
                &crate::observation::structural_json::Encoded {
                    value: &checkpoint,
                    schema: &schema,
                    root: &schema
                },
                CHECKPOINT_TARGET_BYTES - CHECKPOINT_ITEM_BYTES
            )
            .is_some()
        );
        assert!(
            !fit_checkpoint(&mut checkpoint).unwrap(),
            "the same bounded cut does not resync again"
        );
    }
}

#[cfg(test)]
mod prepared_trace_tests;
