//! Bounded capture of finite native display descriptors. Capture copies finite
//! descriptors; source-owned bytes are materialized only by their native owner.
use super::bounded::{
    CHECKPOINT_ITEM_BYTES, CHECKPOINT_PAGE_ITEMS, bounded_bytes, bounded_clone, bounded_json_value,
};
use super::bounded::{CHECKPOINT_TARGET_BYTES, bounded_item, bounded_item_bytes};
use super::content::ContentRef;
use super::cut::{CollectionFrontier, ContentBinding};
use serde::Serialize;
use serde::de::DeserializeOwned;

fn message_json_safe(message: &crate::message::types::MessageBlock) -> bool {
    use crate::message::types::{AssistantContentBlock, MessageBlock};
    match message {
        MessageBlock::Assistant(message) => {
            message
                .content
                .iter()
                .take(CHECKPOINT_PAGE_ITEMS)
                .all(|block| match block {
                    AssistantContentBlock::ToolCall(call) => {
                        bounded_json_value(&call.arguments, CHECKPOINT_ITEM_BYTES).is_some()
                    }
                    _ => true,
                })
        }
        MessageBlock::Tool(message) => message
            .result
            .content
            .iter()
            .take(CHECKPOINT_PAGE_ITEMS)
            .all(|block| match block {
                crate::tools::types::ToolResultContent::Json { value } => {
                    bounded_json_value(value, CHECKPOINT_ITEM_BYTES).is_some()
                }
                _ => true,
            }),
        MessageBlock::User(_) => true,
    }
}
fn bounded_inbound_kind(
    kind: &crate::message::types::InboundKind,
) -> crate::message::types::InboundKind {
    use crate::message::types::{ContextKind, InboundKind};
    match kind {
        InboundKind::Context(ContextKind::GoalStatus(goal)) => InboundKind::Context(
            ContextKind::GoalStatus(Box::new(crate::goal::GoalSnapshot {
                reference: goal.reference.clone(),
                objective: goal.objective.chars().take(256).collect(),
                phase: goal.phase,
                blocked_reason: goal
                    .blocked_reason
                    .as_ref()
                    .map(|reason| reason.chars().take(256).collect()),
                autonomous_round_budget: goal.autonomous_round_budget,
                autonomous_rounds_consumed: goal.autonomous_rounds_consumed,
                origin: goal.origin.clone(),
                last_round_message_id: goal.last_round_message_id.clone(),
            })),
        ),
        InboundKind::Message | InboundKind::GoalContinuation(_) | InboundKind::Context(_) => {
            kind.clone()
        }
    }
}
pub(crate) struct Capture {
    pub collections: Vec<CollectionFrontier>,
    pub content: Vec<ContentBinding>,
    pub revision: u64,
    pub(crate) remaining: usize,
    pub(crate) evicted: bool,
}
impl Capture {
    pub fn new(revision: u64) -> Self {
        Self {
            collections: Vec::new(),
            content: Vec::new(),
            revision,
            remaining: CHECKPOINT_TARGET_BYTES / 8,
            evicted: false,
        }
    }
    /// Admission for an already normalized immutable display row. The
    /// certificate belongs to that exact row, never a revision-only lookup.
    pub(crate) fn admit_prepared(&mut self, encoded: usize, required: usize) -> bool {
        if required > CHECKPOINT_ITEM_BYTES.min(self.remaining) || encoded > required {
            self.evicted = true;
            return false;
        }
        self.remaining -= encoded;
        true
    }
    pub fn reserve(&mut self, item: &impl Serialize) -> bool {
        let Some(bytes) = bounded_item_bytes(item, CHECKPOINT_ITEM_BYTES.min(self.remaining))
        else {
            self.evicted = true;
            return false;
        };
        self.remaining -= bytes.len();
        true
    }
    /// Finite display DTOs only. Lexical JSON carriers use `bounded_clone`
    /// instead: their deserializer requires the original capped JSON bytes.
    pub fn item<T: Serialize + DeserializeOwned>(&mut self, domain: &str, item: &T) -> Option<T> {
        let value = bounded_item(item, CHECKPOINT_ITEM_BYTES.min(self.remaining));
        self.evicted |= value.is_none();
        let loaded = u64::from(value.is_some());
        self.frontier(domain, 1, loaded, if loaded == 1 { None } else { Some(0) });
        let (bytes, value) = value?;
        self.remaining -= bytes.len();
        serde_json::from_value(value).ok()
    }
    /// Copies only the admitted DTO head; raw lexical JSON is not a DTO page.
    pub fn page<T: Serialize + DeserializeOwned>(&mut self, domain: &str, items: &[T]) -> Vec<T> {
        let mut result = Vec::new();
        let mut advanced = 0;
        for item in items.iter().take(CHECKPOINT_PAGE_ITEMS) {
            let Some((bytes, value)) =
                bounded_item(item, CHECKPOINT_ITEM_BYTES.min(self.remaining))
            else {
                self.evicted = true;
                break;
            };
            self.remaining -= bytes.len();
            result.push(serde_json::from_value(value).expect("serialized native display value"));
            advanced += 1;
        }
        self.frontier(
            domain,
            items.len() as u64,
            result.len() as u64,
            (advanced < items.len()).then_some(advanced as u64),
        );
        result
    }
    pub fn frontier(&mut self, domain: &str, total: u64, loaded: u64, next_offset: Option<u64>) {
        self.collections.push(CollectionFrontier {
            domain: domain.into(),
            revision: self.revision,
            total,
            loaded,
            next_offset,
        });
    }
}
impl Capture {
    pub fn message_page(
        &mut self,
        domain: &str,
        messages: &[crate::message::types::MessageBlock],
        conversation: &crate::runtime::identity::ConversationId,
    ) -> Vec<crate::message::types::MessageBlock> {
        let mut result = Vec::new();
        for (index, message) in messages.iter().take(CHECKPOINT_PAGE_ITEMS).enumerate() {
            let before = self.content.len();
            let projected =
                self.message(message, conversation, &[domain.into(), index.to_string()]);
            let Some(bytes) = bounded_bytes(&projected, CHECKPOINT_ITEM_BYTES.min(self.remaining))
            else {
                self.content.truncate(before);
                break;
            };
            self.remaining -= bytes.len();
            result.push(projected);
        }
        self.frontier(
            domain,
            messages.len() as u64,
            result.len() as u64,
            (result.len() < messages.len()).then_some(result.len() as u64),
        );
        result
    }
    pub fn message(
        &mut self,
        message: &crate::message::types::MessageBlock,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> crate::message::types::MessageBlock {
        use crate::message::types::MessageBlock;
        let block_count = match message {
            MessageBlock::User(user) => user.content.len(),
            MessageBlock::Assistant(assistant) => assistant.content.len(),
            MessageBlock::Tool(tool) => tool.result.content.len(),
        };
        if block_count <= CHECKPOINT_PAGE_ITEMS
            && message_json_safe(message)
            && let Some(copy) = bounded_clone(message, CHECKPOINT_ITEM_BYTES)
        {
            return copy;
        }
        self.frontier(
            &format!("{}/content", path.join("/")),
            block_count as u64,
            block_count.min(CHECKPOINT_PAGE_ITEMS) as u64,
            (block_count > CHECKPOINT_PAGE_ITEMS).then_some(CHECKPOINT_PAGE_ITEMS as u64),
        );
        match message {
            MessageBlock::User(user) => self.user_message(user, conversation, path),
            MessageBlock::Assistant(assistant) => {
                self.assistant_message(assistant, conversation, path)
            }
            MessageBlock::Tool(tool) => self.tool_message(tool, conversation, path),
        }
    }
    pub(crate) fn user_message(
        &mut self,
        user: &crate::message::types::UserMessageBlock,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> crate::message::types::MessageBlock {
        use super::content::ContentRepresentation as Rep;
        use crate::message::types::{MessageBlock, UserContentBlock};
        let id = &user.id;

        MessageBlock::User(crate::message::types::UserMessageBlock {
            id: user.id.clone(),
            source: user.source.clone(),
            kind: bounded_inbound_kind(&user.kind),
            timestamp: user.timestamp,
            content: user
                .content
                .iter()
                .take(CHECKPOINT_PAGE_ITEMS)
                .enumerate()
                .map(|(index, block)| {
                    let mut field = path.to_vec();
                    field.extend(["content".into(), index.to_string()]);
                    match block {
                        UserContentBlock::Text(text) => {
                            field.push("text".into());
                            UserContentBlock::Text(crate::message::content::TextBlock {
                                text: self.text(
                                    &text.text,
                                    conversation,
                                    id,
                                    index,
                                    field,
                                    Rep::Utf8Text,
                                ),
                            })
                        }
                        other => bounded_clone(other, 1024).unwrap_or_else(|| {
                            self.reference(conversation, id, index, field, Rep::CanonicalJsonV1, 0);
                            UserContentBlock::Text(crate::message::content::TextBlock {
                                text: String::new(),
                            })
                        }),
                    }
                })
                .collect(),
        })
    }

    fn assistant_message(
        &mut self,
        assistant: &crate::message::types::AssistantMessageBlock,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> crate::message::types::MessageBlock {
        use super::content::ContentRepresentation as Rep;
        use crate::message::types::{AssistantContentBlock, MessageBlock};
        let id = &assistant.id;

        MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
            id: assistant.id.clone(),
            content: assistant
                .content
                .iter()
                .take(CHECKPOINT_PAGE_ITEMS)
                .enumerate()
                .map(|(index, block)| {
                    let mut field = path.to_vec();
                    field.extend(["content".into(), index.to_string()]);
                    match block {
                        AssistantContentBlock::Text(text) => {
                            field.push("text".into());
                            AssistantContentBlock::Text(crate::message::content::TextBlock {
                                text: self.text(
                                    &text.text,
                                    conversation,
                                    id,
                                    index,
                                    field,
                                    Rep::Utf8Text,
                                ),
                            })
                        }
                        AssistantContentBlock::Reasoning(reasoning) => {
                            field.push("text".into());
                            AssistantContentBlock::Reasoning(
                                crate::message::types::ReasoningBlock {
                                    text: reasoning.text.as_ref().map(|text| {
                                        self.text(
                                            text,
                                            conversation,
                                            id,
                                            index,
                                            field,
                                            Rep::Utf8Text,
                                        )
                                    }),
                                    provider_state: reasoning
                                        .provider_state
                                        .as_ref()
                                        .and_then(|state| bounded_clone(state, 1024)),
                                },
                            )
                        }
                        AssistantContentBlock::Refusal(refusal) => {
                            field.push("text".into());
                            AssistantContentBlock::Refusal(crate::message::types::RefusalBlock {
                                text: self.text(
                                    &refusal.text,
                                    conversation,
                                    id,
                                    index,
                                    field,
                                    Rep::Utf8Text,
                                ),
                            })
                        }
                        AssistantContentBlock::ToolCall(call) => {
                            field.push("arguments".into());
                            let arguments = bounded_json_value(&call.arguments, 1024)
                                .unwrap_or_else(|| {
                                    self.reference(
                                        conversation,
                                        id,
                                        index,
                                        field,
                                        Rep::CanonicalJsonV1,
                                        0,
                                    );
                                    serde_json::Value::Null
                                });
                            AssistantContentBlock::ToolCall(crate::tools::types::ToolCall {
                                id: call.id.clone(),
                                tool_id: call.tool_id.clone(),
                                name: call.name.clone(),
                                arguments,
                            })
                        }
                        other @ AssistantContentBlock::Image(_) => bounded_clone(other, 1024)
                            .unwrap_or_else(|| {
                                self.reference(
                                    conversation,
                                    id,
                                    index,
                                    field,
                                    Rep::CanonicalJsonV1,
                                    0,
                                );
                                AssistantContentBlock::Text(crate::message::content::TextBlock {
                                    text: String::new(),
                                })
                            }),
                    }
                })
                .collect(),
        })
    }

    fn tool_message(
        &mut self,
        tool: &crate::message::types::ToolMessageBlock,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> crate::message::types::MessageBlock {
        use super::content::ContentRepresentation as Rep;
        use crate::message::types::MessageBlock;
        let id = &tool.id;

        MessageBlock::Tool(crate::message::types::ToolMessageBlock {
            id: tool.id.clone(),
            occurrence: tool.occurrence.clone(),
            tool_call_id: tool.tool_call_id.clone(),
            tool_id: tool.tool_id.clone(),
            result: crate::tools::types::ToolExecutionResult {
                status: tool.result.status.clone(),
                duration_ms: tool.result.duration_ms,
                exit_code: tool.result.exit_code,
                content: tool
                    .result
                    .content
                    .iter()
                    .take(CHECKPOINT_PAGE_ITEMS)
                    .enumerate()
                    .map(|(index, block)| {
                        let mut field = path.to_vec();
                        field.extend(["result".into(), "content".into(), index.to_string()]);
                        match block {
                            crate::tools::types::ToolResultContent::Text(text) => {
                                field.push("text".into());
                                crate::tools::types::ToolResultContent::Text(
                                    crate::message::content::TextBlock {
                                        text: self.text(
                                            &text.text,
                                            conversation,
                                            id,
                                            index,
                                            field,
                                            Rep::Utf8Text,
                                        ),
                                    },
                                )
                            }
                            crate::tools::types::ToolResultContent::Json { value } => {
                                field.push("value".into());
                                crate::tools::types::ToolResultContent::Json {
                                    value: bounded_json_value(value, 1024).unwrap_or_else(|| {
                                        self.reference(
                                            conversation,
                                            id,
                                            index,
                                            field,
                                            Rep::CanonicalJsonV1,
                                            0,
                                        );
                                        serde_json::Value::Null
                                    }),
                                }
                            }
                            other => bounded_clone(other, 1024).unwrap_or_else(|| {
                                self.reference(
                                    conversation,
                                    id,
                                    index,
                                    field,
                                    Rep::CanonicalJsonV1,
                                    0,
                                );
                                crate::tools::types::ToolResultContent::Text(
                                    crate::message::content::TextBlock {
                                        text: String::new(),
                                    },
                                )
                            }),
                        }
                    })
                    .collect(),
                workflow: bounded_clone(&tool.result.workflow, 1024).flatten(),
                artifacts: self.page(
                    &format!("{}/artifacts", path.join("/")),
                    &tool.result.artifacts,
                ),
                deliveries: self.page(
                    &format!("{}/deliveries", path.join("/")),
                    &tool.result.deliveries,
                ),
                truncation: bounded_clone(&tool.result.truncation, 1024).flatten(),
                managed_output: bounded_clone(&tool.result.managed_output, 1024).flatten(),
            },
        })
    }

    fn text(
        &mut self,
        text: &str,
        conversation: &crate::runtime::identity::ConversationId,
        id: &crate::runtime::identity::MessageId,
        index: usize,
        path: Vec<String>,
        representation: super::content::ContentRepresentation,
    ) -> String {
        if text.len() <= 1024 {
            return text.to_owned();
        }
        self.reference(
            conversation,
            id,
            index,
            path,
            representation,
            text.len() as u64,
        );
        String::new()
    }
    fn reference(
        &mut self,
        conversation: &crate::runtime::identity::ConversationId,
        id: &crate::runtime::identity::MessageId,
        index: usize,
        path: Vec<String>,
        representation: super::content::ContentRepresentation,
        readable_end: u64,
    ) {
        self.content.push(ContentBinding {
            source_path: None,
            path,
            deferred: true,
            content_ref: ContentRef {
                conversation_id: conversation.clone(),
                source: super::content::ContentSource::CanonicalMessageBlock {
                    component: crate::observation::content::CanonicalContentComponent::Payload,
                    locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
                        message_id: id.clone(),
                    },
                    block_index: crate::message::types::ContentBlockIndex::new(
                        u32::try_from(index).expect("bounded block index"),
                    ),
                },
                representation,
                version: 1,
                content_revision: 1,
                readable_end,
            },
        });
    }
}
impl Capture {
    pub fn transcript_page(
        &mut self,
        entries: &[super::transcript::RuntimeClientTranscriptEntry],
        conversation: &crate::runtime::identity::ConversationId,
    ) -> Vec<super::transcript::RuntimeClientTranscriptEntry> {
        let mut result = Vec::new();
        for (index, entry) in entries.iter().take(CHECKPOINT_PAGE_ITEMS).enumerate() {
            let before = self.content.len();
            let Some(projected) = self.transcript_entry(
                entry,
                conversation,
                &["transcript".into(), "entries".into(), index.to_string()],
            ) else {
                self.content.truncate(before);
                break;
            };
            let Some(bytes) = bounded_bytes(&projected, CHECKPOINT_ITEM_BYTES.min(self.remaining))
            else {
                self.content.truncate(before);
                break;
            };
            self.remaining -= bytes.len();
            result.push(projected);
        }
        self.frontier(
            "transcript.entries",
            entries.len() as u64,
            result.len() as u64,
            (result.len() < entries.len()).then_some(result.len() as u64),
        );
        result
    }
    fn audit_block(
        &mut self,
        audit: &crate::publication::PublicationAudit,
        block: &crate::publication::PublicationAuditBlock,
        index: usize,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> crate::publication::PublicationAuditBlock {
        use crate::publication::PublicationAuditBlock as Block;
        let mut field = path.to_vec();
        field.extend([
            "item".into(),
            "audit".into(),
            "content".into(),
            index.to_string(),
        ]);
        let (block_index, text, representation) = match block {
            Block::Text { block_index, text }
            | Block::Reasoning { block_index, text }
            | Block::Refusal { block_index, text } => (
                *block_index,
                text,
                super::content::ContentRepresentation::Utf8Text,
            ),
            Block::ProposedToolCall {
                block_index,
                arguments,
                ..
            } => (
                *block_index,
                arguments,
                super::content::ContentRepresentation::RawJsonUtf8,
            ),
        };
        let displayed = if text.len() <= 1024 {
            text.clone()
        } else {
            field.push(
                if representation == super::content::ContentRepresentation::RawJsonUtf8 {
                    "arguments"
                } else {
                    "text"
                }
                .into(),
            );
            self.content.push(ContentBinding {
                source_path: None,
                path: field,
                deferred: true,
                content_ref: ContentRef {
                    conversation_id: conversation.clone(),
                    source: super::content::ContentSource::DurableTranscriptAudit {
                        component:
                            crate::observation::content::PublicationContentComponent::Payload,
                        stream_id: audit.stream_id.clone(),
                        message_id: audit.message_id.clone(),
                        attempt_id: audit.attempt_id.clone(),
                        turn_id: audit.turn_id.clone(),
                        publication_request_id: audit.request_id.clone(),
                        block_index,
                    },
                    representation,
                    version: 1,
                    content_revision: 1,
                    readable_end: text.len() as u64,
                },
            });
            String::new()
        };
        match block {
            Block::Text { block_index, .. } => Block::Text {
                block_index: *block_index,
                text: displayed,
            },
            Block::Reasoning { block_index, .. } => Block::Reasoning {
                block_index: *block_index,
                text: displayed,
            },
            Block::Refusal { block_index, .. } => Block::Refusal {
                block_index: *block_index,
                text: displayed,
            },
            Block::ProposedToolCall {
                block_index,
                call_id,
                tool_id,
                name,
                complete,
                ..
            } => Block::ProposedToolCall {
                block_index: *block_index,
                call_id: call_id.clone(),
                tool_id: tool_id.clone(),
                name: name.chars().take(256).collect(),
                arguments: displayed,
                complete: *complete,
            },
        }
    }
    pub(crate) fn audit(
        &mut self,
        audit: &crate::publication::PublicationAudit,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> Option<crate::publication::PublicationAudit> {
        if [
            audit.stream_id.as_str(),
            audit.message_id.as_str(),
            audit.attempt_id.as_str(),
            audit.turn_id.as_str(),
            audit.request_id.as_str(),
        ]
        .iter()
        .any(|id| id.len() > 256)
        {
            return None;
        }
        let mut body = audit
            .content
            .iter()
            .take(CHECKPOINT_PAGE_ITEMS)
            .take_while(|block| !matches!(block,crate::publication::PublicationAuditBlock::ProposedToolCall{call_id,tool_id,..} if call_id.as_str().len()>256 || tool_id.as_str().len()>256))
            .enumerate()
            .map(|(index,block)|self.audit_block(audit,block,index,conversation,path))
            .collect::<Vec<_>>();
        self.frontier(
            &format!("{}/item/audit/content", path.join("/")),
            audit.content.len() as u64,
            body.len() as u64,
            (body.len() < audit.content.len()).then_some(body.len() as u64),
        );
        Some(crate::publication::PublicationAudit {
            stream_id: audit.stream_id.clone(),
            attempt_id: audit.attempt_id.clone(),
            turn_id: audit.turn_id.clone(),
            request_id: audit.request_id.clone(),
            message_id: audit.message_id.clone(),
            kind: audit.kind,
            content: std::mem::take(&mut body),
            settled_at: audit.settled_at,
        })
    }
    pub fn transcript_entry(
        &mut self,
        entry: &super::transcript::RuntimeClientTranscriptEntry,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> Option<super::transcript::RuntimeClientTranscriptEntry> {
        use super::transcript::RuntimeClientTranscriptItem as Item;
        if let Some(copy) = bounded_clone(entry, CHECKPOINT_ITEM_BYTES) {
            return Some(copy);
        }
        let item = match &entry.item {
            Item::Message { message } => {
                let mut path = path.to_vec();
                path.extend(["item".into(), "message".into()]);
                Item::Message {
                    message: self.message(message, conversation, &path),
                }
            }
            Item::PublicationAudit { audit } => Item::PublicationAudit {
                audit: self.audit(audit, conversation, path)?,
            },
            other => bounded_clone(other, CHECKPOINT_ITEM_BYTES)?,
        };
        Some(super::transcript::RuntimeClientTranscriptEntry {
            cursor: entry.cursor,
            item,
            response_pending: entry.response_pending,
            turn_process: bounded_clone(&entry.turn_process, 2048).flatten(),
            completed_response: bounded_clone(&entry.completed_response, 2048).flatten(),
            tool_calls: self.page(&format!("{}/tool_calls", path.join("/")), &entry.tool_calls),
        })
    }
}
impl Capture {
    pub(crate) fn interaction_page(
        &mut self,
        items: &[super::interaction::RuntimeClientInteractionHeader],
    ) -> Vec<super::interaction::RuntimeClientInteractionHeader> {
        let mut result = Vec::new();
        for (index, header) in items.iter().take(CHECKPOINT_PAGE_ITEMS).enumerate() {
            if !self.reserve(header) {
                break;
            }
            if !header.body_complete {
                self.frontier(&format!("pending_interactions.{index}.body"), 1, 0, Some(0));
            }
            result.push(header.clone());
        }
        self.frontier(
            "pending_interactions",
            items.len() as u64,
            result.len() as u64,
            (result.len() < items.len()).then_some(result.len() as u64),
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    fn byte_oracle_item<T: Serialize + DeserializeOwned>(
        capture: &mut Capture,
        domain: &str,
        item: &T,
    ) -> Option<T> {
        let bytes = bounded_item_bytes(item, CHECKPOINT_ITEM_BYTES.min(capture.remaining));
        capture.evicted |= bytes.is_none();
        let loaded = u64::from(bytes.is_some());
        capture.frontier(domain, 1, loaded, (loaded == 0).then_some(0));
        let bytes = bytes?;
        capture.remaining -= bytes.len();
        serde_json::from_slice(&bytes).ok()
    }

    fn byte_oracle_page<T: Serialize + DeserializeOwned>(
        capture: &mut Capture,
        domain: &str,
        items: &[T],
    ) -> Vec<T> {
        let mut result = Vec::new();
        for item in items.iter().take(CHECKPOINT_PAGE_ITEMS) {
            let Some(bytes) =
                bounded_item_bytes(item, CHECKPOINT_ITEM_BYTES.min(capture.remaining))
            else {
                capture.evicted = true;
                break;
            };
            capture.remaining -= bytes.len();
            result.push(serde_json::from_slice(&bytes).unwrap());
        }
        let loaded = result.len() as u64;
        capture.frontier(
            domain,
            items.len() as u64,
            loaded,
            (loaded < items.len() as u64).then_some(loaded),
        );
        result
    }

    #[test]
    fn dto_capture_value_decode_matches_original_bytes_at_every_admission_boundary() {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Header {
            sequence: u64,
            opaque: serde_json::Value,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            optional: Option<String>,
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            defaults: Vec<u32>,
            progress: crate::tools::types::ToolProgress,
        }
        let original = Header {
            sequence: u64::MAX,
            opaque: serde_json::json!({"signed": -17, "fraction": 1.5, "text": "水\n\\\""}),
            optional: None,
            defaults: Vec::new(),
            progress: crate::tools::types::ToolProgress {
                completed: Some(f64::NAN),
                ..Default::default()
            },
        };
        let mut unsafe_json = original.clone();
        unsafe_json.opaque = serde_json::json!({"integer": u64::MAX});
        let mut too_many = original.clone();
        too_many.defaults = vec![1; CHECKPOINT_PAGE_ITEMS + 1];
        let mut too_large = original.clone();
        too_large.optional = Some("x".repeat(CHECKPOINT_ITEM_BYTES));
        let cases = [original, unsafe_json, too_many, too_large];
        for budget in [
            0,
            3,
            4,
            16,
            256,
            1024,
            CHECKPOINT_ITEM_BYTES - 1,
            CHECKPOINT_ITEM_BYTES,
            32768,
            CHECKPOINT_TARGET_BYTES / 8,
        ] {
            for header in &cases {
                let mut actual = Capture::new(7);
                let mut oracle = Capture::new(7);
                actual.remaining = budget;
                oracle.remaining = budget;
                assert_eq!(
                    actual.item("none", &None::<Header>),
                    byte_oracle_item(&mut oracle, "none", &None::<Header>)
                );
                let value = actual.item("one", header);
                assert_eq!(value, byte_oracle_item(&mut oracle, "one", header));
                if let Some(value) = value {
                    assert_eq!(value.sequence, u64::MAX);
                    assert!(
                        value.progress.completed.is_none(),
                        "DTO deserialization still runs"
                    );
                }
                assert_eq!(
                    actual.page("rows", &cases),
                    byte_oracle_page(&mut oracle, "rows", &cases)
                );
                let repeated = vec![cases[0].clone(); CHECKPOINT_PAGE_ITEMS + 1];
                let loaded = actual.page("bounded_rows", &repeated);
                assert_eq!(
                    loaded,
                    byte_oracle_page(&mut oracle, "bounded_rows", &repeated)
                );
                if budget == CHECKPOINT_TARGET_BYTES / 8 {
                    assert_eq!(loaded.len(), CHECKPOINT_PAGE_ITEMS);
                    assert_eq!(actual.collections.last().unwrap().next_offset, Some(32));
                }
                assert_eq!(actual.remaining, oracle.remaining);
                assert_eq!(actual.collections, oracle.collections);
                assert_eq!(actual.content, oracle.content);
                assert_eq!(actual.evicted, oracle.evicted);
            }
        }
    }
}
