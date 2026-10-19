//! Exact byte locators over the three existing conversation authorities.
//!
//! References are read evidence, never authorization. The attachment/Child
//! owner authorizes the target; the store verifies all byte identity evidence.
use crate::message::types::ContentBlockIndex;
use crate::runtime::identity::{
    AttemptId, ConversationId, MessageId, PublicationStreamId, RequestId, TurnId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CONTENT_READ_MAX_BYTES: u32 = 65_536;
pub const CONTENT_SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentRepresentation {
    Utf8Text,
    RawJsonUtf8,
    BinaryBytes,
    /// Compact JSON, recursively sorted object keys, `serde_json` number/string
    /// encoding, no whitespace; array order is preserved. Never raw JSON.
    CanonicalJsonV1,
}
impl ContentRepresentation {
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Utf8Text => "utf8_text",
            Self::RawJsonUtf8 => "raw_json_utf8",
            Self::BinaryBytes => "binary_bytes",
            Self::CanonicalJsonV1 => "canonical_json_v1",
        }
    }
}
/// Exact deterministic component of one immutable canonical message.
/// Metadata encodings omit content payload fields; loading them merges metadata
/// into the corresponding message/block without replacing already loaded bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalContentComponent {
    Payload,
    BlockMetadata,
    /// Anchored at block index zero even when the message has no content blocks.
    MessageMetadata,
}
impl CanonicalContentComponent {
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Payload => "canonical",
            Self::BlockMetadata => "canonical_block_metadata",
            Self::MessageMetadata => "canonical_message_metadata",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PublicationContentComponent {
    Payload,
    BlockMetadata,
}
/// Finite identity in the existing canonical Ledger, never a new source authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CanonicalContentLocator {
    ByMessageId {
        message_id: MessageId,
    },
    ByLedgerPosition {
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        position: u64,
    },
}
impl CanonicalContentLocator {
    pub(crate) fn key(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::ByMessageId { message_id } => {
                std::borrow::Cow::Owned(format!("message:{}", message_id.as_str()))
            }
            Self::ByLedgerPosition { position } => {
                std::borrow::Cow::Owned(format!("ledger:{position}"))
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentSource {
    CanonicalMessageBlock {
        component: CanonicalContentComponent,
        locator: CanonicalContentLocator,
        block_index: ContentBlockIndex,
    },
    DurableTranscriptAudit {
        component: PublicationContentComponent,
        stream_id: PublicationStreamId,
        message_id: MessageId,
        attempt_id: AttemptId,
        turn_id: TurnId,
        publication_request_id: RequestId,
        block_index: ContentBlockIndex,
    },
    LivePublicationPrefix {
        component: PublicationContentComponent,
        stream_id: PublicationStreamId,
        message_id: MessageId,
        attempt_id: AttemptId,
        turn_id: TurnId,
        publication_request_id: RequestId,
        block_index: ContentBlockIndex,
    },
}
impl ContentSource {
    pub(crate) fn key(&self) -> (&'static str, std::borrow::Cow<'_, str>, u32) {
        match self {
            Self::CanonicalMessageBlock {
                component,
                locator,
                block_index,
            } => (component.key(), locator.key(), block_index.get()),
            Self::DurableTranscriptAudit {
                component,
                stream_id,
                block_index,
                ..
            } => (
                match component {
                    PublicationContentComponent::Payload => "audit",
                    PublicationContentComponent::BlockMetadata => "audit_block_metadata",
                },
                std::borrow::Cow::Borrowed(stream_id.as_str()),
                block_index.get(),
            ),
            Self::LivePublicationPrefix {
                component,
                stream_id,
                block_index,
                ..
            } => (
                match component {
                    PublicationContentComponent::Payload => "live",
                    PublicationContentComponent::BlockMetadata => "live_block_metadata",
                },
                std::borrow::Cow::Borrowed(stream_id.as_str()),
                block_index.get(),
            ),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentRef {
    pub conversation_id: ConversationId,
    pub source: ContentSource,
    pub representation: ContentRepresentation,
    pub version: u16,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    /// Immutable sources use revision 1. For `LivePublicationPrefix` this is
    /// the committed frame sequence plus one, with exact H verified against
    /// the retained staging frontier index and projection release cut.
    pub content_revision: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub readable_end: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentChunk {
    pub content_ref: ContentRef,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub offset: u64,
    pub data_base64: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub end_offset: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub readable_end: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
    pub complete: bool,
}

#[cfg(test)]
mod tests {
    #[test]
    fn t19_canonical_json_v1_recursively_orders_keys_preserves_arrays_and_values() {
        let value = serde_json::json!({"z":[{"z":"\n水", "a":2},1],"a":{"z":true,"a":null}});
        assert_eq!(
            {
                let mut bytes = Vec::new();
                crate::runtime::read_layout::canonical::to_writer(&mut bytes, &value).unwrap();
                bytes
            },
            "{\"a\":{\"a\":null,\"z\":true},\"z\":[{\"a\":2,\"z\":\"\\n水\"},1]}".as_bytes()
        );
    }
}

/// Native P-derived finite display descriptor; byte authority remains in `ContentRefs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LiveBlockHeader {
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub position: u64,
    pub block_index: ContentBlockIndex,
    pub kind: String,
    pub assembled: bool,
    pub content: Vec<ContentRef>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LiveBlockHeaderPage {
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub total: u64,
    pub headers: Vec<LiveBlockHeader>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
}
